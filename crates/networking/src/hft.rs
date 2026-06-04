//! # HFT 生产场景下的网络编程
//!
//! 高频交易的网络硬约束：
//! - **延迟**：热路径避免阻塞 syscall、避免锁竞争、colo 内尽量 UDP 组播
//! - **正确**：TCP 半包、UDP 丢包、会话断线都必须可恢复
//! - **可观测**：每条 feed 都要有 bytes/frame/gap/reconnect 指标
//!
//! 下面 7 个场景对应订单网关、行情组播、FIX 会话等真实写法。

#![allow(dead_code)]

pub type SeqNum = u64;

// ============================================================================
// 场景 1：订单网关 TCP 会话状态机
// ============================================================================
/// **生产问题**：连接交易所 OMS 后要先 Logon，才能发单；断线后不能带着半开状态继续下单。
pub mod order_gateway_session {
    use super::SeqNum;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum State {
        Disconnected,
        Connecting,
        LogonPending,
        Ready,
        Draining,
    }

    #[derive(Debug)]
    pub enum Event {
        TcpConnected,
        LogonAck,
        TcpClosed,
        HeartbeatTimeout,
    }

    pub struct Gateway {
        pub state: State,
        pub outbound_seq: SeqNum,
    }

    impl Gateway {
        pub fn new() -> Self {
            Self {
                state: State::Disconnected,
                outbound_seq: 1,
            }
        }

        pub fn on_event(&mut self, ev: Event) {
            self.state = match (self.state, ev) {
                (State::Disconnected, Event::TcpConnected) => State::LogonPending,
                (State::LogonPending, Event::LogonAck) => State::Ready,
                (State::Ready, Event::TcpClosed) => State::Draining,
                (_, Event::TcpClosed) | (_, Event::HeartbeatTimeout) => State::Disconnected,
                (s, _) => s,
            };
        }

        pub fn can_send_order(&self) -> bool {
            self.state == State::Ready
        }

        pub fn next_seq(&mut self) -> SeqNum {
            let s = self.outbound_seq;
            self.outbound_seq += 1;
            s
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：订单网关 TCP 会话 FSM");
        let mut gw = Gateway::new();
        assert!(!gw.can_send_order());
        gw.on_event(Event::TcpConnected);
        gw.on_event(Event::LogonAck);
        assert!(gw.can_send_order());
        println!("发单 seq={}", gw.next_seq());
        gw.on_event(Event::HeartbeatTimeout);
        assert!(!gw.can_send_order());
        println!("关键：未 Ready 禁止发单；断线清空 in-flight 状态\n");
    }
}

// ============================================================================
// 场景 2：UDP 组播行情 —— 序列号与 gap
// ============================================================================
/// **生产问题**：ITCH/CME 组播会丢包；策略依赖连续 sequence，必须检测 gap 并触发 snapshot。
pub mod multicast_sequence {
    use super::SeqNum;

    #[derive(Debug, Default)]
    pub struct SeqTracker {
        pub expected: SeqNum,
        pub gaps: u64,
        pub dup: u64,
        pub ok: u64,
    }

    impl SeqTracker {
        pub fn on_datagram(&mut self, seq: SeqNum) -> Option<(SeqNum, SeqNum)> {
            if seq < self.expected {
                self.dup += 1;
                return None;
            }
            if seq > self.expected {
                let gap = (seq - self.expected, seq);
                self.gaps += seq - self.expected;
                self.expected = seq + 1;
                self.ok += 1;
                return Some(gap);
            }
            self.expected += 1;
            self.ok += 1;
            None
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：UDP 组播 sequence + gap");
        let mut t = SeqTracker::default();
        t.expected = 100;
        assert!(t.on_datagram(100).is_none());
        let gap = t.on_datagram(103).unwrap();
        println!("gap {:?} → 应请求 snapshot 或 TCP 重放", gap);
        println!("ok={} gaps={} dup={}", t.ok, t.gaps, t.dup);
        println!("关键：gap 是正常事件，不是 panic；要有补洞路径\n");
    }
}

// ============================================================================
// 场景 3：TCP 长度前缀 framing + 半包重组
// ============================================================================
/// **生产问题**：行情走 TCP 时一次 read 常是半帧；Reassembler 必须跨 read 保持状态。
pub mod tcp_framing_reassembly {
    #[derive(Default)]
    pub struct Reassembler {
        buf: Vec<u8>,
        pub frames: u64,
        pub resync_bytes: u64,
    }

