//! # Copy vs Clone 深度实践
//!
//! 考察点：
//! 1. **Copy**（隐式按位复制）与 **Clone**（显式深拷贝）的语义与性能边界
//! 2. HFT/Web3 中「热路径误 clone」「非 Copy 进无锁队列」「Copy 掩盖 stale 状态」等生产事故
//! 3. 在 API 边界用 Move/Arc/Cow，在内核用热路径 Copy + 固定宽度类型
//! 4. 从具体场景泛化到一般性应对策略
//!
//! 运行：`cargo run -p copy-vs-clone`

#![allow(dead_code)]

// =============================================================================
// 第一部分：HFT 生产场景
// =============================================================================

mod hft {
    // -------------------------------------------------------------------------
    // 场景 1：Quote tick —— 撮合热路径只用 Copy
    // -------------------------------------------------------------------------
    /// **生产问题**：行情更新循环里对 `Quote` 调用 `.clone()`，
    /// 若 Quote 内含 `String` 或 `Vec`，每次 tick 触发堆分配 → P99 延迟尖刺。
    pub mod quote_tick_copy {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Quote {
            pub price_ticks: i64,
            pub qty: u32,
            pub seq: u64,
        }

        /// ✅ 热路径：Copy 隐式复制，栈上若干字节，无堆
        pub fn match_best_bid_ask(bid: Quote, ask: Quote) -> bool {
            bid.price_ticks >= ask.price_ticks && bid.qty > 0 && ask.qty > 0
        }

        /// ❌ 反模式：把 Copy 类型当 Clone 用 —— 语义相同但显式 clone 误导维护者
        pub fn match_with_redundant_clone(bid: Quote, ask: Quote) -> bool {
            let b = bid.clone(); // 多余：Quote 是 Copy，直接赋值即可
            let a = ask.clone();
            b.price_ticks >= a.price_ticks
        }

