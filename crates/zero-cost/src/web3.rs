//! # Web3 生产场景下的零成本抽象
//!
//! 链上/链下系统的共同约束：
//! - **确定性**：同一输入必须同一输出（禁 f64、禁随机 hash map 迭代顺序依赖）
//! - **Gas / 带宽**：编码路径要紧凑，解码要可 inline
//! - **多链**：地址/哈希用 newtype 区分，避免 runtime tag 分支
//!
//! 下面 6 个场景展示「高层 API、底层无额外税」的 Rust 写法。

#![allow(dead_code)]

// ============================================================================
// 场景 1：U256 newtype —— 大整数语义，栈/数组布局可控
// ============================================================================
/// **生产问题**：链上 uint256 用 `[u8; 32]` 裸传，容易大小端搞反、
/// 与 u128 混算溢出 silently。
///
/// **零成本套路**：`U256([u64; 4])` 包装，运算仍是固定宽度整数指令序列。
pub mod u256_newtype {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct U256(pub [u64; 4]);

    impl U256 {
        pub const ZERO: Self = Self([0; 4]);

        #[inline]
        pub fn from_u128(v: u128) -> Self {
            Self([v as u64, (v >> 64) as u64, 0, 0])
        }

        /// 简化版：只支持 u128 范围内加法（演示布局，非完整 uint256）
        #[inline]
        pub fn wrapping_add(self, rhs: Self) -> Self {
            let mut out = [0u64; 4];
            let mut carry = 0u128;
            for i in 0..4 {
                let sum = self.0[i] as u128 + rhs.0[i] as u128 + carry;
                out[i] = sum as u64;
                carry = sum >> 64;
            }
            Self(out)
        }

        pub fn is_zero(self) -> bool {
            self.0 == [0; 4]
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：U256 newtype");
        let a = U256::from_u128(1_000_000_000_000_000_000);
        let b = U256::from_u128(2);
        let c = a.wrapping_add(b);
        println!("1e18 + 2 = {:?}", c);
        println!("关键：禁止与 u64 直接混算；布局 `[u64;4]` 可 mmap / SIMD\n");
    }
}

// ============================================================================
// 场景 2：泛型 ABI 编码 —— TypeEncode 单态化，无 runtime schema
// ============================================================================
/// **生产问题**：运行时反射式 ABI（HashMap 字段名 → encoder）在
/// batch submit 路径上分配频繁，且难以 inline。
///
/// **零成本套路**：`encode<T: TypeEncode>()` 编译期展开为固定字节序列。
pub mod abi_generic {
    pub trait TypeEncode {
        fn encode(&self, out: &mut Vec<u8>);
    }

    impl TypeEncode for u64 {
        #[inline]
        fn encode(&self, out: &mut Vec<u8>) {
            out.extend_from_slice(&self.to_be_bytes());
        }
    }

    impl TypeEncode for [u8; 32] {
        #[inline]
        fn encode(&self, out: &mut Vec<u8>) {
            out.extend_from_slice(self);
        }
    }

    /// 元组编码 = 顺序 append，单态化后等价于手写 concat
    impl TypeEncode for (u64, [u8; 32]) {
        #[inline]
        fn encode(&self, out: &mut Vec<u8>) {
            self.0.encode(out);
            self.1.encode(out);
        }
    }

    pub fn encode_call<T: TypeEncode>(selector: [u8; 4], args: &T) -> Vec<u8> {
        let mut out = selector.to_vec();
        args.encode(&mut out);
        out
    }

    pub fn demonstrate() {
        println!("## 场景 2：泛型 TypeEncode ABI");
        let addr = [0xab_u8; 32];
        let calldata = encode_call([0x12, 0x34, 0x56, 0x78], &(1_000_u64, addr));
        println!("calldata len = {} bytes", calldata.len());
        println!("关键：每个 `(u64,[u8;32])` 调用点生成专用 encode 函数\n");
    }
}

// ============================================================================
// 场景 3：事件 log 过滤 —— 迭代器融合，topic 匹配 inline
// ============================================================================
/// **生产问题**：扫描百万条 receipt log，若每条 `String::from` + 动态 filter，
/// indexer 吞吐上不去。
///
/// **零成本套路**：`filter_map` + 固定 32 字节 topic 比较，release 融合单循环。
pub mod event_filter {
    #[derive(Clone, Copy)]
    pub struct Log {
        pub block: u64,
        pub topic0: [u8; 32],
        pub data: [u8; 32],
    }

    pub const TRANSFER: [u8; 32] = [0xdd; 32]; // 演示用假 topic

    #[inline]
    pub fn is_transfer(log: &Log) -> bool {
        log.topic0 == TRANSFER
    }

    pub fn scan_transfers(logs: &[Log], min_block: u64) -> impl Iterator<Item = &Log> {
        logs.iter().filter(move |l| l.block >= min_block && is_transfer(l))
    }

    pub fn demonstrate() {
        println!("## 场景 3：事件 log 迭代器过滤");
        let logs = [
            Log { block: 100, topic0: TRANSFER, data: [0; 32] },
            Log { block: 99, topic0: TRANSFER, data: [0; 32] },
            Log { block: 101, topic0: [0; 32], data: [0; 32] },
        ];
        let n = scan_transfers(&logs, 100).count();
        println!("transfer logs (block>=100) = {}", n);
        println!("关键：无 String；topic 比较编译为 32 字节 memcmp\n");
    }
}

// ============================================================================
// 场景 4：Merkle 验证泛型哈希 —— HashFn 单态化 Keccak/SHA256
// ============================================================================
/// **生产问题**：MPT / Merkle 路径验证若用 `Box<dyn Fn>` 做 hash，
/// 每个节点一次间接调用，证明验证延迟线性放大。
///
/// **零成本套路**：`fn verify<H: Hasher>(proof: &Proof<H>)` 单态化 hash 函数。
pub mod merkle_generic {
    pub trait Hasher {
        fn hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32];
    }

