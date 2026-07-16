//! # Web3 生产场景下的泛型
//!
//! 链上/链下基础设施的硬约束：
//! - **多链**：Ethereum / L2 / alt-L1 共享逻辑，链参数编译期或构造期注入
//! - **类型安全**：ABI 编解码、地址、哈希长度在类型层卡住
//! - **可替换**：Hasher / RPC / VM 后端可测可换，热路径仍倾向静态分发
//!
//! 下面 6 个场景对应 indexer、searcher、节点、钱包等真实组件。

#![allow(dead_code)]

pub type ChainId = u64;

// ============================================================================
// 场景 1：泛型 ABI 编解码（trait + 关联类型）
// ============================================================================
/// **生产问题**：Router 收到 `calldata`，要先读 4 字节 selector，再按 ABI 解参数。
/// 手写 `match selector` 会在合约升级时爆炸；反射式 HashMap 解码太慢。
///
/// **泛型套路**：`SolType` 关联 `Encoded` 布局，`Codec<T>` 单态化 encode/decode。
pub mod abi_codec {
    pub trait SolType: Sized {
        type Encoded: AsRef<[u8]>;
        fn encode(&self) -> Self::Encoded;
        fn decode(bytes: &[u8]) -> Option<Self>;
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct U256([u8; 32]);

    impl U256 {
        pub fn from_u64(v: u64) -> Self {
            let mut buf = [0u8; 32];
            buf[24..].copy_from_slice(&v.to_be_bytes());
            Self(buf)
        }

        pub fn to_u64(&self) -> u64 {
            let mut tail = [0u8; 8];
            tail.copy_from_slice(&self.0[24..]);
            u64::from_be_bytes(tail)
        }
    }

    impl SolType for U256 {
        type Encoded = [u8; 32];

        fn encode(&self) -> Self::Encoded {
            self.0
        }

        fn decode(bytes: &[u8]) -> Option<Self> {
            if bytes.len() < 32 {
                return None;
            }
            let mut buf = [0u8; 32];
            buf.copy_from_slice(&bytes[..32]);
            Some(Self(buf))
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Address([u8; 20]);

    impl Address {
        pub fn zero() -> Self {
            Self([0u8; 20])
        }
    }

    impl SolType for Address {
        type Encoded = [u8; 32]; // ABI 左 padding 到 32

        fn encode(&self) -> Self::Encoded {
            let mut out = [0u8; 32];
            out[12..].copy_from_slice(&self.0);
            out
        }

        fn decode(bytes: &[u8]) -> Option<Self> {
            if bytes.len() < 32 {
                return None;
            }
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&bytes[12..32]);
            Some(Self(addr))
        }
    }

    pub fn encode_call<T: SolType>(sel: [u8; 4], arg: &T) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 32);
        out.extend_from_slice(&sel);
        out.extend_from_slice(arg.encode().as_ref());
        out
    }

    pub fn demonstrate() {
        println!("## Web3-1：泛型 ABI Codec SolType");

        let sel = [0x12, 0x34, 0x56, 0x78];
        let amount = U256::from_u64(1_000_000);
        let calldata = encode_call(sel, &amount);
        println!("calldata len = {} (4 + 32)", calldata.len());

        let decoded = U256::decode(&calldata[4..]).unwrap();
        println!("roundtrip amount = {}", decoded.to_u64());
        println!("新 Solidity 类型 = 新 impl SolType；无运行时反射\n");
    }
}

// ============================================================================
// 场景 2：泛型交易信封（多 tx 类型统一管线）
// ============================================================================
/// **生产问题**：同时处理 Legacy、EIP-1559、Blob tx；广播、模拟、签名的「外壳」
/// 相同，字段布局不同。
///
/// **泛型套路**：`TxEnvelope<B: TxBody>`，`B` 携带链上载荷，关联 `ChainId`。
pub mod tx_envelope {
    use super::ChainId;

    pub trait TxBody {
        fn gas_limit(&self) -> u64;
        fn chain_id(&self) -> ChainId;
    }

