//! # Web3 生产场景下的 Interior Mutability
//!
//! 链上/链下基础设施硬约束：
//! - **单线程 reactor**：WS 订阅、mempool stream 常在单 task 内 dedup / 缓冲
//! - **nonce / gas**：多 signer 或 RPC 线程争用，Atomic 或细粒度 Mutex
//! - **索引 reorg**：pending 块缓冲用 RefCell，确认后 flush；回滚时 mutate
//! - **缓存**：ABI / decimals 读多写少 → RwLock 或 OnceLock
//!
//! 下面 6 个场景对应 mempool、钱包、indexer、searcher、多链客户端。

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub type TxHash = [u8; 32];
pub type Address = [u8; 20];
pub type BlockNum = u64;

// ============================================================================
// 场景 1：Mempool 去重 —— RefCell<HashSet> 单线程 reactor
// ============================================================================
/// **生产问题**：eth_subscribe 每秒上千 tx hash；要在 `&self` 的 stream handler 里
/// 标记已见，避免重复模拟 / 广播。
///
/// **套路**：`RefCell<HashSet<TxHash>>` + 环形淘汰（此处简化为 retain 上限）。
pub mod mempool_dedup {
    use super::*;

    pub struct SeenCache {
        seen: RefCell<HashSet<TxHash>>,
        cap: usize,
        pub hits: Cell<u64>,
        pub inserts: Cell<u64>,
    }

    impl SeenCache {
        pub fn new(cap: usize) -> Self {
            Self {
                seen: RefCell::new(HashSet::new()),
                cap,
                hits: Cell::new(0),
                inserts: Cell::new(0),
            }
        }

        pub fn check_and_insert(&self, hash: TxHash) -> bool {
            let mut set = self.seen.borrow_mut();
            if set.contains(&hash) {
                self.hits.set(self.hits.get() + 1);
                return false;
            }
            if set.len() >= self.cap {
                set.clear(); // 生产用 LRU / 世代；此处演示 RefCell mutate
            }
            set.insert(hash);
            self.inserts.set(self.inserts.get() + 1);
            true
        }
    }

    pub fn demonstrate() {
        println!("## Web3-1：Mempool 去重 RefCell");

        let cache = SeenCache::new(10_000);
        let h = [0xAB; 32];
        let first = cache.check_and_insert(h);
        let dup = cache.check_and_insert(h);
        println!(
            "first={} dup={} hits={}",
            first,
            dup,
            cache.hits.get()
        );
        println!("单线程 WS handler；多线程改用 DashSet / sharded Mutex\n");
    }
}

// ============================================================================
// 场景 2：账户 Nonce —— AtomicU64
// ============================================================================
/// **生产问题**：searcher / relayer 并发构造 tx，nonce 不能重复也不能大跳。
/// 热路径 `fetch_add` 比 Mutex 轻。
///
/// **套路**：`AtomicU64` 存 next nonce；失败回滚用 compare_exchange（此处演示 add）。
pub mod nonce_atomic {
    use super::*;

    pub struct NonceTracker {
        next: AtomicU64,
        pub resyncs: Cell<u64>,
    }

    impl NonceTracker {
        pub fn new(start: u64) -> Self {
            Self {
                next: AtomicU64::new(start),
                resyncs: Cell::new(0),
            }
        }

        pub fn take(&self) -> u64 {
            self.next.fetch_add(1, Ordering::SeqCst)
        }

        /// 链上 nonce 落后时由单线程 reconciler 调用。
        pub fn resync(&self, chain_nonce: u64) {
            self.next.store(chain_nonce, Ordering::SeqCst);
            self.resyncs.set(self.resyncs.get() + 1);
        }
    }

    pub fn demonstrate() {
        println!("## Web3-2：Nonce AtomicU64");

        let nonce = Arc::new(NonceTracker::new(42));
        let n1 = nonce.take();
        let n2 = nonce.take();
        nonce.resync(44);
        let n3 = nonce.take();
        println!("nonces {} {} → resync → {}", n1, n2, n3);
        println!("多 signer 时按 account 分片，每片一个 Atomic\n");
    }
}

// ============================================================================
// 场景 3：合约 ABI / 解码表 —— RwLock
// ============================================================================
/// **生产问题**：indexer 对每笔 log 查 `(address, topic0) → decoder`；新合约部署偶尔写。
///
/// **套路**：`RwLock<HashMap<Address, u8>>` 存 decoder id；热路径 read。
pub mod abi_cache_rwlock {
    use super::*;

    pub struct DecoderTable {
        inner: RwLock<HashMap<Address, u8>>,
    }

    impl DecoderTable {
        pub fn new() -> Self {
            Self {
                inner: RwLock::new(HashMap::new()),
            }
        }

        pub fn decoder_id(&self, addr: Address) -> Option<u8> {
            self.inner.read().ok()?.get(&addr).copied()
        }

        pub fn register(&self, addr: Address, id: u8) {
            if let Ok(mut g) = self.inner.write() {
                g.insert(addr, id);
            }
        }
    }

    pub fn demonstrate() {
        println!("## Web3-3：ABI 解码表 RwLock");

        let table = DecoderTable::new();
        let router = [0x01; 20];
        table.register(router, 3);
        println!("router decoder={:?}", table.decoder_id(router));
        println!("升级 / 新池子 write；indexer 热路径 read\n");
    }
}

