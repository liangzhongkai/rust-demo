//! # 泛化：从 HFT/Web3 场景到通用网络策略
//!
//! | 问题类型           | 标志特征                     | 首选套路                          |
//! |--------------------|------------------------------|-----------------------------------|
//! | 1. Framing 层      | TCP/WebSocket/P2P 字节流     | Reassembler + 长度上限            |
//! | 2. 会话状态机      | Logon/订阅/握手              | 显式 FSM + 非法迁移拒绝           |
//! | 3. 背压            | 生产 > 消费                  | 有界队列 + drop 策略 + metric     |
//! | 4. 健康检测        | 静默断连                     | 心跳 + idle timeout               |
//! | 5. 重连恢复        | 断线、seq gap                | 退避 + resync + 幂等              |
//! | 6. 多路复用        | 多请求/多订阅                | id 关联 / 单 writer               |
//! | 7. 多节点容错      | 公共 RPC 不稳                | 健康分 + failover + 限流          |
//! | 8. 可观测性        | 线上「卡了」                 | bytes/frame/gap/reconnect 指标    |

#![allow(dead_code)]

// ============================================================================
// 策略 1：Framing 层独立
// ============================================================================
pub mod framing_layer {
    #[derive(Default)]
    pub struct Framer {
        buf: Vec<u8>,
    }

    impl Framer {
        pub fn push(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
            self.buf.extend_from_slice(chunk);
            let mut frames = Vec::new();
            while self.buf.len() >= 4 {
                let n = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as usize;
                if self.buf.len() < 4 + n {
                    break;
                }
                frames.push(self.buf[4..4 + n].to_vec());
                self.buf.drain(..4 + n);
            }
            frames
        }
    }

    pub fn demonstrate() {
        println!("## 策略 1：Framing 层");
        let mut f = Framer::default();
        let frames = f.push(&[0, 0, 0, 2, b'x', b'y']);
        println!("帧数 = {}", frames.len());
        println!("HFT: tcp_framing | Web3: RLPx / WS frame\n");
    }
}

// ============================================================================
// 策略 2：会话 FSM
// ============================================================================
pub mod session_fsm {
    #[derive(Debug, PartialEq)]
    pub enum S {
        Init,
        Active,
    }

    pub fn transition(s: S, ev: &str) -> S {
        match (s, ev) {
            (S::Init, "hello") => S::Active,
            (S::Active, "bye") => S::Init,
            (s, _) => s,
        }
    }

    pub fn demonstrate() {
        println!("## 策略 2：会话 FSM");
        let s = transition(S::Init, "hello");
        assert_eq!(s, S::Active);
        println!("HFT: order_gateway_session | Web3: ws_resubscribe\n");
    }
}

// ============================================================================
// 策略 3：背压
// ============================================================================
pub mod backpressure {
    pub struct Meter {
        pub pushed: u64,
        pub dropped: u64,
    }

    pub fn try_push(m: &mut Meter, cap: usize, len: usize) -> bool {
        if len >= cap {
            m.dropped += 1;
            false
        } else {
            m.pushed += 1;
            true
        }
    }

    pub fn demonstrate() {
        println!("## 策略 3：背压 + metric");
        let mut m = Meter { pushed: 0, dropped: 0 };
        try_push(&mut m, 10, 5);
        try_push(&mut m, 10, 15);
        println!("pushed={} dropped={}", m.pushed, m.dropped);
        println!("HFT: backpressure_bounded | Web3: indexer 降采样\n");
    }
}

// ============================================================================
// 策略 4：健康检测
// ============================================================================
pub mod health_check {
    pub struct Health {
        pub last_ok_ms: u64,
        pub timeout_ms: u64,
    }

    impl Health {
        pub fn is_healthy(&self, now_ms: u64) -> bool {
            now_ms.saturating_sub(self.last_ok_ms) < self.timeout_ms
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：健康检测");
        let h = Health {
            last_ok_ms: 0,
            timeout_ms: 30_000,
        };
        println!("@5s healthy={}", h.is_healthy(5_000));
        println!("@60s healthy={}", h.is_healthy(60_000));
        println!("HFT: heartbeat_watchdog | Web3: RPC probe eth_blockNumber\n");
    }
}

// ============================================================================
// 策略 5：重连 + 状态恢复
// ============================================================================
pub mod reconnect_resync {
    pub fn backoff_ms(attempt: u32) -> u64 {
        (100 * 2u64.saturating_pow(attempt.min(8))).min(30_000)
    }

