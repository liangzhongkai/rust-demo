//! # 泛化：从 HFT/Web3 场景到通用生命周期策略
//!
//! | 问题类型              | 标志特征                          | 首选套路                               |
//! |-----------------------|-----------------------------------|----------------------------------------|
//! | 1. 零拷贝视图         | 解析热路径、wire/frame buffer     | `View<'buf>` 显式 lifetime             |
//! | 2. 批处理同寿         | tick/block/simulation scratch     | Arena / bump；drop 整块                |
//! | 3. 模糊边界           | 借还是 clone 不确定               | `Cow<'a, T>`                           |
//! | 4. 跨线程/async       | channel / spawn / `'static`       | 边界 promote：`Arc` / `Vec` / `String` |
//! | 5. 回调泛型           | 多种输入源、短借                  | HRTB `for<'a> Fn(&'a T)`               |
//! | 6. 配置/符号          | 启动后不变或极少变                | `&'static str` / OnceLock intern       |
//! | 7. 解析与处理分离     | 先 scan 再决策                    | 两阶段：parse scope → owned commit     |
//! | 8. 缓存引用           | 想 cache `&str`                   | 存 owned 或 index；勿 cache 借来的 slice |
//!
//! 下面 8 个策略各有一个 *通用模板*。

#![allow(dead_code)]

use std::borrow::Cow;
use std::sync::Arc;

// ============================================================================
// 策略 1：零拷贝视图 —— 显式 `'buf`
// ============================================================================
/// HFT: hft::fix_zero_copy
/// Web3: web3::tx_calldata_view
pub mod strategy_zero_copy_view {
    pub struct FrameView<'buf> {
        pub header: &'buf [u8],
        pub body: &'buf [u8],
    }

    pub fn split_frame<'buf>(raw: &'buf [u8], header_len: usize) -> Option<FrameView<'buf>> {
        if raw.len() < header_len {
            return None;
        }
        Some(FrameView {
            header: &raw[..header_len],
            body: &raw[header_len..],
        })
    }

    pub fn demonstrate() {
        println!("## 策略 1：零拷贝 View<'buf>");
        let raw = b"HDR:001BODY:payload";
        let v = split_frame(raw, 7).unwrap();
        println!("header={:?} body={:?}\n", v.header, v.body);
    }
}

// ============================================================================
// 策略 2：Arena 批处理 —— 同生共死
// ============================================================================
/// HFT: hft::tick_arena
/// Web3: web3::block_sim_scratch
pub mod strategy_arena_batch {
    use bumpalo::Bump;

    pub fn with_arena<R>(f: impl for<'a> FnOnce(&'a Bump) -> R) -> R {
        let bump = Bump::new();
        f(&bump)
    }

    pub fn demonstrate() {
        println!("## 策略 2：Arena 批处理");
        let sum = with_arena(|bump| {
            let tmp: &mut [i64] = bump.alloc_slice_fill_copy(3, 0);
            tmp.copy_from_slice(&[10, 20, 30]);
            tmp.iter().sum::<i64>()
        });
        println!("scratch sum={}\n", sum);
    }
}

// ============================================================================
// 策略 3：Cow —— 借/拥有统一类型
// ============================================================================
/// HFT: hft::gateway_own_boundary（边界可选 Cow）
/// Web3: web3::abi_cow_cache
pub mod strategy_cow {
    use super::*;

    pub fn normalize_id(input: Cow<'_, [u8]>) -> Cow<'static, [u8]> {
        if input.starts_with(b"TMP-") {
            Cow::Owned(input.into_owned())
        } else {
            Cow::Owned(input.into_owned()) // 演示：生产可保持 Borrowed 若已是 'static
        }
    }

    pub fn demonstrate() {
        println!("## 策略 3：Cow 模糊边界");
        let borrowed: Cow<[u8]> = Cow::Borrowed(b"ORD-123");
        let owned = normalize_id(borrowed);
        println!("normalized len={}\n", owned.len());
    }
}

// ============================================================================
// 策略 4：边界 owned —— 跨线程 / async 前 promote
// ============================================================================
/// HFT: hft::gateway_own_boundary
/// Web3: web3::mempool_promote, web3::async_spawn_static
pub mod strategy_own_boundary {
    use super::*;

    pub struct Message {
        pub bytes: Arc<[u8]>,
    }

