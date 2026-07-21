//! # 泛化：从 HFT/Web3 场景到通用 Interior Mutability 策略
//!
//! | 问题类型           | 标志特征                         | 首选套路                              |
//! |--------------------|----------------------------------|---------------------------------------|
//! | 1. Copy 标量       | seq、flags、generation           | Cell（单线程）/ Atomic*（多线程）     |
//! | 2. 单线程 &self    | reactor、回调、trait object      | RefCell；borrow 必须短                |
//! | 3. 多线程复杂状态  | 队列、多字段不变式               | Mutex；临界区最小化                   |
//! | 4. 读多写少        | 配置、ABI 表、decimals           | RwLock / ArcSwap / 世代快照           |
//! | 5. 热路径计数      | metrics、rate limit、nonce       | Atomic + fetch_*                      |
//! | 6. 一次性初始化    | 解码器、静态表                   | OnceLock / lazy_static                  |
//! | 7. 可串行化        | 不必共享内存                     | channel / actor（替代 interior mut）  |
//! | 8. 跨线程共享簿    | 多 worker 写同一结构             | 分片 Mutex 或单 writer + 消息         |
//!
//! 下面 8 个策略各有一个 *通用模板*。

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

// ============================================================================
// 策略 1：Cell —— 单线程 Copy 标量
// ============================================================================
/// HFT: hft::fix_session_cell
/// Web3: web3::gas_oracle_atomic 里的 updates（单线程侧）
pub mod strategy_cell {
    use super::*;
    pub struct SeqGen {
        inner: Cell<u64>,
    }

    impl SeqGen {
        pub fn new() -> Self {
            Self { inner: Cell::new(0) }
        }

        pub fn next(&self) -> u64 {
            let n = self.inner.get();
            self.inner.set(n + 1);
            n
        }
    }

    pub fn demonstrate() {
        println!("## 策略 1：Cell — Copy 标量、单 owner 线程");
        let s = SeqGen::new();
        println!("seq={}\n", s.next());
    }
}

// ============================================================================
// 策略 2：RefCell —— 单线程 &self 突变
// ============================================================================
/// HFT: hft::orderbook_refcell
/// Web3: web3::mempool_dedup
pub mod strategy_refcell {
    use super::*;
    pub struct Slot<T> {
        inner: RefCell<T>,
    }

    impl<T> Slot<T> {
        pub fn new(v: T) -> Self {
            Self {
                inner: RefCell::new(v),
            }
        }

        pub fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
            f(&mut *self.inner.borrow_mut())
        }

        pub fn with_ref<R>(&self, f: impl FnOnce(&T) -> R) -> R {
            f(&*self.inner.borrow())
        }
    }

    pub fn demonstrate() {
        println!("## 策略 2：RefCell — 单线程 &self + 短 borrow");
        let slot = Slot::new(0u64);
        slot.with_mut(|n: &mut u64| *n += 1);
        let v = slot.with_ref(|n| *n);
        println!("value={}\n", v);
    }
}

// ============================================================================
// 策略 3：Atomic —— 多线程无锁标量
// ============================================================================
/// HFT: hft::exposure_atomic, hft::gateway_metrics
/// Web3: web3::nonce_atomic, web3::gas_oracle_atomic
pub mod strategy_atomic {
    use super::*;
    pub struct Counter {
        n: AtomicU64,
    }

    impl Counter {
        pub fn new() -> Self {
            Self {
                n: AtomicU64::new(0),
            }
        }

        pub fn inc(&self) {
            self.n.fetch_add(1, Ordering::Relaxed);
        }

        pub fn get(&self) -> u64 {
            self.n.load(Ordering::Relaxed)
        }
    }

    pub fn demonstrate() {
        println!("## 策略 3：Atomic — 热路径计数/闸门");
        let c = Counter::new();
        c.inc();
        println!("count={}\n", c.get());
    }
}

// ============================================================================
// 策略 4：RwLock —— 读多写少映射表
// ============================================================================
/// HFT: hft::instrument_config_rwlock
/// Web3: web3::abi_cache_rwlock
pub mod strategy_rwlock {
    use super::*;
    use std::collections::HashMap;

    pub struct MapCache<K, V> {
        inner: RwLock<HashMap<K, V>>,
    }

    impl<K: Eq + std::hash::Hash + Copy, V: Copy> MapCache<K, V> {
        pub fn new() -> Self {
            Self {
                inner: RwLock::new(HashMap::new()),
            }
        }

        pub fn get(&self, k: K) -> Option<V> {
            self.inner.read().ok()?.get(&k).copied()
        }