    pub struct Keccak256;
    impl Hasher for Keccak256 {
        #[inline]
        fn hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
            // 演示：xor 占位，生产用 tiny-keccak / sha3
            let mut out = [0u8; 32];
            for i in 0..32 {
                out[i] = left[i] ^ right[i];
            }
            out
        }
    }

    pub struct Sha256;
    impl Hasher for Sha256 {
        #[inline]
        fn hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
            let mut out = [0u8; 32];
            for i in 0..32 {
                out[i] = left[i].wrapping_add(right[i]);
            }
            out
        }
    }

    pub fn verify<H: Hasher>(leaf: [u8; 32], siblings: &[[u8; 32]]) -> [u8; 32] {
        siblings
            .iter()
            .fold(leaf, |acc, sib| H::hash(&acc, sib))
    }

    pub fn demonstrate() {
        println!("## 场景 4：Merkle verify 泛型 Hasher");
        let leaf = [1u8; 32];
        let sibs = [[2u8; 32], [3u8; 32]];
        let k = verify::<Keccak256>(leaf, &sibs);
        let s = verify::<Sha256>(leaf, &sibs);
        println!("root(keccak-demo) = {:02x?}...", &k[0..4]);
        println!("root(sha256-demo) = {:02x?}...", &s[0..4]);
        println!("关键：换哈希 = 换类型参数，热路径无 vtable\n");
    }
}

// ============================================================================
// 场景 5：多链地址 newtype —— 编译期链 ID，无 runtime enum 分支
// ============================================================================
/// **生产问题**：`enum Chain { Eth, Arb, ... }` + match 在每个 RPC 调用分支，
/// 多链 indexer 热路径分支预测失败。
///
/// **零成本套路**：`EthAddress([u8;20])` / `ArbAddress` newtype；
/// 多链 = 多 crate feature 或多 binary，而非单 binary 内 match。
pub mod address_newtype {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct EthAddress(pub [u8; 20]);

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct ArbAddress(pub [u8; 20]);

    impl EthAddress {
        #[inline]
        pub fn from_hex_bytes(b: [u8; 20]) -> Self {
            Self(b)
        }

        pub fn checksum_display(&self) -> String {
            format!("0x{}", hex_prefix(&self.0))
        }
    }

    fn hex_prefix(b: &[u8]) -> String {
        b.iter().take(4).map(|x| format!("{:02x}", x)).collect()
    }

    pub trait RpcClient {
        fn get_balance(&self, addr: EthAddress) -> u128;
    }

    pub struct MainnetRpc;
    impl RpcClient for MainnetRpc {
        fn get_balance(&self, _addr: EthAddress) -> u128 {
            1_000_000_000_000_000_000
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：EthAddress newtype");
        let addr = EthAddress::from_hex_bytes([0xde; 20]);
        let rpc = MainnetRpc;
        println!("{} balance = {} wei", addr.checksum_display(), rpc.get_balance(addr));
        println!("关键：ArbAddress 与 EthAddress 类型不兼容，防 cross-chain 误传\n");
    }
}

// ============================================================================
// 场景 6：EVM 执行模型静态分派 —— 插件边界才用 dyn
// ============================================================================
/// **生产问题**：模拟器核心用 `dyn Vm` 调度每条 opcode，interpreted 模式
/// 已够慢，再加 vtable 雪上加霜。
///
/// **零成本套路**：`Simulate<E: Evm>` 单态化 opcode dispatch table；
/// `dyn Vm` 仅用于加载外部 precompile 插件。
pub mod evm_static {
    pub struct State {
        pub gas_used: u64,
        pub value: u128,
    }

    pub trait Evm {
        fn step(&mut self, state: &mut State, opcode: u8) -> bool;
    }

    pub struct Homestead;
    impl Evm for Homestead {
        #[inline]
        fn step(&mut self, state: &mut State, opcode: u8) -> bool {
            match opcode {
                0x00 => false, // STOP
                0x01 => {
                    state.gas_used += 3;
                    true
                }
                _ => {
                    state.gas_used += 1;
                    true
                }
            }
        }
    }

    pub struct Cancun;
    impl Evm for Cancun {
        #[inline]
        fn step(&mut self, state: &mut State, opcode: u8) -> bool {
            match opcode {
                0x00 => false,
                0x01 => {
                    state.gas_used += 2; // 假设 gas schedule 变化
                    true
                }
                _ => {
                    state.gas_used += 1;
                    true
                }
            }
        }
    }

    pub fn run<E: Evm>(evm: &mut E, code: &[u8]) -> State {
        let mut st = State { gas_used: 0, value: 0 };
        for &op in code {
            if !evm.step(&mut st, op) {
                break;
            }
        }
        st
    }

    pub fn demonstrate() {
        println!("## 场景 6：EVM 静态分派 Simulate<E>");
        let code = [0x01, 0x01, 0x00];
        let h = run(&mut Homestead, &code);
        let c = run(&mut Cancun, &code);
        println!("Homestead gas = {}, Cancun gas = {}", h.gas_used, c.gas_used);
        println!("关键：fork 规则 = 类型；replay 引擎选 E 后零 runtime fork 分支\n");
    }
}

pub fn demonstrate() {
    u256_newtype::demonstrate();
    abi_generic::demonstrate();
    event_filter::demonstrate();
    merkle_generic::demonstrate();
    address_newtype::demonstrate();
    evm_static::demonstrate();
}
