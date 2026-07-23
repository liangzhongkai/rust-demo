//! # Web3 生产场景下的生命周期
//!
//! 链上/链下基础设施硬约束：
//! - **零拷贝解码**：RLP / ABI calldata / log topics 指向原始 bytes
//! - **block 边界**：模拟 / trace 的 scratch 绑在一次 `process_block` 上
//! - **mempool 晋升**：WS 帧内 `'frame` 视图 → 入池前必须 `Arc<[u8]>` / `Bytes`
//! - **async 边界**：`tokio::spawn` 要求 `Future: 'static`，不能借局部 buffer
//!
//! 下面 6 个场景对应 tx 解码、event log、mempool、ABI 缓存、block 模拟、async 边界。

#![allow(dead_code)]

use std::borrow::Cow;
use std::sync::Arc;
use std::thread;

pub type TxHash = [u8; 32];
pub type Address = [u8; 20];

// ============================================================================
// 场景 1：Tx calldata 零拷贝 —— `TxView<'raw>` 绑定 RPC/WS 帧
// ============================================================================
/// **生产问题**：mempool stream 每秒上千笔，不能把每笔 calldata `Vec::from`。
///
/// **套路**：`decode_tx(raw: &'raw [u8]) -> TxView<'raw>`；to/data 仍是切片。
pub mod tx_calldata_view {
    use super::*;

    #[derive(Debug)]
    pub struct TxView<'raw> {
        pub hash: TxHash,
        pub to: Option<Address>,
        pub value_wei: u128,
        pub input: &'raw [u8],
    }

    /// 简化 RLP：假定 layout `[hash 32][to 20 optional][value 16][input rest]`
    pub fn decode_tx(raw: &[u8]) -> Result<TxView<'_>, &'static str> {
        if raw.len() < 32 + 20 + 16 {
            return Err("too short");
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&raw[..32]);
        let mut to = [0u8; 20];
        to.copy_from_slice(&raw[32..52]);
        let value_bytes = &raw[52..68];
        let value_wei = u128::from_be_bytes(value_bytes.try_into().unwrap());
        let input = &raw[68..];
        Ok(TxView {
            hash,
            to: Some(to),
            value_wei,
            input,
        })
    }

    pub fn demonstrate() {
        println!("## Web3-1：TxView<'raw> calldata 零拷贝");

        let mut raw = vec![0u8; 68 + 4];
        raw[..32].fill(0xAB);
        raw[32..52].fill(0xCD);
        raw[52..68].copy_from_slice(&1_000_000_000_000_000_000u128.to_be_bytes());
        raw[68..].copy_from_slice(&[0xA9, 0x05, 0x9C, 0xBB]); // transfer selector 示意

        let tx = decode_tx(&raw).unwrap();
        println!(
            "value={} wei input_len={} selector={:02x?}",
            tx.value_wei,
            tx.input.len(),
            &tx.input[..4.min(tx.input.len())]
        );
        println!("TxView 不能比 raw frame 活得更久\n");
    }
}

// ============================================================================
// 场景 2：Event log 解码 —— topics/data 借 receipt bytes
// ============================================================================
/// **生产问题**：indexer 扫块要解 thousands of logs，避免每 topic `String`。
///
/// **套路**：`LogEntry<'rcpt>` 持 `&'rcpt [u8]` slices。
pub mod event_log_decode {
    use super::*;

    #[derive(Debug)]
    pub struct LogEntry<'rcpt> {
        pub address: Address,
        pub topic0: Option<&'rcpt [u8]>,
        pub data: &'rcpt [u8],
    }

    /// 简化：address[20] + topic0_len[1] + topic0 + data
    pub fn parse_log(raw: &[u8]) -> Result<LogEntry<'_>, &'static str> {
        if raw.len() < 21 {
            return Err("short");
        }
        let mut address = [0u8; 20];
        address.copy_from_slice(&raw[..20]);
        let t0_len = raw[20] as usize;
        if raw.len() < 21 + t0_len {
            return Err("bad topic");
        }
        let topic0 = if t0_len > 0 {
            Some(&raw[21..21 + t0_len])
        } else {
            None
        };
        let data = &raw[21 + t0_len..];
        Ok(LogEntry {
            address,
            topic0,
            data,
        })
    }

    pub fn demonstrate() {
        println!("## Web3-2：LogEntry<'rcpt> 零拷贝");

        let topic = [0xDE; 32];
        let mut raw = Vec::new();
        raw.extend_from_slice(&[0xEE; 20]);
        raw.push(32);
        raw.extend_from_slice(&topic);
        raw.extend_from_slice(b"indexed_payload");

        let log = parse_log(&raw).unwrap();
        println!(
            "topic0_len={} data={:?}",
            log.topic0.map(|t| t.len()).unwrap_or(0),
            std::str::from_utf8(log.data).unwrap_or("<bin>")
        );
        println!("receipt buffer 复用下一笔 tx 前必须 drop 所有 LogEntry\n");
    }
}

// ============================================================================
// 场景 3：Block 模拟 scratch —— arena 绑一次 `simulate_block`
// ============================================================================
/// **生产问题**：searcher 模拟一笔 swap 要临时建 path、字符串 trace label。
///
/// **套路**：`Bump` per simulation；结果汇总后只留 owned 输出。
pub mod block_sim_scratch {
    use bumpalo::Bump;