        pub fn insert(&self, k: K, v: V) {
            if let Ok(mut g) = self.inner.write() {
                g.insert(k, v);
            }
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：RwLock — 配置/解码表热读");
        let m: MapCache<u32, u32> = MapCache::new();
        m.insert(1, 100);
        println!("get={:?}\n", m.get(1));
    }
}

// ============================================================================
// 策略 5：Mutex —— 多线程复杂队列
// ============================================================================
/// Web3: web3::pending_tx_mutex
pub mod strategy_mutex {
    use super::*;
    pub struct Queue<T> {
        inner: Mutex<Vec<T>>,
    }

    impl<T> Queue<T> {
        pub fn new() -> Self {
            Self {
                inner: Mutex::new(Vec::new()),
            }
        }

        pub fn push(&self, item: T) {
            if let Ok(mut g) = self.inner.lock() {
                g.push(item);
            }
        }

        pub fn pop(&self) -> Option<T> {
            self.inner.lock().ok()?.pop()
        }
    }

    pub fn demonstrate() {
        println!("## 策略 5：Mutex — 多线程队列/复杂不变式");
        let q: Queue<u64> = Queue::new();
        q.push(42);
        println!("pop={:?}\n", q.pop());
    }
}

// ============================================================================
// 策略 6：OnceLock —— 延迟初始化
// ============================================================================
/// 通用：解码器、venue 插件表首次 touch 时注册
pub mod strategy_once {
    use super::*;
    static THRESHOLD: OnceLock<u64> = OnceLock::new();

    pub fn threshold() -> u64 {
        *THRESHOLD.get_or_init(|| 1_000_000)
    }

    pub fn demonstrate() {
        println!("## 策略 6：OnceLock — 一次性/延迟 init");
        println!("threshold={}\n", threshold());
    }
}

// ============================================================================
// 策略 7：Channel 替代 —— 串行状态机
// ============================================================================
/// 当不变式复杂且多线程时，与其 Arc<Mutex<State>> 不如单 writer task。
pub mod strategy_channel {
    use std::sync::mpsc;

    pub(crate) enum Cmd {
        Inc,
        Get(mpsc::Sender<u64>),
    }

    pub fn spawn_counter() -> mpsc::Sender<Cmd> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut n = 0u64;
            for cmd in rx {
                match cmd {
                    Cmd::Inc => n += 1,
                    Cmd::Get(reply) => {
                        let _ = reply.send(n);
                    }
                }
            }
        });
        tx
    }

    pub fn demonstrate() {
        println!("## 策略 7：Channel — 用消息串行替代共享 Mutex");
        let tx = spawn_counter();
        tx.send(Cmd::Inc).unwrap();
        let (reply_tx, reply_rx) = mpsc::channel();
        tx.send(Cmd::Get(reply_tx)).unwrap();
        println!("via channel n={}\n", reply_rx.recv().unwrap());
    }
}

// ============================================================================
// 策略 8：分片 —— 降低 Mutex 争用
// ============================================================================
/// 多 account / 多 symbol 各一把小锁，好过全局一把。
pub mod strategy_sharding {
    use std::sync::Mutex;

    pub struct ShardedCounters {
        shards: Vec<Mutex<u64>>,
    }

    impl ShardedCounters {
        pub fn new(n: usize) -> Self {
            let mut shards = Vec::with_capacity(n);
            for _ in 0..n {
                shards.push(Mutex::new(0));
            }
            Self { shards }
        }

        fn shard_idx(&self, key: u64) -> usize {
            (key as usize) % self.shards.len()
        }

        pub fn inc(&self, key: u64) {
            if let Ok(mut g) = self.shards[self.shard_idx(key)].lock() {
                *g += 1;
            }
        }

        pub fn total(&self) -> u64 {
            self.shards
                .iter()
                .filter_map(|m| m.lock().ok())
                .map(|g| *g)
                .sum()
        }
    }

    pub fn demonstrate() {
        println!("## 策略 8：分片 Mutex — 按 key 降争用");
        let s = ShardedCounters::new(4);
        s.inc(1);
        s.inc(1);
        s.inc(99);
        println!("total={}\n", s.total());
    }
}

pub fn demonstrate() {
    strategy_cell::demonstrate();
    strategy_refcell::demonstrate();
    strategy_atomic::demonstrate();
    strategy_rwlock::demonstrate();
    strategy_mutex::demonstrate();
    strategy_once::demonstrate();
    strategy_channel::demonstrate();
    strategy_sharding::demonstrate();
}