    #[derive(Debug, Clone)]
    pub struct LegacyBody {
        pub nonce: u64,
        pub gas_price: u64,
        pub gas: u64,
        pub chain_id: ChainId,
    }

    impl TxBody for LegacyBody {
        fn gas_limit(&self) -> u64 {
            self.gas
        }

        fn chain_id(&self) -> ChainId {
            self.chain_id
        }
    }

    #[derive(Debug, Clone)]
    pub struct Eip1559Body {
        pub nonce: u64,
        pub max_fee: u64,
        pub priority_fee: u64,
        pub gas: u64,
        pub chain_id: ChainId,
    }

    impl TxBody for Eip1559Body {
        fn gas_limit(&self) -> u64 {
            self.gas
        }

        fn chain_id(&self) -> ChainId {
            self.chain_id
        }
    }

    pub struct TxEnvelope<B: TxBody> {
        pub body: B,
        pub signature: [u8; 65],
    }

    impl<B: TxBody> TxEnvelope<B> {
        pub fn max_cost_wei(&self, fee_per_gas: u64) -> u64 {
            self.body.gas_limit().saturating_mul(fee_per_gas)
        }

        pub fn validate_chain(&self, expected: ChainId) -> Result<(), &'static str> {
            if self.body.chain_id() != expected {
                Err("chain id mismatch")
            } else {
                Ok(())
            }
        }
    }

    pub fn demonstrate() {
        println!("## Web3-2：泛型 TxEnvelope<TxBody>");

        let legacy = TxEnvelope {
            body: LegacyBody {
                nonce: 1,
                gas_price: 30,
                gas: 21_000,
                chain_id: 1,
            },
            signature: [0u8; 65],
        };
        let eip1559 = TxEnvelope {
            body: Eip1559Body {
                nonce: 2,
                max_fee: 50,
                priority_fee: 2,
                gas: 100_000,
                chain_id: 1,
            },
            signature: [0u8; 65],
        };

        println!("legacy validate → {:?}", legacy.validate_chain(1));
        println!("1559 gas limit   = {}", eip1559.body.gas_limit());
        println!("统一 `broadcast(envelope)` 签名，具体 B 在编译期确定\n");
    }
}

// ============================================================================
// 场景 3：泛型 Merkle 验证（Hasher + const 深度）
// ============================================================================
/// **生产问题**：L2 状态证明、空投白名单、receipt root 都要验证 inclusion；
/// 哈希算法（keccak / sha256）与树深度因链而异。
///
/// **泛型套路**：`MerkleProof<H: Hasher, const DEPTH: usize>`。
pub mod merkle_verify {
    pub trait Hasher {
        fn hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32];
    }

    pub struct Keccak256;

    impl Hasher for Keccak256 {
        fn hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
            // 教学用 xor 折叠模拟 —— 生产用 keccak256 crate
            let mut out = [0u8; 32];
            for i in 0..32 {
                out[i] = left[i] ^ right[i];
            }
            out
        }
    }

    pub struct MerkleProof<H, const DEPTH: usize> {
        pub siblings: [[u8; 32]; DEPTH],
        pub path_bits: u64,
        _h: std::marker::PhantomData<H>,
    }

    impl<H: Hasher, const DEPTH: usize> MerkleProof<H, DEPTH> {
        pub fn verify(&self, leaf: [u8; 32], root: [u8; 32]) -> bool {
            let mut cur = leaf;
            for d in 0..DEPTH {
                let sibling = self.siblings[d];
                let left;
                let right;
                if (self.path_bits >> d) & 1 == 0 {
                    left = cur;
                    right = sibling;
                } else {
                    left = sibling;
                    right = cur;
                }
                cur = H::hash(&left, &right);
            }
            cur == root
        }
    }

    pub fn demonstrate() {
        println!("## Web3-3：泛型 MerkleProof<Hasher, DEPTH>");

        let leaf = [1u8; 32];
        let sib = [2u8; 32];
        let root = Keccak256::hash(&leaf, &sib);
        let proof = MerkleProof::<Keccak256, 1> {
            siblings: [sib],
            path_bits: 0,
            _h: std::marker::PhantomData,
        };
        println!("depth-1 verify = {}", proof.verify(leaf, root));
        println!("换 Hasher 或 DEPTH = 换类型；证明长度在类型层校验\n");
    }
}

