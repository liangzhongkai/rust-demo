//! # Web3 生产场景下的 Inline Cache
//!
//! 链上/链下基础设施硬约束：
//! - **热路径重复**：mempool / indexer / searcher 对同一合约、同一 selector 连打
//! - **布局可变**：代理合约升级、ERC20 decimals 初始化后不变但仍要首次 RPC
//! - **正确性**：reorg / 升级后读到旧 layout = 资损级 bug
//!
//! 下面 6 个场景对应 router、indexer、searcher、多链 RPC 客户端。

#![allow(dead_code)]

use std::collections::HashMap;

pub type Address = [u8; 20];
pub type Selector = [u8; 4];
pub type Topic0 = [u8; 32];

// ============================================================================
// 场景 1：ABI selector → 解码器 IC
// ============================================================================
/// **生产问题**：Router / mempool sniffer 每笔 tx 读 4 字节 selector 再
/// `HashMap` 找解码器。热点合约（Uniswap V2/V3 router）selector 高度局部。
///
/// **IC 套路**：`MonoIc { last_sel, decoder_id }`；命中跳过 Map。
pub mod abi_selector_ic {
    use super::*;

    type DecodeFn = fn(&[u8]) -> Option<u64>;

    fn decode_swap(data: &[u8]) -> Option<u64> {
        if data.len() < 4 + 32 {
            return None;
        }
        let mut tail = [0u8; 8];
        tail.copy_from_slice(&data[28..36]);
        Some(u64::from_be_bytes(tail))
    }

    fn decode_approve(data: &[u8]) -> Option<u64> {
        if data.len() < 4 + 64 {
            return None;
        }
        let mut tail = [0u8; 8];
        tail.copy_from_slice(&data[60..68]);
        Some(u64::from_be_bytes(tail))
    }

    pub struct SelectorIc {
        table: HashMap<Selector, DecodeFn>,
        last: Option<(Selector, DecodeFn)>,
        pub hits: u64,
        pub misses: u64,
    }

    impl SelectorIc {
        pub fn new() -> Self {
            let mut table: HashMap<Selector, DecodeFn> = HashMap::new();
            table.insert([0x38, 0xed, 0x17, 0x39], decode_swap); // fake
            table.insert([0x09, 0x5e, 0xa7, 0xb3], decode_approve);
            Self {
                table,
                last: None,
                hits: 0,
                misses: 0,
            }
        }

        pub fn decode(&mut self, calldata: &[u8]) -> Option<u64> {
            if calldata.len() < 4 {
                return None;
            }
            let mut sel = [0u8; 4];
            sel.copy_from_slice(&calldata[..4]);

            let decoder = if let Some((s, f)) = self.last {
                if s == sel {
                    self.hits += 1;
                    f
                } else {
                    self.misses += 1;
                    let f = *self.table.get(&sel)?;
                    self.last = Some((sel, f));
                    f
                }
            } else {
                self.misses += 1;
                let f = *self.table.get(&sel)?;
                self.last = Some((sel, f));
                f
            };
            decoder(calldata)
        }
    }

    pub fn demonstrate() {
        println!("## Web3-1：ABI selector 解码 IC");

        let mut ic = SelectorIc::new();
        let mut calldata = vec![0x38, 0xed, 0x17, 0x39];
        calldata.extend_from_slice(&[0u8; 24]);
        calldata.extend_from_slice(&1_000_000u64.to_be_bytes());

        for _ in 0..25 {
            let _ = ic.decode(&calldata);
        }
        let amount = ic.decode(&calldata);
        println!("swap 连打：hits={} amount={:?}", ic.hits, amount);
        println!("MEV searcher 对同一 router 方法连模拟时收益最大\n");
    }
}

// ============================================================================
// 场景 2：ERC20 decimals / 元数据 IC
// ============================================================================
/// **生产问题**：报价路径每次 `decimals()` 静态调用太贵；decimals 部署后几乎不变。
///
/// **IC 套路**：`(token, decimals)` 单态缓存；未知 token 才走 RPC/表。
pub mod token_meta_ic {
    use super::*;

    #[derive(Clone, Copy)]
    pub struct TokenMeta {
        pub decimals: u8,
        pub layout_gen: u64, // 代理升级世代
    }

    pub struct TokenIc {
        table: HashMap<Address, TokenMeta>,
        last: Option<(Address, TokenMeta)>,
        pub rpc_calls: u64,
        pub hits: u64,
    }

    impl TokenIc {
        pub fn new() -> Self {
            Self {
                table: HashMap::new(),
                last: None,
                rpc_calls: 0,
                hits: 0,
            }
        }

        /// 模拟「查链或本地索引」。
        fn fetch_rpc(&mut self, token: Address) -> TokenMeta {
            self.rpc_calls += 1;
            let meta = TokenMeta {
                decimals: if token[19] == 1 { 6 } else { 18 },
                layout_gen: 1,
            };
            self.table.insert(token, meta);
            meta
        }