    pub fn promote(frame: &[u8]) -> Message {
        Message {
            bytes: Arc::from(frame),
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：边界 promote → Arc/Vec");
        let msg = promote(b"wire");
        println!("Arc len={}\n", msg.bytes.len());
    }
}

// ============================================================================
// 策略 5：HRTB —— 回调接受任意短借
// ============================================================================
/// HFT: hft::hrtb_feed_handler
pub mod strategy_hrtb {
    pub fn apply_twice<T, F>(items: &[T], mut f: F)
    where
        F: for<'a> FnMut(&'a T),
    {
        for item in items {
            f(item);
        }
    }

    pub fn demonstrate() {
        println!("## 策略 5：HRTB for<'a> Fn(&'a T)");
        let data = [1, 2, 3];
        let mut acc = 0;
        apply_twice(&data, |x| acc += x);
        println!("acc={}\n", acc);
    }
}

// ============================================================================
// 策略 6：Static / intern —— 长寿命配置
// ============================================================================
/// HFT: hft::static_symbol_table
/// Web3: web3::abi_cow_cache（Borrowed 分支）
pub mod strategy_static_intern {
    use std::collections::HashMap;

    pub struct Interner {
        storage: Vec<String>,
        index: HashMap<String, usize>,
    }

    impl Interner {
        pub fn new() -> Self {
            Self {
                storage: Vec::new(),
                index: HashMap::new(),
            }
        }

        /// 启动阶段 intern；热路径只比较 id。
        pub fn intern(&mut self, s: &str) -> usize {
            if let Some(&id) = self.index.get(s) {
                return id;
            }
            let id = self.storage.len();
            self.storage.push(s.to_string());
            self.index.insert(s.to_string(), id);
            id
        }

        pub fn resolve(&self, id: usize) -> &str {
            &self.storage[id]
        }
    }

    pub fn demonstrate() {
        println!("## 策略 6：Static / intern");
        let mut interner = Interner::new();
        let a = interner.intern("BTC-USDT");
        let b = interner.intern("BTC-USDT");
        println!("same id={} name={}\n", a == b, interner.resolve(a));
    }
}

// ============================================================================
// 策略 7：两阶段 parse → commit
// ============================================================================
/// HFT: parse NewOrder → GatewayOrder
/// Web3: TxView → PooledTx
pub mod strategy_two_phase {
    pub struct Draft<'buf> {
        pub field: &'buf str,
    }

    pub struct Committed {
        pub field: String,
    }

    pub fn parse<'buf>(raw: &'buf str) -> Draft<'buf> {
        Draft { field: raw }
    }

    pub fn commit(draft: Draft<'_>) -> Committed {
        Committed {
            field: draft.field.to_string(),
        }
    }

    pub fn demonstrate() {
        println!("## 策略 7：两阶段 parse → commit");
        let raw = "pending";
        let draft = parse(raw);
        let committed = commit(draft);
        println!("committed={}\n", committed.field);
    }
}

// ============================================================================
// 策略 8：缓存用 owned / index，不缓存借用
// ============================================================================
/// 陷阱: pitfalls::view_outlives_buffer 的修法
pub mod strategy_cache_owned {
    use std::collections::HashMap;

    pub struct Cache {
        by_key: HashMap<u64, String>,
    }

    impl Cache {
        pub fn new() -> Self {
            Self {
                by_key: HashMap::new(),
            }
        }

        pub fn insert(&mut self, key: u64, value: &str) {
            self.by_key.insert(key, value.to_string());
        }

        pub fn get(&self, key: u64) -> Option<&str> {
            self.by_key.get(&key).map(|s| s.as_str())
        }
    }

    pub fn demonstrate() {
        println!("## 策略 8：缓存存 owned，对外再借");
        let mut c = Cache::new();
        c.insert(1, "hello");
        println!("get={:?}\n", c.get(1));
    }
}

pub fn demonstrate() {
    strategy_zero_copy_view::demonstrate();
    strategy_arena_batch::demonstrate();
    strategy_cow::demonstrate();
    strategy_own_boundary::demonstrate();
    strategy_hrtb::demonstrate();
    strategy_static_intern::demonstrate();
    strategy_two_phase::demonstrate();
    strategy_cache_owned::demonstrate();
}