    const MAX_FRAME: usize = 65536;

    impl Reassembler {
        pub fn feed(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
            self.buf.extend_from_slice(chunk);
            let mut out = Vec::new();
            loop {
                if self.buf.len() < 4 {
                    break;
                }
                let len = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as usize;
                if len == 0 || len > MAX_FRAME {
                    self.buf.drain(..1);
                    self.resync_bytes += 1;
                    continue;
                }
                if self.buf.len() < 4 + len {
                    break;
                }
                out.push(self.buf[4..4 + len].to_vec());
                self.buf.drain(..4 + len);
                self.frames += 1;
            }
            out
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：TCP framing + 半包重组");
        let mut r = Reassembler::default();
        let a = r.feed(&[0, 0, 0, 3, b'A', b'B']);
        let b = r.feed(&[b'C', 0, 0, 0, 2, b'X', b'Y']);
        println!("两批共 {} 帧，resync={}", a.len() + b.len(), r.resync_bytes);
        println!("关键：Reassembler 生命周期 = 连接生命周期\n");
    }
}

// ============================================================================
// 场景 4：心跳与空闲连接检测
// ============================================================================
/// **生产问题**：防火墙/NAT 会静默丢连接；不发心跳会「假在线」导致订单挂起。
pub mod heartbeat_watchdog {
    use std::time::{Duration, Instant};

    pub struct Watchdog {
        pub last_rx: Instant,
        pub last_tx: Instant,
        pub idle_limit: Duration,
        pub hb_interval: Duration,
    }

    impl Watchdog {
        pub fn new(idle_ms: u64, hb_ms: u64) -> Self {
            let now = Instant::now();
            Self {
                last_rx: now,
                last_tx: now,
                idle_limit: Duration::from_millis(idle_ms),
                hb_interval: Duration::from_millis(hb_ms),
            }
        }

        pub fn on_rx(&mut self) {
            self.last_rx = Instant::now();
        }

        pub fn should_send_hb(&self, now: Instant) -> bool {
            now.duration_since(self.last_tx) >= self.hb_interval
        }

        pub fn is_stale(&self, now: Instant) -> bool {
            now.duration_since(self.last_rx) >= self.idle_limit
        }

        pub fn mark_hb_sent(&mut self, now: Instant) {
            self.last_tx = now;
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：心跳 + 空闲检测");
        let mut w = Watchdog::new(30_000, 1_000);
        let t0 = Instant::now();
        w.on_rx();
        assert!(!w.is_stale(t0));
        let stale_at = t0 + Duration::from_millis(31_000);
        assert!(w.is_stale(stale_at));
        println!("31s 无下行 → stale，应主动 disconnect + 重连");
        println!("关键：心跳与业务消息共用同一 socket；计时用单调时钟\n");
    }
}

// ============================================================================
// 场景 5：背压 —— 慢消费者不能拖垮行情线程
// ============================================================================
/// **生产问题**：策略算慢了，若用无界 channel，内存暴涨并最终 OOM。
pub mod backpressure_bounded {
    use std::collections::VecDeque;

    #[derive(Debug)]
    pub enum PushResult<T> {
        Ok,
        Dropped(T),
    }

    pub struct BoundedQueue<T> {
        cap: usize,
        q: VecDeque<T>,
        pub dropped: u64,
    }

    impl<T> BoundedQueue<T> {
        pub fn new(cap: usize) -> Self {
            Self {
                cap,
                q: VecDeque::with_capacity(cap),
                dropped: 0,
            }
        }