// ============================================================================
// 场景 4：泛型 RPC 适配器（多链端点）
// ============================================================================
/// **生产问题**：indexer 同时拉 Ethereum / Arbitrum / Optimism；URL、chainId、
/// block time 不同，但 `get_block_number` / `get_logs` 接口相同。
///
/// **泛型套路**：`RpcClient<C: ChainSpec>`，关联类型固定响应解码方式。
pub mod rpc_adapter {
    use super::ChainId;

    pub trait ChainSpec {
        const CHAIN_ID: ChainId;
        const NAME: &'static str;
        fn parse_block_number(hex: &str) -> Option<u64>;
    }

    pub struct Ethereum;
    pub struct Arbitrum;

    impl ChainSpec for Ethereum {
        const CHAIN_ID: ChainId = 1;
        const NAME: &'static str = "ethereum";

        fn parse_block_number(hex: &str) -> Option<u64> {
            u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok()
        }
    }

    impl ChainSpec for Arbitrum {
        const CHAIN_ID: ChainId = 42_161;
        const NAME: &'static str = "arbitrum";

        fn parse_block_number(hex: &str) -> Option<u64> {
            u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok()
        }
    }

    pub struct RpcClient<C: ChainSpec> {
        pub endpoint: String,
        _c: std::marker::PhantomData<C>,
    }

    impl<C: ChainSpec> RpcClient<C> {
        pub fn new(endpoint: impl Into<String>) -> Self {
            Self {
                endpoint: endpoint.into(),
                _c: std::marker::PhantomData,
            }
        }

        pub fn chain_name(&self) -> &'static str {
            C::NAME
        }

        pub fn handle_block_response(&self, hex: &str) -> Option<u64> {
            C::parse_block_number(hex)
        }
    }

    pub fn demonstrate() {
        println!("## Web3-4：泛型 RpcClient<ChainSpec>");

        let eth = RpcClient::<Ethereum>::new("https://eth.example");
        let arb = RpcClient::<Arbitrum>::new("https://arb.example");
        println!(
            "eth block = {:?}, arb block = {:?}",
            eth.handle_block_response("0x10"),
            arb.handle_block_response("0x20")
        );
        println!(
            "同函数 `new`，不同类型 `RpcClient<Ethereum>` vs `RpcClient<Arbitrum>`\n"
        );
    }
}

// ============================================================================
// 场景 5：泛型 Mempool 过滤器（谓词组合）
// ============================================================================
/// **生产问题**：searcher 对 pending tx 跑数十条规则（min gas、router 白名单、
/// 方法 selector）；规则天天变，不能每次改核心循环。
///
/// **泛型套路**：`Predicate<T>` + 元组/链式 AND，`Filter<P>` 单态化整条规则链。
pub mod mempool_filter {
    #[derive(Debug, Clone)]
    pub struct PendingTx {
        pub to: [u8; 20],
        pub selector: [u8; 4],
        pub gas_price: u64,
    }

    pub trait Predicate<T> {
        fn matches(&self, tx: &T) -> bool;
    }

    pub struct MinGas(pub u64);

    impl Predicate<PendingTx> for MinGas {
        fn matches(&self, tx: &PendingTx) -> bool {
            tx.gas_price >= self.0
        }
    }

    pub struct RouterWhitelist<const N: usize>(pub [[u8; 20]; N]);

    impl<const N: usize> Predicate<PendingTx> for RouterWhitelist<N> {
        fn matches(&self, tx: &PendingTx) -> bool {
            self.0.contains(&tx.to)
        }
    }

    pub struct SelectorIs(pub [u8; 4]);

    impl Predicate<PendingTx> for SelectorIs {
        fn matches(&self, tx: &PendingTx) -> bool {
            tx.selector == self.0
        }
    }

    impl<T, A, B> Predicate<T> for (A, B)
    where
        A: Predicate<T>,
        B: Predicate<T>,
    {
        fn matches(&self, tx: &T) -> bool {
            self.0.matches(tx) && self.1.matches(tx)
        }
    }

