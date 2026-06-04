//! # 网络编程常见陷阱与诊断
//!
//! 生产事故里反复出现的 8 个网络陷阱：
//! - 现象（监控/日志里看到什么）
//! - 根因（协议/调度层面发生了什么）
//! - 修法（一行改法 + 风格预防）

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：把 TCP read 当成完整消息
// ============================================================================
pub mod tcp_no_message_boundary {
    pub fn bad_handle(chunk: &[u8]) -> Option<u64> {
        // ❌ 假设每次 read 都是 8 字节完整字段
        Some(u64::from_be_bytes(chunk[0..8].try_into().ok()?))
    }

    pub fn good_handle(buf: &mut Vec<u8>, chunk: &[u8]) -> Option<u64> {
        buf.extend_from_slice(chunk);
        if buf.len() < 8 {
            return None;
        }
        let v = u64::from_be_bytes(buf[0..8].try_into().ok()?);
        buf.drain(..8);
        Some(v)
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：TCP 无消息边界");
        let half = b"\x00\x00\x00";
        // bad_handle(half) 会 panic：对 3 字节切片取 [0..8]
        println!("半包：bad_handle 会 panic；good 先缓冲再取 8 字节");
        let mut buf = Vec::new();
        assert!(good_handle(&mut buf, half).is_none());
        assert_eq!(good_handle(&mut buf, b"\x00\x00\x00\x00\x2a"), Some(42));
        println!("规则：永远 framing 层重组后再 parse\n");
    }
}

// ============================================================================
// 陷阱 2：热路径阻塞式 I/O
// ============================================================================
pub mod blocking_on_hot_path {
    pub fn bad_pattern() -> &'static str {
        "read_exact() 在策略线程里阻塞 → 错过下一 tick"
    }

    pub fn good_pattern() -> &'static str {
        "I/O 线程 non-blocking/epoll → SPSC 投递解码后事件"
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：热路径阻塞 I/O");
        println!("❌ {}", bad_pattern());
        println!("✅ {}", good_pattern());
        println!("规则：行情线程只做无锁消费；syscall 隔离\n");
    }
}

// ============================================================================
// 陷阱 3：无界 channel 导致 OOM
// ============================================================================
pub mod unbounded_channel {
    pub fn bad_queue_push(q: &mut Vec<u64>, item: u64) {
        q.push(item); // ❌ 慢消费者时无限增长
    }

    pub fn good_queue_push(q: &mut Vec<u64>, cap: usize, item: u64) -> bool {
        if q.len() >= cap {
            return false;
        }
        q.push(item);
        true
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：无界队列 OOM");
        let mut q = Vec::new();
        for i in 0..5 {
            good_queue_push(&mut q, 3, i);
        }
        assert!(!good_queue_push(&mut q, 3, 99));
        println!("有界 cap=3 时 len={}", q.len());
        println!("规则：mpsc 用 try_send；满则 drop + metric\n");
    }
}

// ============================================================================
// 陷阱 4：无心跳 → 僵尸连接
// ============================================================================
pub mod zombie_connection {
    pub struct Conn {
        pub alive: bool,
        pub last_rx_ms: u64,
    }

    pub fn bad_is_connected(c: &Conn) -> bool {
        c.alive // ❌ 对端已死但本地仍认为在线
    }

    pub fn good_is_connected(c: &Conn, now_ms: u64, timeout_ms: u64) -> bool {
        c.alive && now_ms.saturating_sub(c.last_rx_ms) < timeout_ms
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：僵尸连接");
        let c = Conn {
            alive: true,
            last_rx_ms: 0,
        };
        println!("bad @60s = {}", bad_is_connected(&c));
        println!("good @60s = {}", good_is_connected(&c, 60_000, 30_000));
        println!("规则：应用层心跳 + TCP keepalive 双保险\n");
    }
}