        pub fn push(&mut self, item: T) -> PushResult<T> {
            if self.q.len() >= self.cap {
                self.dropped += 1;
                return PushResult::Dropped(item);
            }
            self.q.push_back(item);
            PushResult::Ok
        }

        pub fn pop(&mut self) -> Option<T> {
            self.q.pop_front()
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：有界队列背压");
        let mut q = BoundedQueue::new(2);
        assert!(matches!(q.push(1), PushResult::Ok));
        assert!(matches!(q.push(2), PushResult::Ok));
        assert!(matches!(q.push(3), PushResult::Dropped(_)));
        println!("dropped={} —— HFT 宁可丢 tick 也不阻塞 I/O 线程", q.dropped);
        println!("关键：热路径用 SPSC + 固定容量；慢路径采样降频\n");
    }
}

// ============================================================================
// 场景 6：FIX MsgSeqNum 空洞与 ResendRequest
// ============================================================================
/// **生产问题**：对端重传或乱序导致 seq 不连续；必须发 ResendRequest 而不是继续交易。
pub mod fix_seq_gap {
    use super::SeqNum;

    pub struct InboundSeq {
        pub next: SeqNum,
        pub resend_from: Option<SeqNum>,
    }

    impl InboundSeq {
        pub fn on_msg(&mut self, seq: SeqNum) -> Option<SeqNum> {
            if seq < self.next {
                return None; // 重复，忽略
            }
            if seq > self.next {
                self.resend_from = Some(self.next);
                self.next = seq + 1;
                return self.resend_from;
            }
            self.next += 1;
            None
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：FIX inbound seq gap → ResendRequest");
        let mut s = InboundSeq { next: 10, resend_from: None };
        assert!(s.on_msg(10).is_none());
        let from = s.on_msg(15).unwrap();
        println!("缺口：从 seq {} 重传", from);
        println!("关键：gap 期间暂停发单；恢复后校验 ExecID 幂等\n");
    }
}

// ============================================================================
// 场景 7：I/O 调度 —— busy-poll vs epoll 延迟权衡
// ============================================================================
/// **生产问题**：colo 里有人用 busy-poll 换 P99 延迟，有人用 epoll 换 CPU；
/// 选型取决于 feed 频率与核心是否独占。
pub mod io_scheduling {
    #[derive(Debug, Clone, Copy)]
    pub struct LatencyProfile {
        pub p50_ns: u64,
        pub p99_ns: u64,
        pub cpu_pct: f32,
    }

    pub fn simulate_busy_poll() -> LatencyProfile {
        LatencyProfile {
            p50_ns: 800,
            p99_ns: 2_000,
            cpu_pct: 95.0,
        }
    }

    pub fn simulate_epoll() -> LatencyProfile {
        LatencyProfile {
            p50_ns: 3_000,
            p99_ns: 25_000,
            cpu_pct: 5.0,
        }
    }

    pub fn pick_strategy(ticks_per_sec: u64, dedicated_core: bool) -> &'static str {
        if ticks_per_sec > 500_000 && dedicated_core {
            "busy-poll / SO_BUSY_POLL"
        } else {
            "epoll / io_uring + edge-trigger"
        }
    }

    pub fn demonstrate() {
        println!("## 场景 7：busy-poll vs epoll");
        let bp = simulate_busy_poll();
        let ep = simulate_epoll();
        println!("busy-poll P99={}ns CPU={}%", bp.p99_ns, bp.cpu_pct);
        println!("epoll     P99={}ns CPU={}%", ep.p99_ns, ep.cpu_pct);
        println!(
            "500k tick/s + 独占核 → {}",
            pick_strategy(600_000, true)
        );
        println!("关键：用 perf + 实际 feed 测；不要照搬博客配置\n");
    }
}

pub fn demonstrate() {
    order_gateway_session::demonstrate();
    multicast_sequence::demonstrate();
    tcp_framing_reassembly::demonstrate();
    heartbeat_watchdog::demonstrate();
    backpressure_bounded::demonstrate();
    fix_seq_gap::demonstrate();
    io_scheduling::demonstrate();
}
