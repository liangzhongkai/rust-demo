//! # Web3 / 区块链生产场景下的网络编程
//!
//! Web3 的网络负载特征：
//! - **JSON-RPC / WebSocket**：节点通信主流协议
//! - **长连接 + 订阅**：断线必须 resubscribe
//! - **多节点容错**：公共 RPC 限流、宕机、链重组
//! - **P2P gossip**：交易哈希去重、fan-out 扇出
//!
//! 下面 6 个场景对应 indexer、bot、钱包、执行客户端里的常见写法。

#![allow(dead_code)]

pub type JsonId = u64;

// ============================================================================
// 场景 1：JSON-RPC 请求/响应 id 关联
// ============================================================================
/// **生产问题**：并发发多个 `eth_call`，响应乱序到达；必须用 id 匹配 pending 表。
pub mod json_rpc_correlation {
    use super::JsonId;
    use std::collections::HashMap;

    #[derive(Debug)]
    pub struct Pending {
        pub method: &'static str,
    }

    pub struct RpcClient {
        next_id: JsonId,
        pending: HashMap<JsonId, Pending>,
    }

    impl RpcClient {
        pub fn new() -> Self {
            Self {
                next_id: 1,
                pending: HashMap::new(),
            }
        }

        pub fn call(&mut self, method: &'static str) -> (JsonId, String) {
            let id = self.next_id;
            self.next_id += 1;
            self.pending.insert(id, Pending { method });
            let req = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":[]}}"#);
            (id, req)
        }