    pub struct Filter<P> {
        predicate: P,
    }

    impl<P> Filter<P> {
        pub fn new(predicate: P) -> Self {
            Self { predicate }
        }

        pub fn accept<T>(&self, tx: &T) -> bool
        where
            P: Predicate<T>,
        {
            self.predicate.matches(tx)
        }
    }

    pub fn demonstrate() {
        println!("## Web3-5：泛型 Mempool Filter Predicate");

        let router = [0xAAu8; 20];
        let rules = Filter::new((
            MinGas(20),
            (
                RouterWhitelist::<1>([router]),
                SelectorIs([0x38, 0xed, 0x17, 0x39]),
            ),
        ));

        let hit = PendingTx {
            to: router,
            selector: [0x38, 0xed, 0x17, 0x39],
            gas_price: 30,
        };
        let miss = PendingTx {
            to: router,
            selector: [0x00, 0x00, 0x00, 0x00],
            gas_price: 30,
        };
        println!("hit = {}, miss = {}", rules.accept(&hit), rules.accept(&miss));
        println!("规则链编译期内联；新增规则不碰 `Filter` 结构\n");
    }
}

// ============================================================================
// 场景 6：泛型 VM 指令分派（关联类型 OpTable）
// ============================================================================
/// **生产问题**：revm / 自定义 simulator 对每个 opcode 做不同栈操作；用
/// `match opcode` 巨型分支难测难扩展。
///
/// **泛型套路**：`Interpreter<Ops: OpTable>`，`Ops::dispatch` 关联具体 ISA。
pub mod vm_dispatch {
    pub trait OpTable {
        type Opcode: Copy + Into<u8>;
        fn dispatch(op: Self::Opcode, stack: &mut Vec<u64>) -> Result<(), &'static str>;
    }

    #[derive(Clone, Copy)]
    pub enum EvmOp {
        Push = 0,
        Add = 1,
        Stop = 2,
    }

    impl From<EvmOp> for u8 {
        fn from(o: EvmOp) -> u8 {
            o as u8
        }
    }

    pub struct EvmIsa;

    impl OpTable for EvmIsa {
        type Opcode = EvmOp;

        fn dispatch(op: Self::Opcode, stack: &mut Vec<u64>) -> Result<(), &'static str> {
            match op {
                EvmOp::Push => {
                    stack.push(1);
                    Ok(())
                }
                EvmOp::Add => {
                    let b = stack.pop().ok_or("stack underflow")?;
                    let a = stack.pop().ok_or("stack underflow")?;
                    stack.push(a + b);
                    Ok(())
                }
                EvmOp::Stop => Err("halt"),
            }
        }
    }

    pub struct Interpreter<Ops: OpTable> {
        stack: Vec<u64>,
        _ops: std::marker::PhantomData<Ops>,
    }

    impl<Ops: OpTable> Interpreter<Ops> {
        pub fn new() -> Self {
            Self {
                stack: Vec::new(),
                _ops: std::marker::PhantomData,
            }
        }

        pub fn step(&mut self, op: Ops::Opcode) -> Result<(), &'static str> {
            Ops::dispatch(op, &mut self.stack)
        }

        pub fn peek(&self) -> Option<u64> {
            self.stack.last().copied()
        }
    }

    pub fn demonstrate() {
        println!("## Web3-6：泛型 Interpreter<OpTable>");

        let mut vm = Interpreter::<EvmIsa>::new();
        assert!(vm.step(EvmOp::Push).is_ok());
        assert!(vm.step(EvmOp::Push).is_ok());
        assert!(vm.step(EvmOp::Add).is_ok());
        println!("stack top after PUSH,PUSH,ADD = {:?}", vm.peek());
        println!("换 ISA = 新 impl OpTable；解释器循环代码零修改\n");
    }
}

pub fn demonstrate() {
    abi_codec::demonstrate();
    tx_envelope::demonstrate();
    merkle_verify::demonstrate();
    rpc_adapter::demonstrate();
    mempool_filter::demonstrate();
    vm_dispatch::demonstrate();
}