    pub struct ResyncState {
        pub last_seq: u64,
        pub need_snapshot: bool,
    }

    pub fn on_reconnect(s: &mut ResyncState, has_gap: bool) {
        s.need_snapshot = has_gap;
    }

    pub fn demonstrate() {
        println!("## 策略 5：退避 + resync");
        println!("attempt 3 → {}ms", backoff_ms(3));
        let mut st = ResyncState {
            last_seq: 1000,
            need_snapshot: false,
        };
        on_reconnect(&mut st, true);
        println!("need_snapshot = {}", st.need_snapshot);
        println!("HFT: fix_seq_gap | Web3: ws_resubscribe + 区块回放\n");
    }
}

// ============================================================================
// 策略 6：请求多路复用
// ============================================================================
pub mod multiplexing {
    use std::collections::HashMap;

    pub struct Multiplexer {
        pending: HashMap<u64, &'static str>,
    }

    impl Multiplexer {
        pub fn insert(&mut self, id: u64, tag: &'static str) {
            self.pending.insert(id, tag);
        }

        pub fn resolve(&mut self, id: u64) -> Option<&'static str> {
            self.pending.remove(&id)
        }
    }

    pub fn demonstrate() {
        println!("## 策略 6：id 多路复用");
        let mut m = Multiplexer { pending: HashMap::new() };
        m.insert(1, "balance");
        m.insert(2, "block");
        println!("resolved = {:?}", m.resolve(2));
        println!("HFT: 单连接 seq | Web3: json_rpc_correlation\n");
    }
}

// ============================================================================
// 策略 7：多节点容错
// ============================================================================
pub mod multi_endpoint {
    pub fn pick<'a>(urls: &'a [(&'a str, bool)]) -> Option<&'a str> {
        urls.iter().find(|(_, ok)| *ok).map(|(u, _)| *u)
    }

    pub fn demonstrate() {
        println!("## 策略 7：多节点 failover");
        let urls = [("https://a", false), ("https://b", true)];
        println!("选中 {:?}", pick(&urls));
        println!("Web3: rpc_failover | HFT: 主备行情 feed\n");
    }
}

// ============================================================================
// 策略 8：可观测性
// ============================================================================
pub mod observability {
    #[derive(Default, Debug)]
    pub struct NetMetrics {
        pub bytes_in: u64,
        pub frames: u64,
        pub gaps: u64,
        pub reconnects: u64,
        pub drops: u64,
    }

    pub fn demonstrate() {
        println!("## 策略 8：网络指标");
        let m = NetMetrics {
            bytes_in: 1_000_000,
            frames: 50_000,
            gaps: 3,
            reconnects: 1,
            drops: 12,
        };
        println!("{:?}", m);
        println!("告警：gaps↑ 且 reconnects↑ → 链路或对端问题\n");
    }
}

// ============================================================================
// 反例：什么时候不要自建网络栈
// ============================================================================
pub mod when_not_to_handroll {
    pub fn demonstrate() {
        println!("## 反例：何时不要自建");
        println!("  - HTTP/REST 客户端 → reqwest / hyper");
        println!("  - 全功能 WebSocket → tokio-tungstenite / axum");
        println!("  - FIX 会话 → quickfix 或 vendor SDK");
        println!("  - 以太坊 P2P → reth/net 模块");
        println!("  - 手写前先问：协议是否标准 + 有无成熟连接池？\n");
    }
}

pub fn demonstrate() {
    framing_layer::demonstrate();
    session_fsm::demonstrate();
    backpressure::demonstrate();
    health_check::demonstrate();
    reconnect_resync::demonstrate();
    multiplexing::demonstrate();
    multi_endpoint::demonstrate();
    observability::demonstrate();
    when_not_to_handroll::demonstrate();
}