    pub struct SimStep<'arena> {
        pub label: &'arena str,
        pub gas_used: u64,
    }

    pub struct SimReport {
        pub total_gas: u64,
        pub step_labels: Vec<String>,
    }

    pub fn simulate_block(steps: &[(&str, u64)]) -> SimReport {
        let bump = Bump::new();
        let mut arena_steps: Vec<SimStep<'_>> = Vec::new();
        let mut total = 0u64;

        for &(label, gas) in steps {
            let label_in_arena: &str = bump.alloc_str(label);
            arena_steps.push(SimStep {
                label: label_in_arena,
                gas_used: gas,
            });
            total += gas;
        }

        // 边界：只把需要跨 scope 的字段 owned 化
        let step_labels = arena_steps.iter().map(|s| s.label.to_string()).collect();
        SimReport {
            total_gas: total,
            step_labels,
        }
    }

    pub fn demonstrate() {
        println!("## Web3-3：Block 模拟 arena scratch");

        let report = simulate_block(&[
            ("uniswap_v3_swap", 120_000),
            ("weth_deposit", 45_000),
        ]);
        println!("total_gas={} steps={:?}\n", report.total_gas, report.step_labels);
    }
}

// ============================================================================
// 场景 4：Mempool 晋升 —— `'frame` 视图 → `Arc<[u8]>` 入池
// ============================================================================
/// **生产问题**：WS handler 解析出 `TxView`，但要丢进跨 task 的 mempool map。
///
/// **套路**：短生命周期 parse → 立刻 promote 为 `Arc` / `Bytes`。
pub mod mempool_promote {
    use super::*;
    use tx_calldata_view::{decode_tx, TxView};

    pub struct PooledTx {
        pub raw: Arc<[u8]>,
        pub hash: TxHash,
    }

    impl PooledTx {
        pub fn promote(view: TxView<'_>, raw: &[u8]) -> Self {
            Self {
                raw: Arc::from(raw),
                hash: view.hash,
            }
        }
    }

    pub fn demonstrate() {
        println!("## Web3-4：Mempool 晋升 frame → Arc");

        let raw = vec![0xAB; 72];
        let view = decode_tx(&raw).unwrap();
        let pooled = PooledTx::promote(view, &raw);
        drop(raw);

        let hash = pooled.hash;
        let arc_len = pooled.raw.len();
        thread::spawn(move || {
            let _ = (hash, arc_len);
        })
        .join()
        .unwrap();
        println!("PooledTx.raw 是 Arc；可跨 async task / 线程\n");
    }
}

// ============================================================================
// 场景 5：ABI 缓存 —— `Cow<'static, str>` 读多写少
// ============================================================================
/// **生产问题**：合约 ABI JSON 启动加载后几乎只读；偶尔热更新。
///
/// **套路**：内存里 `Cow<'static, str>`；更新时 `Cow::Owned` 替换。
pub mod abi_cow_cache {
    use super::*;
    use std::sync::RwLock;

    pub struct AbiCache {
        inner: RwLock<Cow<'static, str>>,
    }

    impl AbiCache {
        pub fn from_static(json: &'static str) -> Self {
            Self {
                inner: RwLock::new(Cow::Borrowed(json)),
            }
        }

        pub fn get(&self) -> Cow<'static, str> {
            self.inner.read().unwrap().clone()
        }

        pub fn hot_reload(&self, new_json: String) {
            *self.inner.write().unwrap() = Cow::Owned(new_json);
        }
    }

    pub fn demonstrate() {
        println!("## Web3-5：ABI 缓存 Cow<'static, str>");

        static STARTUP_ABI: &str = r#"{"name":"transfer","type":"function"}"#;
        let cache = AbiCache::from_static(STARTUP_ABI);
        let snap = cache.get();
        println!("abi len={} (borrowed={})", snap.len(), matches!(snap, Cow::Borrowed(_)));
        cache.hot_reload(r#"{"updated":true}"#.to_string());
        println!("after reload: borrowed={}\n", matches!(cache.get(), Cow::Borrowed(_)));
    }
}

// ============================================================================
// 场景 6：Async 边界 —— spawn 需要 `'static`，局部 buffer 必须 owned
// ============================================================================
/// **生产问题**：RPC 回调里 `async { process(&buf) }` 编译失败：buf 不够 `'static`。
///
/// **套路**：spawn 前 `buf.to_vec()` / `Arc<[u8]>`；或整块 future 拥有数据。
pub mod async_spawn_static {
    use super::*;

    pub struct RpcJob {
        pub payload: Arc<[u8]>,
    }

    /// 模拟 `tokio::spawn` 的 `'static` 约束：用 thread + move 演示同一规则。
    pub fn spawn_rpc_handler(payload: Arc<[u8]>) -> usize {
        let handle = thread::spawn(move || payload.len());
        handle.join().unwrap()
    }

    pub fn on_rpc_frame(frame: &[u8]) -> usize {
        let owned: Arc<[u8]> = Arc::from(frame);
        spawn_rpc_handler(owned)
    }

    pub fn demonstrate() {
        println!("## Web3-6：Async/spawn `'static` 边界");

        let frame = b"{\"jsonrpc\":\"2.0\",\"method\":\"eth_call\"}";
        let len = on_rpc_frame(frame);
        println!("spawn processed {} bytes", len);
        println!("不能 spawn 借 `frame` 的 future；须 move owned\n");
    }
}

pub fn demonstrate() {
    tx_calldata_view::demonstrate();
    event_log_decode::demonstrate();
    block_sim_scratch::demonstrate();
    mempool_promote::demonstrate();
    abi_cow_cache::demonstrate();
    async_spawn_static::demonstrate();
}
