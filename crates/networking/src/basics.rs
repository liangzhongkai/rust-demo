//! # 网络编程底层机制
//!
//! 这一节回答四个根本问题，后面 HFT/Web3 章节都建立在这之上：
//!
//! 1. **端点（Endpoint）**：谁连谁、地址怎么表示
//! 2. **流 vs 报文**：TCP 是字节流（无消息边界），UDP 是独立 datagram
//! 3. **读循环**：为什么生产代码永远是 `read → buffer → framing → parse`
//! 4. **分层**：socket I/O、framing、协议解析、业务状态机必须拆开

#![allow(dead_code)]

// ============================================================================
// 1. 端点与地址
// ============================================================================
pub mod endpoint {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SocketAddr {
        pub host: String,
        pub port: u16,
    }

    impl SocketAddr {
        pub fn parse(s: &str) -> Option<Self> {
            let (host, port) = s.rsplit_once(':')?;
            Some(Self {
                host: host.to_string(),
                port: port.parse().ok()?,
            })
        }

        pub fn display(&self) -> String {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn demonstrate() {
        println!("## 1. 端点 Endpoint");
        let gw = SocketAddr::parse("10.0.0.12:443").unwrap();
        println!("订单网关 = {}", gw.display());
        println!("HFT：colo 内网 IP；Web3：多 RPC URL 列表\n");
    }
}

// ============================================================================
// 2. TCP 流 vs UDP 报文
// ============================================================================
pub mod stream_vs_datagram {
    /// TCP：一次 `read` 可能拿到 0.5 帧或 3 帧粘在一起 —— **没有消息边界**
    pub fn tcp_read_simulation(chunks: &[&[u8]]) -> Vec<u8> {
        chunks.iter().flat_map(|c| c.iter().copied()).collect()
    }

    /// UDP：每个 chunk 是完整 datagram（可能丢、可能乱序）
    pub fn udp_read_simulation(datagrams: &[&[u8]]) -> Vec<Vec<u8>> {
        datagrams.iter().map(|d| d.to_vec()).collect()
    }

    pub fn demonstrate() {
        println!("## 2. TCP 流 vs UDP 报文");
        let tcp_buf = tcp_read_simulation(&[
            b"\x00\x00\x00\x05",
            b"HELLO\x00\x00\x00\x02",
            b"XY",
        ]);
        println!("TCP 合并缓冲 {} 字节（需 framing 切帧）", tcp_buf.len());

        let dg = udp_read_simulation(&[b"tick1", b"tick2"]);
        println!("UDP 独立报文 {} 条（需 sequence 去重/补洞）", dg.len());
        println!("规则：永远不要在 read 回调里直接当「一条消息」解析\n");
    }
}

// ============================================================================
// 3. 标准读循环：read → reassemble → parse → dispatch
// ============================================================================
pub mod read_loop {
    #[derive(Default)]
    pub struct Session {
        pub bytes_in: u64,
        pub frames_out: u64,
        buffer: Vec<u8>,
    }

    impl Session {
        pub fn on_read(&mut self, chunk: &[u8]) -> usize {
            self.bytes_in += chunk.len() as u64;
            self.buffer.extend_from_slice(chunk);
            let mut n = 0;
            while self.buffer.len() >= 4 {
                let len = u32::from_be_bytes(self.buffer[0..4].try_into().unwrap()) as usize;
                if self.buffer.len() < 4 + len {
                    break;
                }
                let _frame = &self.buffer[4..4 + len];
                self.buffer.drain(..4 + len);
                self.frames_out += 1;
                n += 1;
            }
            n
        }
    }

    pub fn demonstrate() {
        println!("## 3. 读循环 read → reassemble → parse");
        let mut s = Session::default();
        assert_eq!(s.on_read(&[0, 0, 0, 2, b'H']), 0); // 半帧：缺 1 字节 payload
        assert_eq!(s.on_read(&[b'i', 0, 0, 0, 1, b'!']), 2); // 补全首帧 + 第二帧
        println!("bytes_in={} frames={}", s.bytes_in, s.frames_out);
        println!("生产：I/O 线程只做收包；解析在 framing 层之后\n");
    }
}

// ============================================================================
// 4. 分层架构
// ============================================================================
pub mod layering {
    pub enum IoEvent {
        Connected,
        Data(Vec<u8>),
        Disconnected,
    }

    pub enum Frame {
        Heartbeat,
        OrderAck { id: u64 },
    }

    #[derive(Debug)]
    pub enum BizEvent {
        OrderFilled { id: u64 },
    }

    pub fn io_to_frame(ev: IoEvent) -> Option<Frame> {
        match ev {
            IoEvent::Data(buf) if buf == b"\x00" => Some(Frame::Heartbeat),
            IoEvent::Data(buf) if buf.len() >= 9 && buf[0] == 1 => {
                let id = u64::from_le_bytes(buf[1..9].try_into().ok()?);
                Some(Frame::OrderAck { id })
            }
            _ => None,
        }
    }

    pub fn frame_to_biz(f: Frame) -> Option<BizEvent> {
        match f {
            Frame::OrderAck { id } => Some(BizEvent::OrderFilled { id }),
            Frame::Heartbeat => None,
        }
    }

    pub fn demonstrate() {
        println!("## 4. 分层 socket → frame → business");
        let ev = IoEvent::Data(vec![1, 42, 0, 0, 0, 0, 0, 0, 0]);
        if let Some(f) = io_to_frame(ev) {
            if let Some(b) = frame_to_biz(f) {
                println!("业务事件 {:?}", b);
            }
        }
        println!("好处：换协议/加 TLS 只动下层；策略层不碰字节\n");
    }
}

pub fn demonstrate() {
    endpoint::demonstrate();
    stream_vs_datagram::demonstrate();
    read_loop::demonstrate();
    layering::demonstrate();
}