// ============================================================================
// 场景 4：Reorg 缓冲 —— RefCell pending blocks
// ============================================================================
/// **生产问题**：indexer 在 confirmation 前不能把块写入 DB；reorg 时要 rollback
/// 内存态。handler 接口常为 `&self`。
///
/// **套路**：`RefCell<Vec<PendingBlock>>` 存未确认块；finalize 时 drain。
pub mod reorg_buffer {
    use super::*;

    #[derive(Clone, Debug)]
    pub struct PendingBlock {
        pub num: BlockNum,
        pub hash: TxHash,
    }

    pub struct Indexer {
        pending: RefCell<Vec<PendingBlock>>,
        finalized: Cell<BlockNum>,
    }

    impl Indexer {
        pub fn new() -> Self {
            Self {
                pending: RefCell::new(Vec::new()),
                finalized: Cell::new(0),
            }
        }

        pub fn on_new_head(&self, block: PendingBlock) {
            self.pending.borrow_mut().push(block);
        }

        pub fn finalize_through(&self, num: BlockNum) {
            let mut p = self.pending.borrow_mut();
            p.retain(|b| b.num > num);
            self.finalized.set(num);
        }

        pub fn rollback_to(&self, num: BlockNum) {
            let mut p = self.pending.borrow_mut();
            p.retain(|b| b.num <= num);
        }

        pub fn pending_len(&self) -> usize {
            self.pending.borrow().len()
        }
    }

    pub fn demonstrate() {
        println!("## Web3-4：Reorg 缓冲 RefCell");

        let idx = Indexer::new();
        idx.on_new_head(PendingBlock {
            num: 100,
            hash: [1; 32],
        });
        idx.on_new_head(PendingBlock {
            num: 101,
            hash: [2; 32],
        });
        idx.rollback_to(100);
        println!(
            "reorg 后 pending_len={} finalized={}",
            idx.pending_len(),
            idx.finalized.get()
        );
        println!("单线程 indexer；多 worker 用 channel 串行状态机\n");
    }
}

// ============================================================================
// 场景 5：Gas Oracle —— AtomicU64 共享报价
// ============================================================================
/// **生产问题**：builder / searcher / wallet 同时读 base fee / priority fee；
/// 后台 goroutine 每块更新一次。
///
/// **套路**：`AtomicU64` 存 wei；读者 Relaxed load，写者 store。
pub mod gas_oracle_atomic {
    use super::*;

    pub struct GasOracle {
        base_fee_wei: AtomicU64,
        priority_wei: AtomicU64,
        pub updates: Cell<u64>,
    }

    impl GasOracle {
        pub fn new(base: u64, priority: u64) -> Self {
            Self {
                base_fee_wei: AtomicU64::new(base),
                priority_wei: AtomicU64::new(priority),
                updates: Cell::new(0),
            }
        }

        pub fn publish(&self, base: u64, priority: u64) {
            self.base_fee_wei.store(base, Ordering::Release);
            self.priority_wei.store(priority, Ordering::Release);
            self.updates.set(self.updates.get() + 1);
        }

        pub fn quote(&self) -> (u64, u64) {
            (
                self.base_fee_wei.load(Ordering::Acquire),
                self.priority_wei.load(Ordering::Acquire),
            )
        }
    }

    pub fn demonstrate() {
        println!("## Web3-5：Gas Oracle Atomic");

        let oracle = Arc::new(GasOracle::new(30_000_000_000, 2_000_000_000));
        oracle.publish(35_000_000_000, 3_000_000_000);
        let (b, p) = oracle.quote();
        println!("base={} priority={} updates={}\n", b, p, oracle.updates.get());
    }
}

// ============================================================================
// 场景 6：待签交易池 —— Mutex 多线程 signer
// ============================================================================
/// **生产问题**：RPC 线程 submit、signer 线程 drain、timeout 线程 cancel；
/// 队列有复杂不变式（nonce 顺序、替换规则）。
///
/// **套路**：`Mutex<Vec<PendingTx>>`；临界区尽量短；或 actor channel。
pub mod pending_tx_mutex {
    use super::*;

    #[derive(Clone, Debug)]
    pub struct PendingTx {
        pub hash: TxHash,
        pub nonce: u64,
    }

    pub struct TxPool {
        inner: Mutex<Vec<PendingTx>>,
    }

    impl TxPool {
        pub fn new() -> Self {
            Self {
                inner: Mutex::new(Vec::new()),
            }
        }

        pub fn submit(&self, tx: PendingTx) {
            if let Ok(mut g) = self.inner.lock() {
                g.push(tx);
            }
        }

        pub fn drain_ready(&self, limit: usize) -> Vec<PendingTx> {
            let mut g = self.inner.lock().unwrap();
            let n = g.len().min(limit);
            g.drain(0..n).collect()
        }

        pub fn len(&self) -> usize {
            self.inner.lock().map(|g| g.len()).unwrap_or(0)
        }
    }

    pub fn demonstrate() {
        println!("## Web3-6：待签队列 Mutex");

        let pool = Arc::new(TxPool::new());
        pool.submit(PendingTx {
            hash: [0x01; 32],
            nonce: 1,
        });
        pool.submit(PendingTx {
            hash: [0x02; 32],
            nonce: 2,
        });
        let batch = pool.drain_ready(1);
        println!("drain 1 tx, pool_len={} batch={:?}\n", pool.len(), batch.len());
    }
}

pub fn demonstrate() {
    mempool_dedup::demonstrate();
    nonce_atomic::demonstrate();
    abi_cache_rwlock::demonstrate();
    reorg_buffer::demonstrate();
    gas_oracle_atomic::demonstrate();
    pending_tx_mutex::demonstrate();
}