        pub fn on_response(&mut self, id: JsonId, result: &str) -> Option<String> {
            let p = self.pending.remove(&id)?;
            Some(format!("{} → {}", p.method, result))
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：JSON-RPC id 关联");
        let mut c = RpcClient::new();
        let (id1, _) = c.call("eth_blockNumber");
        let (id2, _) = c.call("eth_getBalance");
        println!("{}", c.on_response(id2, r#""0x0""#).unwrap());
        println!("{}", c.on_response(id1, r#""0x10""#).unwrap());
        println!("关键：超时清理 pending；id 用 AtomicU64 防锁\n");
    }
}

// ============================================================================
// 场景 2：WebSocket 订阅 + 断线重订阅
// ============================================================================
/// **生产问题**：`eth_subscribe` 后 TCP 断了，重连必须重发全部 subscription。
pub mod ws_resubscribe {
    #[derive(Debug, Clone)]
    pub struct Subscription {
        pub sub_id: String,
        pub method: &'static str,
    }

    pub struct WsSession {
        pub connected: bool,
        pub subs: Vec<Subscription>,
    }

    impl WsSession {
        pub fn subscribe(&mut self, method: &'static str) {
            self.subs.push(Subscription {
                sub_id: format!("0x{}", self.subs.len()),
                method,
            });
        }

        pub fn on_disconnect(&mut self) {
            self.connected = false;
        }

        pub fn on_reconnect(&mut self) -> Vec<&'static str> {
            self.connected = true;
            self.subs.iter().map(|s| s.method).collect()
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：WebSocket 断线重订阅");
        let mut s = WsSession {
            connected: true,
            subs: vec![],
        };
        s.subscribe("newHeads");
        s.subscribe("logs");
        s.on_disconnect();
        let methods = s.on_reconnect();
        println!("重连后重发: {:?}", methods);
        println!("关键：sub_id 会变；用本地 logical_id 映射\n");
    }
}

// ============================================================================
// 场景 3：devp2p / RLPx 帧头（长度前缀 + 能力握手）
// ============================================================================
/// **生产问题**：执行层 P2P 在加密前先交换 Hello；帧体有最大长度防 DoS。
pub mod rlpx_framing {
    pub const MAX_FRAME: usize = 16 * 1024 * 1024;

    #[derive(Debug)]
    pub struct FrameHeader {
        pub body_len: u32,
    }

    pub fn decode_header(buf: &[u8]) -> Result<FrameHeader, &'static str> {
        if buf.len() < 3 {
            return Err("short header");
        }
        let body_len = ((buf[0] as u32) << 16) | ((buf[1] as u32) << 8) | (buf[2] as u32);
        if body_len as usize > MAX_FRAME {
            return Err("frame too large");
        }
        Ok(FrameHeader { body_len })
    }

    pub fn demonstrate() {
        println!("## 场景 3：RLPx 帧头（3-byte length + cap）");
        let h = decode_header(&[0, 0, 32]).unwrap();
        println!("body_len={}", h.body_len);
        println!("err = {:?}", decode_header(&[0xFF, 0xFF, 0xFF]));
        println!("关键：len 先验校验再分配 buffer；防 peer DoS\n");
    }
}

// ============================================================================
// 场景 4：Mempool gossip 去重
// ============================================================================
/// **生产问题**：同一 tx 从多个 peer 传来；广播前必须 dedup，否则 fan-out 风暴。
pub mod mempool_dedup {
    use std::collections::HashSet;

    pub type TxHash = [u8; 32];

    pub struct SeenSet {
        cap: usize,
        set: HashSet<TxHash>,
        pub forwarded: u64,
        pub dup: u64,
    }

    impl SeenSet {
        pub fn new(cap: usize) -> Self {
            Self {
                cap,
                set: HashSet::with_capacity(cap),
                forwarded: 0,
                dup: 0,
            }
        }

        pub fn on_gossip(&mut self, hash: TxHash) -> bool {
            if self.set.contains(&hash) {
                self.dup += 1;
                return false;
            }
            if self.set.len() >= self.cap {
                self.set.clear(); // 生产用 LRU / 时间窗 Bloom
            }
            self.set.insert(hash);
            self.forwarded += 1;
            true
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：Mempool gossip dedup");
        let mut s = SeenSet::new(100);
        let h = [1u8; 32];
        assert!(s.on_gossip(h));
        assert!(!s.on_gossip(h));
        println!("forwarded={} dup={}", s.forwarded, s.dup);
        println!("关键：seen 表有界；过期按 block height 滚动\n");
    }
}

// ============================================================================
// 场景 5：多 RPC 端点 failover
// ============================================================================
/// **生产问题**：Alchemy/Infura 限流或 5xx；indexer 要在健康节点间切换。
pub mod rpc_failover {
    #[derive(Debug, Clone)]
    pub struct Endpoint {
        pub url: &'static str,
        pub latency_ms: u32,
        pub error_rate: f32,
        pub blocked: bool,
    }

    pub fn score(e: &Endpoint) -> f32 {
        if e.blocked {
            return f32::MIN;
        }
        let lat_penalty = e.latency_ms as f32;
        let err_penalty = e.error_rate * 1000.0;
        1000.0 - lat_penalty - err_penalty
    }

    pub fn pick_best(endpoints: &[Endpoint]) -> Option<&Endpoint> {
        endpoints.iter().max_by(|a, b| {
            score(a)
                .partial_cmp(&score(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    pub fn demonstrate() {
        println!("## 场景 5：多 RPC failover");
        let eps = vec![
            Endpoint {
                url: "https://a",
                latency_ms: 50,
                error_rate: 0.01,
                blocked: false,
            },
            Endpoint {
                url: "https://b",
                latency_ms: 30,
                error_rate: 0.5,
                blocked: false,
            },
        ];
        let best = pick_best(&eps).unwrap();
        println!("选中 {} score={:.1}", best.url, score(best));
        println!("关键：滑动窗口统计；429 时 cooldown + 换节点\n");
    }
}

// ============================================================================
// 场景 6：批量 JSON-RPC（indexer 回填）
// ============================================================================
/// **生产问题**：同步历史区块时逐个 `eth_getBlockByNumber` 太慢；HTTP batch 减少 RTT。
pub mod json_rpc_batch {
    pub fn build_batch(ids: &[u64], block_nums: &[u64]) -> String {
        let items: Vec<String> = ids
            .iter()
            .zip(block_nums)
            .map(|(id, n)| {
                format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"method":"eth_getBlockByNumber","params":["0x{n:x}",false]}}"#
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    pub fn parse_batch_response(body: &str) -> Vec<(u64, bool)> {
        // 教学简化：真实用 serde_json::Value 或 simd-json
        body.split('"')
            .filter_map(|s| s.parse::<u64>().ok())
            .zip([true, true, false])
            .collect()
    }

    pub fn demonstrate() {
        println!("## 场景 6：JSON-RPC HTTP batch");
        let batch = build_batch(&[1, 2, 3], &[100, 101, 102]);
        println!("请求体 {} 字节", batch.len());
        println!("关键：batch 大小受节点限制（常见 1000）；失败要拆批重试\n");
    }
}

pub fn demonstrate() {
    json_rpc_correlation::demonstrate();
    ws_resubscribe::demonstrate();
    rlpx_framing::demonstrate();
    mempool_dedup::demonstrate();
    rpc_failover::demonstrate();
    json_rpc_batch::demonstrate();
}