// ============================================================================
// 陷阱 5：忽略 UDP sequence gap
// ============================================================================
pub mod ignore_udp_gap {
    pub fn bad_advance(_expected: &mut u64, seq: u64) {
        *_expected = seq + 1; // ❌ 静默跳过丢失的 tick
    }

    pub fn good_advance(expected: &mut u64, seq: u64) -> Option<(u64, u64)> {
        if seq > *expected {
            let gap = (*expected, seq);
            *expected = seq + 1;
            return Some(gap);
        }
        if seq == *expected {
            *expected += 1;
        }
        None
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：忽略 UDP gap");
        let mut e = 100u64;
        bad_advance(&mut e, 105);
        println!("bad 后 expected={}（中间 101-104 被吞）", e);
        let mut e2 = 100u64;
        println!("good gap = {:?}", good_advance(&mut e2, 105));
        println!("规则：gap → snapshot / 重放 channel\n");
    }
}

// ============================================================================
// 陷阱 6：Web3 热路径解析完整 JSON
// ============================================================================
pub mod json_on_hot_path {
    pub fn bad_dispatch(line: &str) -> Option<&str> {
        // ❌ 每条 WS 消息 serde_json::from_str
        line.split("\"result\":").nth(1)
    }

    pub fn good_dispatch(line: &str) -> Option<&str> {
        // ✅ 先比 method/subscription 前缀，再局部解析
        if line.contains(r#""method":"eth_subscription""#) {
            line.split("\"result\":").nth(1)
        } else {
            None
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 6：热路径全量 JSON");
        let msg = r#"{"method":"eth_subscription","params":{"result":"0xabc"}}"#;
        assert!(good_dispatch(msg).is_some());
        println!("规则：索引器冷路径用 serde；热路径用 simd-json / 字段提取\n");
    }
}

// ============================================================================
// 陷阱 7：多线程共享连接无同步
// ============================================================================
pub mod shared_connection_race {
    // ❌ Arc<TcpStream> 多线程同时 write
    // ✅ 单 writer 任务 + channel 发命令

    pub enum OrderCmd {
        New { id: u64 },
    }

    pub struct SingleWriter {
        pub queue: Vec<OrderCmd>,
    }

    impl SingleWriter {
        pub fn enqueue(&mut self, cmd: OrderCmd) {
            self.queue.push(cmd);
        }

        pub fn drain_to_wire(&mut self) -> Vec<u8> {
            let mut wire = Vec::new();
            for c in self.queue.drain(..) {
                match c {
                    OrderCmd::New { id } => wire.extend_from_slice(&id.to_le_bytes()),
                }
            }
            wire
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：连接多线程写");
        let mut w = SingleWriter { queue: vec![] };
        w.enqueue(OrderCmd::New { id: 1 });
        w.enqueue(OrderCmd::New { id: 2 });
        println!("单 writer 刷出 {} 字节", w.drain_to_wire().len());
        println!("规则：连接 = 单所有者；多生产者用 mpsc → writer\n");
    }
}

// ============================================================================
// 陷阱 8：重连无退避 —— 打爆对端
// ============================================================================
pub mod reconnect_storm {
    pub fn bad_delay(_attempt: u32) -> u64 {
        0 // ❌ 立即重连风暴
    }

    pub fn good_delay(attempt: u32) -> u64 {
        let base = 100u64;
        let max = 30_000u64;
        (base * 2u64.saturating_pow(attempt.min(10))).min(max)
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：重连风暴");
        for a in 0..5 {
            println!("  attempt {} → {}ms", a, good_delay(a));
        }
        println!("bad 立即重连 = {}ms", bad_delay(0));
        println!("规则：指数退避 + jitter；会话层先 Logout 再连\n");
    }
}

pub fn demonstrate() {
    tcp_no_message_boundary::demonstrate();
    blocking_on_hot_path::demonstrate();
    unbounded_channel::demonstrate();
    zombie_connection::demonstrate();
    ignore_udp_gap::demonstrate();
    json_on_hot_path::demonstrate();
    shared_connection_race::demonstrate();
    reconnect_storm::demonstrate();
}