        pub fn demonstrate() {
            println!("## HFT-1：Quote tick（热路径 Copy，禁堆分配）");
            let bid = Quote {
                price_ticks: 100_05,
                qty: 500,
                seq: 1,
            };
            let ask = Quote {
                price_ticks: 100_04,
                qty: 300,
                seq: 2,
            };
            // Copy：传参时按位复制，原变量仍可用
            assert!(match_best_bid_ask(bid, ask));
            assert_eq!(bid.seq, 1);
            println!("  bid/ask 为 Copy → 传参零堆分配，seq 仍可读");
            println!("  策略：价格/数量/序号用固定宽度整数 + Copy newtype\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 2：订单路由 —— 入口 Move，热路径 &str / Arc<str>
    // -------------------------------------------------------------------------
    /// **生产问题**：FIX 网关每条消息 `client_id.clone()` 进路由表，
    /// 10 万 msg/s × 32B String ≈ 3.2 MB/s 堆 churn，GC 压力下 P99 恶化。
    pub mod order_routing_clone_trap {
        use std::sync::Arc;

        pub struct Order {
            pub client_id: String,
            pub qty: u64,
        }

        /// ❌ 反模式：热路径 clone client_id
        pub fn route_lossy(order: &Order, routes: &mut Vec<(String, u64)>) {
            routes.push((order.client_id.clone(), order.qty)); // 每条消息堆分配
        }

        /// ✅ 入口 Move 一次，路由表存 Arc<str> 共享
        pub fn ingest_order(order: Order) -> (Arc<str>, u64) {
            (Arc::from(order.client_id.as_str()), order.qty)
        }

        pub fn route_shared(client: Arc<str>, qty: u64, routes: &mut Vec<(Arc<str>, u64)>) {
            routes.push((client, qty)); // Arc clone = 原子计数，无堆复制字符串
        }

        pub fn demonstrate() {
            println!("## HFT-2：订单路由（入口 Move / Arc，热路径禁 String clone）");
            let order = Order {
                client_id: "DESK-A".into(),
                qty: 1000,
            };
            let (client, qty) = ingest_order(order);
            let mut routes = Vec::new();
            route_shared(Arc::clone(&client), qty, &mut routes);
            route_shared(client, 500, &mut routes);
            assert_eq!(routes.len(), 2);
            println!("  入口：Order Move → Arc<str> 一次转换");
            println!("  热路径：Arc::clone 仅增引用计数");
            println!("  策略：String 只在边界出现一次；内核 &str / Arc<str>\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 3：SPSC 环形队列 —— 无锁结构要求 T: Copy
    // -------------------------------------------------------------------------
    /// **生产问题**：把 `String` 或带 `Vec` 的结构塞进 lock-free ring buffer，
    /// 编译失败或被迫 Box 化 → 指针追逐、ABA 风险、延迟不可控。
    pub mod spsc_ring_copy_only {
        /// 简化 SPSC：仅演示 Copy 约束
        pub struct RingBuffer<T: Copy, const N: usize> {
            buf: [T; N],
            head: usize,
            tail: usize,
        }

        impl<T: Copy, const N: usize> RingBuffer<T, N> {
            pub fn new(default: T) -> Self {
                Self {
                    buf: [default; N],
                    head: 0,
                    tail: 0,
                }
            }

            pub fn push(&mut self, v: T) -> bool {
                let next = (self.head + 1) % N;
                if next == self.tail {
                    return false;
                }
                self.buf[self.head] = v; // Copy 写入，无 Drop 竞争
                self.head = next;
                true
            }

            pub fn pop(&mut self) -> Option<T> {
                if self.head == self.tail {
                    return None;
                }
                let v = self.buf[self.tail]; // Copy 读出
                self.tail = (self.tail + 1) % N;
                Some(v)
            }
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct MarketEvent {
            pub price_ticks: i64,
            pub qty: u32,
        }

        pub fn demonstrate() {
            println!("## HFT-3：SPSC 环形队列（T: Copy 约束）");
            let mut ring = RingBuffer::<MarketEvent, 4>::new(MarketEvent {
                price_ticks: 0,
                qty: 0,
            });
            ring.push(MarketEvent {
                price_ticks: 100,
                qty: 10,
            });
            let ev = ring.pop().unwrap();
            assert_eq!(ev.price_ticks, 100);
            println!("  RingBuffer<T> 要求 T: Copy → 栈上按位搬运");
            println!("  非 Copy 类型：用 index/offset 进队列，堆上对象池统一管理");
            println!("  策略：无锁通道 payload 必须是 Copy 或整数 handle\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 4：OrderId Copy —— 去重与序号窗口
    // -------------------------------------------------------------------------
    /// **生产问题**：用 `String` 做 clOrdID 去重，HashSet 每次 insert clone；
    /// 改用 Copy 的 u64 OrderId，热路径零堆。
    pub mod order_id_copy_dedup {
        use std::collections::HashSet;

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct OrderId(pub u64);

        pub struct DedupGate {
            seen: HashSet<OrderId>,
        }

        impl DedupGate {
            pub fn new() -> Self {
                Self {
                    seen: HashSet::new(),
                }
            }

            /// id 为 Copy：insert 按位复制进 HashSet
            pub fn accept(&mut self, id: OrderId) -> bool {
                self.seen.insert(id)
            }
        }

        pub fn demonstrate() {
            println!("## HFT-4：OrderId Copy 去重");
            let mut gate = DedupGate::new();
            let id = OrderId(42_001);
            assert!(gate.accept(id));
            assert!(!gate.accept(id)); // Copy 后再 insert，语义正确
            assert_eq!(id.0, 42_001); // 原 id 仍可用
            println!("  Copy newtype → HashSet insert 无堆分配");
            println!("  策略：wire 解析出 u64 后转 OrderId，内核不再碰 String\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 5：深度快照 —— Level Copy，整本 Clone
    // -------------------------------------------------------------------------
    /// **生产问题**：风控每秒 snapshot 全量 depth book；
    /// 若 Level 含 String 则 clone 全书极慢；Level 用 Copy，仅 Vec<Level> clone。
    pub mod depth_snapshot {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Level {
            pub price_ticks: i64,
            pub qty: u64,
        }

        #[derive(Debug, Clone)]
        pub struct DepthBook {
            pub bids: Vec<Level>,
            pub asks: Vec<Level>,
        }

        impl DepthBook {
            pub fn top_of_book(&self) -> (Option<Level>, Option<Level>) {
                (self.bids.first().copied(), self.asks.first().copied())
                // Level: Copy → .copied() 从 &Level 得到 Level 值拷贝
            }

            pub fn snapshot(&self) -> Self {
                self.clone() // 显式深拷贝 Vec；Level 按位复制
            }
        }

        pub fn demonstrate() {
            println!("## HFT-5：深度快照（Level Copy，Vec Clone）");
            let book = DepthBook {
                bids: vec![Level {
                    price_ticks: 100,
                    qty: 50,
                }],
                asks: vec![Level {
                    price_ticks: 101,
                    qty: 30,
                }],
            };
            let (bid, _ask) = book.top_of_book();
            assert_eq!(bid.unwrap().price_ticks, 100);
            let snap = book.snapshot();
            assert_eq!(snap.bids.len(), 1);
            println!("  Level: Copy → top_of_book 无堆");
            println!("  DepthBook: Clone → 风控快照显式 clone 整 Vec");
            println!("  策略：热路径读 Level；冷路径 snapshot 才 Clone 全书\n");
        }
    }

    pub fn run_all() {
        quote_tick_copy::demonstrate();
        order_routing_clone_trap::demonstrate();
        spsc_ring_copy_only::demonstrate();
        order_id_copy_dedup::demonstrate();
        depth_snapshot::demonstrate();
    }
}

// =============================================================================
// 第二部分：Web3 生产场景
// =============================================================================

mod web3 {
    // -------------------------------------------------------------------------
    // 场景 1：交易字段 —— ChainId / Nonce / GasPrice Copy
    // -------------------------------------------------------------------------
    /// **生产问题**：构建交易时对 `chain_id`、`nonce` 误用 clone 或包在 Arc 里，
    /// 增加无谓开销；这些字段本质是 Copy 标量。
    pub mod tx_fields_copy {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct ChainId(pub u64);

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct TxHeader {
            pub chain_id: ChainId,
            pub nonce: u64,
            pub gas_price_wei: u128,
        }

        pub fn sign_payload(header: TxHeader) -> [u8; 32] {
            // 模拟：Copy 传参，栈上组合
            let mut buf = [0u8; 32];
            buf[..8].copy_from_slice(&header.nonce.to_be_bytes());
            buf[8..16].copy_from_slice(&header.chain_id.0.to_be_bytes());
            buf
        }

        pub fn demonstrate() {
            println!("## Web3-1：TxHeader Copy（chain_id / nonce / gas）");
            let header = TxHeader {
                chain_id: ChainId(1),
                nonce: 42,
                gas_price_wei: 20_000_000_000,
            };
            let sig_input = sign_payload(header);
            assert_ne!(sig_input, [0u8; 32]);
            assert_eq!(header.nonce, 42); // Copy 后原 header 仍有效
            println!("  TxHeader: Copy → 签名路径无堆");
            println!("  策略：链上标量字段一律 Copy newtype\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 2：Calldata —— selector Copy，payload 边界 Clone 一次
    // -------------------------------------------------------------------------
    /// **生产问题**：每次 encode 都对 4 字节 selector 做 `Vec` 分配；
    /// selector 用 `[u8; 4]` Copy，仅 args 用 Vec，在 RPC 边界 clone 一次。
    pub mod calldata_copy_vs_clone {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Selector(pub [u8; 4]);

        pub fn encode_call(selector: Selector, args: &[u8]) -> Vec<u8> {
            let mut out = Vec::with_capacity(4 + args.len());
            out.extend_from_slice(&selector.0); // Copy array
            out.extend_from_slice(args);
            out
        }

        /// RPC 边界：此处 Clone 一次 calldata 交给 HTTP 层，可接受
        pub fn broadcast(calldata: Vec<u8>) -> usize {
            calldata.len() // 模拟发送：Move 进网络栈
        }

        pub fn demonstrate() {
            println!("## Web3-2：Calldata（selector Copy，payload 边界 Clone）");
            let sel = Selector([0xa9, 0x05, 0x9c, 0xbb]);
            let mut args = [0u8; 32];
            args[31] = 1;
            let calldata = encode_call(sel, &args);
            assert_eq!(calldata.len(), 36);
            let _ = broadcast(calldata);
            println!("  Selector [u8;4]: Copy → encode 无额外分配");
            println!("  Vec calldata: 构造一次，broadcast Move 出去");
            println!("  策略：固定宽度 Copy；变长 payload 只 clone/Move 一次过边界\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 3：Log topic —— [u8;32] Copy 比较，解码 String 放冷路径
    // -------------------------------------------------------------------------
    /// **生产问题**：监听 Transfer 事件时在热路径 `topic.to_string()` 再比较，
    /// 每秒数千 log × 66 字符 hex 分配 → indexer lag。
    pub mod log_topic_copy {
        pub const TRANSFER_TOPIC: [u8; 32] = [0u8; 32]; // 示意：真实为 keccak256

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Topic(pub [u8; 32]);

        pub fn is_transfer_fast(topic: Topic) -> bool {
            topic.0 == TRANSFER_TOPIC // Copy 比较，无堆
        }

        /// ❌ 冷路径才需要：给 UI / 日志
        pub fn topic_to_hex(topic: Topic) -> String {
            topic.0.iter().map(|b| format!("{:02x}", b)).collect()
        }

        pub fn demonstrate() {
            println!("## Web3-3：Log topic（[u8;32] Copy 过滤）");
            let topic = Topic(TRANSFER_TOPIC);
            assert!(is_transfer_fast(topic));
            let hex = topic_to_hex(topic);
            assert_eq!(hex.len(), 64);
            println!("  热路径：Topic Copy + 常量比较");
            println!("  冷路径：topic_to_hex 才分配 String");
            println!("  策略：filter 用字节 Copy；decode 推迟到确认匹配后\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 4：共享配置 —— Arc 替代反复 Clone
    // -------------------------------------------------------------------------
    /// **生产问题**：每个 worker `config.clone()` 复制整份 RPC endpoints Vec，
    /// N 个 tokio task × M 个 URL → 内存膨胀；应 `Arc<Config>` 共享。
    pub mod shared_config_arc {
        use std::sync::Arc;

        #[derive(Debug, Clone)]
        pub struct RpcConfig {
            pub endpoints: Vec<String>,
            pub chain_id: u64,
        }

        pub struct Indexer {
            config: Arc<RpcConfig>,
        }

        impl Indexer {
            pub fn new(config: Arc<RpcConfig>) -> Self {
                Self { config }
            }

            pub fn endpoint_count(&self) -> usize {
                self.config.endpoints.len() // 无 clone config
            }
        }

        pub fn spawn_workers(config: RpcConfig, n: usize) -> Vec<Indexer> {
            let shared = Arc::new(config);
            (0..n).map(|_| Indexer::new(Arc::clone(&shared))).collect()
        }

        pub fn demonstrate() {
            println!("## Web3-4：RpcConfig Arc 共享（禁每 worker Clone）");
            let cfg = RpcConfig {
                endpoints: vec!["https://eth.llamarpc.com".into()],
                chain_id: 1,
            };
            let workers = spawn_workers(cfg, 4);
            assert_eq!(workers.len(), 4);
            assert_eq!(workers[0].endpoint_count(), 1);
            println!("  入口：RpcConfig 构造一次 → Arc");
            println!("  worker：Arc::clone 共享 endpoints，无 Vec 深拷贝");
            println!("  策略：不可变配置 Arc；可变状态 Mutex/Channel\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 5：MEV bundle —— 模拟路径 Copy，提交路径 Clone 一次
    // -------------------------------------------------------------------------
    /// **生产问题**：模拟器对每个 candidate tx 深拷贝 calldata，
    /// 100 次模拟 × 2KB calldata = 200KB/轮；模拟用 `&[u8]` 借用，提交才 clone。
    pub mod mev_simulate_borrow {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct GasLimit(pub u64);

        pub fn simulate_tx(calldata: &[u8], gas: GasLimit) -> bool {
            calldata.len() < 10_000 && gas.0 <= 30_000_000
        }

        pub fn submit_bundle(txs: Vec<Vec<u8>>) -> usize {
            txs.iter().map(|t| t.len()).sum()
        }

        pub fn build_and_submit(calldata: &[u8], gas: GasLimit) -> Option<usize> {
            if simulate_tx(calldata, gas) {
                // 仅提交路径 clone 一次
                Some(submit_bundle(vec![calldata.to_vec()]))
            } else {
                None
            }
        }

        pub fn demonstrate() {
            println!("## Web3-5：MEV 模拟（simulate 借用，submit 才 clone）");
            let calldata = vec![0u8; 512];
            let gas = GasLimit(21_000);
            assert!(simulate_tx(&calldata, gas));
            let bytes = build_and_submit(&calldata, gas).unwrap();
            assert_eq!(bytes, 512);
            println!("  simulate：&[u8] 借用，零拷贝");
            println!("  submit：to_vec() 一次，过 RPC 边界");
            println!("  策略：热循环 borrow；仅在 escape 点 Clone/Move\n");
        }
    }

    pub fn run_all() {
        tx_fields_copy::demonstrate();
        calldata_copy_vs_clone::demonstrate();
        log_topic_copy::demonstrate();
        shared_config_arc::demonstrate();
        mev_simulate_borrow::demonstrate();
    }
}

// =============================================================================
// 第三部分：泛化策略
// =============================================================================

mod generalized {
    // -------------------------------------------------------------------------
    // 泛化 1：Copy vs Clone 语义对照
    // -------------------------------------------------------------------------
    pub mod copy_vs_clone_semantics {
        #[derive(Clone, Copy, Debug)]
        pub struct Counter(i32);

        pub fn demonstrate() {
            println!("## 泛化-1：Copy vs Clone 语义");
            let c = Counter(1);
            let c2 = c; // Copy：c 仍有效
            assert_eq!(c.0, 1);
            assert_eq!(c2.0, 1);

            let s = String::from("hello");
            let s2 = s.clone(); // Clone：显式深拷贝
            assert_eq!(s, "hello");
            assert_eq!(s2, "hello");

            println!("  Copy：赋值/传参后原变量仍可用（按位复制）");
            println!("  Clone：必须显式 .clone()，可能堆分配");
            println!("  Move：非 Copy 类型赋值后原变量失效\n");
        }
    }

    // -------------------------------------------------------------------------
    // 泛化 2：Copy 陷阱 —— 掩盖「本应 Move」的语义
    // -------------------------------------------------------------------------
    pub mod copy_stale_trap {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct Slot {
            pub seq: u64,
            pub value: i64,
        }

        /// ❌ Copy 使旧副本仍「合法」，容易误用过期 seq
        pub fn update_slot_wrong(slot: Slot, new_value: i64) -> (Slot, Slot) {
            let mut s = slot;
            s.value = new_value;
            s.seq += 1;
            (slot, s) // slot 仍是旧 seq！Copy 未失效
        }

        pub fn demonstrate() {
            println!("## 泛化-2：Copy 陷阱（stale 副本仍合法）");
            let slot = Slot { seq: 1, value: 100 };
            let (stale, fresh) = update_slot_wrong(slot, 200);
            assert_eq!(stale.seq, 1);
            assert_eq!(fresh.seq, 2);
            println!("  Copy 类型：旧副本 stale 但编译器不阻止使用");
            println!("  应对：状态机用 !Copy + Move；或 seq 校验拒绝旧副本\n");
        }
    }

    // -------------------------------------------------------------------------
    // 泛化 3：Cow —— 延迟 Clone
    // -------------------------------------------------------------------------
    pub mod cow_lazy_clone {
        use std::borrow::Cow;

        pub fn normalize_symbol(input: &str) -> Cow<'_, str> {
            if input.chars().all(|c| c.is_ascii_uppercase()) {
                Cow::Borrowed(input)
            } else {
                Cow::Owned(input.to_ascii_uppercase())
            }
        }

        pub fn demonstrate() {
            println!("## 泛化-3：Cow 延迟 Clone");
            let a = normalize_symbol("ETH");
            let b = normalize_symbol("eth");
            assert!(matches!(a, Cow::Borrowed(_)));
            assert!(matches!(b, Cow::Owned(_)));
            println!("  已满足条件 → Borrowed，零分配");
            println!("  需变换 → Owned，仅此时 clone");
            println!("  策略：API 返回 Cow<'a, T>，调用方决定要不要拥有\n");
        }
    }

    // -------------------------------------------------------------------------
    // 泛化 4：决策矩阵
    // -------------------------------------------------------------------------
    pub fn print_playbook() {
        println!("## 泛化-4：Copy / Clone 决策手册");
        println!("  ┌────────────────────┬──────────────────────────────────────────┐");
        println!("  │ 场景               │ 推荐做法                                  │");
        println!("  ├────────────────────┼──────────────────────────────────────────┤");
        println!("  │ 价格/数量/序号     │ Copy newtype（i64/u64）                   │");
        println!("  │ 枚举/小数组        │ Copy（#[derive(Copy,Clone)]）             │");
        println!("  │ 无锁队列 payload   │ T: Copy 或 integer handle                 │");
        println!("  │ 字符串标识         │ 入口 parse → Arc<str>；禁热路径 clone     │");
        println!("  │ 不可变配置         │ Arc<Config> 共享                          │");
        println!("  │ 变长 byte payload  │ 热路径 &[u8]；边界 to_vec/Clone 一次      │");
        println!("  │ 整集合快照         │ 冷路径显式 .clone()，文档化成本           │");
        println!("  │ 可能借用也可能拥有 │ Cow<'a, T>                                │");
        println!("  │ 带 Drop 资源       │ 不可 Copy；用 Move 或 Arc/Mutex           │");
        println!("  └────────────────────┴──────────────────────────────────────────┘");
        println!();
        println!("  HFT 典型事故链：热路径 String clone → 堆 churn → P99 延迟 → 漏单");
        println!("  Web3 典型事故链：模拟循环 to_vec → 带宽/CPU 浪费 → indexer lag → 错失 MEV");
        println!("  共同原则：Copy 是「免费复制」；Clone 必须显式且计成本；Move 是默认所有权转移");
        println!("  口诀：内核 Copy + 整数，边界 Move 一次，共享 Arc，可变才 Clone");
    }

    // -------------------------------------------------------------------------
    // 泛化 5：Copy trait 成立条件
    // -------------------------------------------------------------------------
    pub mod copy_trait_rules {
        pub fn demonstrate() {
            println!("## 泛化-5：Copy trait 成立条件");
            println!("  1. 所有字段都是 Copy（标量、Copy struct、引用）");
            println!("  2. 无自定义 Drop — Drop 类型不可 Copy");
            println!("  3. Copy 是 marker trait，与 Clone 配对 derive");
            println!("  4. 不可 Copy 时：优先 Move；需共享用 Arc；需拷贝用 Cow");
            println!("  5. 不要为含 String/Vec 的类型强 derive Copy — 编译器会拒绝\n");
        }
    }

    // -------------------------------------------------------------------------
    // 泛化 6：Arc clone vs data clone
    // -------------------------------------------------------------------------
    pub mod arc_vs_data_clone {
        use std::sync::Arc;

        pub fn demonstrate() {
            println!("## 泛化-6：Arc::clone vs data clone");
            let data = vec![1u8; 1024];
            let arc1 = Arc::new(data);
            let arc2 = Arc::clone(&arc1); // 原子 +1，不复制 1024 字节
            assert_eq!(Arc::strong_count(&arc1), 2);
            assert!(Arc::ptr_eq(&arc1, &arc2));
            println!("  Arc::clone：共享堆数据，O(1)");
            println!("  Vec::clone：深拷贝，O(n)");
            println!("  策略：多读者同一份不可变数据 → Arc；各需独立修改 → 各自 Clone\n");
        }
    }

    pub fn run_all() {
        copy_vs_clone_semantics::demonstrate();
        copy_stale_trap::demonstrate();
        cow_lazy_clone::demonstrate();
        copy_trait_rules::demonstrate();
        arc_vs_data_clone::demonstrate();
        print_playbook();
    }
}

fn main() {
    println!("=== Rust Copy vs Clone：HFT / Web3 生产场景 ===\n");

    println!("--- 第一部分：HFT ---\n");
    hft::run_all();

    println!("--- 第二部分：Web3 ---\n");
    web3::run_all();

    println!("--- 第三部分：泛化策略 ---\n");
    generalized::run_all();

    println!("=== 全部示例运行完毕 ===");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hft_quote_copy_preserves_original() {
        let q = hft::quote_tick_copy::Quote {
            price_ticks: 1,
            qty: 1,
            seq: 99,
        };
        let _ = hft::quote_tick_copy::match_best_bid_ask(
            q,
            hft::quote_tick_copy::Quote {
                price_ticks: 1,
                qty: 1,
                seq: 2,
            },
        );
        assert_eq!(q.seq, 99);
    }

    #[test]
    fn hft_dedup_rejects_duplicate() {
        let mut gate = hft::order_id_copy_dedup::DedupGate::new();
        let id = hft::order_id_copy_dedup::OrderId(1);
        assert!(gate.accept(id));
        assert!(!gate.accept(id));
    }

    #[test]
    fn hft_ring_buffer_copy() {
        let mut ring = hft::spsc_ring_copy_only::RingBuffer::<
            hft::spsc_ring_copy_only::MarketEvent,
            2,
        >::new(hft::spsc_ring_copy_only::MarketEvent {
            price_ticks: 0,
            qty: 0,
        });
        assert!(ring.push(hft::spsc_ring_copy_only::MarketEvent {
            price_ticks: 1,
            qty: 1,
        }));
        assert!(ring.pop().is_some());
        assert!(ring.pop().is_none());
    }

    #[test]
    fn web3_tx_header_copy() {
        let h = web3::tx_fields_copy::TxHeader {
            chain_id: web3::tx_fields_copy::ChainId(1),
            nonce: 0,
            gas_price_wei: 0,
        };
        let _ = web3::tx_fields_copy::sign_payload(h);
        assert_eq!(h.nonce, 0);
    }

    #[test]
    fn web3_simulate_borrow_no_alloc_on_pass() {
        let data = vec![0u8; 100];
        assert!(web3::mev_simulate_borrow::simulate_tx(
            &data,
            web3::mev_simulate_borrow::GasLimit(21_000)
        ));
    }

    #[test]
    fn generalized_cow_borrowed_for_uppercase() {
        let c = generalized::cow_lazy_clone::normalize_symbol("BTC");
        assert!(matches!(c, std::borrow::Cow::Borrowed(_)));
    }
}