        pub fn decimals(&mut self, token: Address) -> u8 {
            if let Some((t, m)) = self.last {
                if t == token {
                    self.hits += 1;
                    return m.decimals;
                }
            }
            let meta = match self.table.get(&token) {
                Some(m) => *m,
                None => self.fetch_rpc(token),
            };
            self.last = Some((token, meta));
            meta.decimals
        }

        /// 代理合约升级：bump layout_gen 并清 IC。
        pub fn on_upgrade(&mut self, token: Address) {
            if let Some(m) = self.table.get_mut(&token) {
                m.layout_gen += 1;
            }
            if matches!(self.last, Some((t, _)) if t == token) {
                self.last = None;
            }
        }
    }

    pub fn demonstrate() {
        println!("## Web3-2：ERC20 decimals IC（免重复 RPC）");

        let mut ic = TokenIc::new();
        let usdc = {
            let mut a = [0u8; 20];
            a[19] = 1;
            a
        };
        for _ in 0..10 {
            let _ = ic.decimals(usdc);
        }
        println!(
            "decimals={} hits={} rpc_calls={}（应为 1 次 RPC）\n",
            ic.decimals(usdc),
            ic.hits,
            ic.rpc_calls
        );
    }
}

// ============================================================================
// 场景 3：合约 Storage Layout / Shape IC
// ============================================================================
/// **生产问题**：读槽位前要知道「实现合约的 storage layout 版本」。
/// 透明代理下 `proxy` 地址不变，`implementation` 会变。
///
/// **IC 套路**：guard = `(proxy, impl_hash/gen)`；命中则用缓存的 slot 偏移表。
pub mod storage_layout_ic {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct LayoutGuard {
        pub proxy: Address,
        pub impl_gen: u64,
    }

    pub struct LayoutIc {
        guard: Option<LayoutGuard>,
        /// 缓存：balanceOf mapping 的 slot 基址（示意）
        balance_slot: u64,
        pub hits: u64,
        pub misses: u64,
    }

    impl LayoutIc {
        pub fn new() -> Self {
            Self {
                guard: None,
                balance_slot: 0,
                hits: 0,
                misses: 0,
            }
        }

        fn resolve(gen: u64) -> u64 {
            // 不同实现版本 slot 布局不同
            if gen == 1 {
                0
            } else {
                2
            }
        }

        pub fn balance_slot(&mut self, g: LayoutGuard) -> u64 {
            if self.guard == Some(g) {
                self.hits += 1;
                return self.balance_slot;
            }
            self.misses += 1;
            self.balance_slot = Self::resolve(g.impl_gen);
            self.guard = Some(g);
            self.balance_slot
        }
    }

    pub fn demonstrate() {
        println!("## Web3-3：代理合约 storage layout IC");

        let proxy = [0x11; 20];
        let mut ic = LayoutIc::new();
        let g1 = LayoutGuard {
            proxy,
            impl_gen: 1,
        };
        for _ in 0..8 {
            let _ = ic.balance_slot(g1);
        }
        let g2 = LayoutGuard {
            proxy,
            impl_gen: 2,
        };
        println!(
            "升级前 slot={} 升级后 slot={} hits={} misses={}\n",
            ic.balance_slot(g1),
            ic.balance_slot(g2),
            ic.hits,
            ic.misses
        );
    }
}

// ============================================================================
// 场景 4：Event topic0 → 解析器 IC
// ============================================================================
/// **生产问题**：indexer 扫日志，`topic0` → 事件类型；块内常重复同一事件
/// （Transfer 刷屏）。每条日志 HashMap 查找浪费。
///
/// **IC 套路**：单态 / 2-slot PIC 缓存最近 topic0 的 parser。
pub mod event_topic_ic {
    use super::*;

    type ParseFn = fn(&[u8]) -> u64;

    fn parse_transfer(_data: &[u8]) -> u64 {
        1
    }
    fn parse_approval(_data: &[u8]) -> u64 {
        2
    }
    fn parse_swap(_data: &[u8]) -> u64 {
        3
    }

    pub struct TopicIc {
        table: HashMap<Topic0, ParseFn>,
        last: Option<(Topic0, ParseFn)>,
        pub hits: u64,
        pub misses: u64,
    }

    impl TopicIc {
        pub fn new() -> Self {
            let mut table: HashMap<Topic0, ParseFn> = HashMap::new();
            let mut t_transfer = [0u8; 32];
            t_transfer[0] = 0xdd;
            let mut t_approval = [0u8; 32];
            t_approval[0] = 0x8c;
            let mut t_swap = [0u8; 32];
            t_swap[0] = 0xc4;
            table.insert(t_transfer, parse_transfer);
            table.insert(t_approval, parse_approval);
            table.insert(t_swap, parse_swap);
            Self {
                table,
                last: None,
                hits: 0,
                misses: 0,
            }
        }

        pub fn classify(&mut self, topic0: Topic0) -> Option<u64> {
            let parser = if let Some((t, f)) = self.last {
                if t == topic0 {
                    self.hits += 1;
                    f
                } else {
                    self.misses += 1;
                    let f = *self.table.get(&topic0)?;
                    self.last = Some((topic0, f));
                    f
                }
            } else {
                self.misses += 1;
                let f = *self.table.get(&topic0)?;
                self.last = Some((topic0, f));
                f
            };
            Some(parser(&[]))
        }
    }

    pub fn demonstrate() {
        println!("## Web3-4：Event topic0 解析 IC");

        let mut ic = TopicIc::new();
        let mut transfer = [0u8; 32];
        transfer[0] = 0xdd;
        for _ in 0..100 {
            let _ = ic.classify(transfer);
        }
        let kind = ic.classify(transfer);
        println!("Transfer 刷屏：hits={} kind={:?}\n", ic.hits, kind);
    }
}

// ============================================================================
// 场景 5：eth_call 目标合约「快捷路径」IC
// ============================================================================
/// **生产问题**：模拟器对同一 pool 反复 `eth_call` 报价；每次解析 to+data
/// 选执行后端（EVM解释 / 预编译快捷路径）。
///
/// **IC 套路**：缓存 `(to, sel) → backend_id`；命中走专用报价器。
pub mod eth_call_path_ic {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub struct CallKey {
        pub to: Address,
        pub sel: Selector,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Backend {
        GenericEvm,
        UniV2Quoter,
        UniV3Quoter,
    }

    pub struct CallPathIc {
        last: Option<(CallKey, Backend)>,
        pub hits: u64,
        pub misses: u64,
    }

    impl CallPathIc {
        pub fn new() -> Self {
            Self {
                last: None,
                hits: 0,
                misses: 0,
            }
        }

        fn resolve(key: CallKey) -> Backend {
            if key.to[0] == 0x02 {
                Backend::UniV2Quoter
            } else if key.to[0] == 0x03 {
                Backend::UniV3Quoter
            } else {
                Backend::GenericEvm
            }
        }

        pub fn backend(&mut self, key: CallKey) -> Backend {
            if let Some((k, b)) = self.last {
                if k == key {
                    self.hits += 1;
                    return b;
                }
            }
            self.misses += 1;
            let b = Self::resolve(key);
            self.last = Some((key, b));
            b
        }
    }

    pub fn demonstrate() {
        println!("## Web3-5：eth_call 快捷路径 IC");

        let mut ic = CallPathIc::new();
        let key = CallKey {
            to: {
                let mut a = [0u8; 20];
                a[0] = 0x02;
                a
            },
            sel: [0x01, 0x02, 0x03, 0x04],
        };
        for _ in 0..15 {
            let _ = ic.backend(key);
        }
        println!(
            "backend={:?} hits={}（专用 quoter 跳过通用 EVM）\n",
            ic.backend(key),
            ic.hits
        );
    }
}

// ============================================================================
// 场景 6：多链 RPC 客户端 Handler IC
// ============================================================================
/// **生产问题**：同一进程连 ETH / Arb / Base；序列化、链 ID、gas 模型不同。
/// 请求序列常粘滞在一条链上（扫块、跟单）。
///
/// **IC 套路**：缓存 `chain_id → codec/handler`；切链才 miss。
pub mod multichain_rpc_ic {
    pub type EncodeFn = fn(u64) -> u64;

    fn encode_eth(nonce: u64) -> u64 {
        nonce
    }
    fn encode_l2(nonce: u64) -> u64 {
        nonce.wrapping_add(1_000_000) // 示意：L2 额外域
    }

    pub struct ChainIc {
        last_chain: Option<u64>,
        encode: EncodeFn,
        pub hits: u64,
        pub misses: u64,
    }

    impl ChainIc {
        pub fn new() -> Self {
            Self {
                last_chain: None,
                encode: encode_eth,
                hits: 0,
                misses: 0,
            }
        }

        pub fn encode_tx(&mut self, chain_id: u64, nonce: u64) -> Option<u64> {
            if self.last_chain == Some(chain_id) {
                self.hits += 1;
                return Some((self.encode)(nonce));
            }
            self.misses += 1;
            let f: EncodeFn = match chain_id {
                1 => encode_eth,
                42161 | 8453 => encode_l2,
                _ => return None,
            };
            self.last_chain = Some(chain_id);
            self.encode = f;
            Some(f(nonce))
        }
    }

    pub fn demonstrate() {
        println!("## Web3-6：多链 RPC codec IC");

        let mut ic = ChainIc::new();
        for _ in 0..20 {
            let _ = ic.encode_tx(42161, 7);
        }
        let encoded = ic.encode_tx(42161, 7);
        println!("Arb 粘滞：hits={} encoded={:?}\n", ic.hits, encoded);
    }
}

pub fn demonstrate() {
    abi_selector_ic::demonstrate();
    token_meta_ic::demonstrate();
    storage_layout_ic::demonstrate();
    event_topic_ic::demonstrate();
    eth_call_path_ic::demonstrate();
    multichain_rpc_ic::demonstrate();
}
