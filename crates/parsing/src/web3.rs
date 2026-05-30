//! # Web3 / 区块链生产场景下的解析
//!
//! Web3 的解析负载特征：
//! - **十六进制 everywhere**：地址、哈希、calldata、RLP
//! - **强类型 schema**：ABI selector、EIP-712、event topic
//! - **不可信输入**：节点 RPC、 mempool、链上 calldata 都必须校验边界
//!
//! 下面 6 个场景对应 reth、foundry、indexer 里的常见写法。

#![allow(dead_code)]

pub type Address = [u8; 20];
pub type B256 = [u8; 32];

// ============================================================================
// 场景 1：ABI calldata —— selector + 静态参数
// ============================================================================
/// **生产问题**：Router 收到 `data`，要先读 4 字节 selector，再按 ABI 解参数。
pub mod abi_calldata {
    use super::*;

    pub const TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb]; // transfer(address,uint256)

    #[derive(Debug, PartialEq, Eq)]
    pub struct TransferCall {
        pub to: Address,
        pub amount: u128,
    }

    pub fn decode_transfer(data: &[u8]) -> Result<TransferCall, &'static str> {
        if data.len() < 4 + 32 + 32 {
            return Err("too short");
        }
        if data[0..4] != TRANSFER_SELECTOR {
            return Err("bad selector");
        }
        // ABI 静态参数：每个 32 字节 word，右对齐
        let to_word = &data[4..36];
        let mut to = [0u8; 20];
        to.copy_from_slice(&to_word[12..32]);

        let amt_word = &data[36..68];
        let amount = u128::from_be_bytes(amt_word[16..32].try_into().unwrap());

        Ok(TransferCall { to, amount })
    }

    pub fn demonstrate() {
        println!("## 场景 1：ABI calldata（selector + 32-byte words）");
        let mut data = Vec::from(TRANSFER_SELECTOR);
        // address word（左 pad 12 零 + 20 字节地址）
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(&[0xde; 20]);
        // amount = 1000
        data.extend_from_slice(&[0u8; 16]);
        data.extend_from_slice(&1000u128.to_be_bytes());

        let call = decode_transfer(&data).unwrap();
        println!("to=0x{} amount={}", hex20(&call.to), call.amount);
        println!("关键：动态类型要读 offset word；静态类型直接切片\n");
    }
}

// ============================================================================
// 场景 2：RLP 交易解码
// ============================================================================
/// **生产问题**：执行层客户端、钱包、广播服务都要解 RLP 编码的 legacy tx。
pub mod rlp_tx {
    #[derive(Debug, PartialEq, Eq)]
    pub struct LegacyTx {
        pub nonce: u64,
        pub gas_price: u64,
        pub gas: u64,
        pub to: Option<[u8; 20]>,
        pub value: u128,
        pub data: Vec<u8>,
    }

    pub fn decode_item(input: &[u8]) -> Result<(Vec<u8>, usize), &'static str> {
        if input.is_empty() {
            return Err("empty");
        }
        let prefix = input[0];
        match prefix {
            0x00..=0x7f => Ok((vec![prefix], 1)),
            0x80..=0xb7 => {
                let len = (prefix - 0x80) as usize;
                if input.len() < 1 + len {
                    return Err("short string");
                }
                Ok((input[1..1 + len].to_vec(), 1 + len))
            }
            0xb8..=0xbf => {
                let len_of_len = (prefix - 0xb7) as usize;
                if input.len() < 1 + len_of_len {
                    return Err("short long-string header");
                }
                let len = parse_be_usize(&input[1..1 + len_of_len])?;
                let start = 1 + len_of_len;
                if input.len() < start + len {
                    return Err("short long string");
                }
                Ok((input[start..start + len].to_vec(), start + len))
            }
            0xc0..=0xf7 => {
                let len = (prefix - 0xc0) as usize;
                Ok((input[1..1 + len].to_vec(), 1 + len)) // 简化：返回 raw list payload
            }
            _ => Err("unsupported rlp"),
        }
    }

    fn parse_be_usize(bytes: &[u8]) -> Result<usize, &'static str> {
        if bytes.is_empty() || bytes.len() > 8 {
            return Err("bad length field");
        }
        let mut acc = 0usize;
        for &b in bytes {
            acc = acc.checked_mul(256).and_then(|v| v.checked_add(b as usize)).ok_or("overflow")?;
        }
        Ok(acc)
    }

    pub fn decode_u64(item: &[u8]) -> Result<u64, &'static str> {
        if item.is_empty() {
            return Ok(0);
        }
        if item.len() > 8 {
            return Err("u64 overflow");
        }
        let mut acc = 0u64;
        for &b in item {
            acc = acc.checked_mul(256).and_then(|v| v.checked_add(b as u64)).ok_or("overflow")?;
        }
        Ok(acc)
    }

    pub fn demonstrate() {
        println!("## 场景 2：RLP 逐项解码（递归列表）");
        // [nonce, gasPrice, gas, to, value, data] 的简化演示：只解第一个 integer
        let nonce_rlp = [0x05u8]; // single byte self-encoded
        let (item, n) = decode_item(&nonce_rlp).unwrap();
        println!("nonce item = {:?}, consumed = {}", item, n);
        println!("生产里用 alloy/rlp crate；手写用于理解 offset 推进\n");
    }
}

// ============================================================================
// 场景 3：hex 地址/哈希解析 + checksum 校验
// ============================================================================
/// **生产问题**：CLI、API、前端传来的 `0x` 地址必须校验长度与字符集。
pub mod hex_address {
    use super::*;

    pub fn parse_hex_bytes(s: &str, out_len: usize) -> Result<Vec<u8>, &'static str> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        if s.len() != out_len * 2 {
            return Err("wrong length");
        }
        let mut out = vec![0u8; out_len];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hi = hex_nibble(chunk[0])?;
            let lo = hex_nibble(chunk[1])?;
            out[i] = (hi << 4) | lo;
        }
        Ok(out)
    }

    pub fn parse_address(s: &str) -> Result<Address, &'static str> {
        let v = parse_hex_bytes(s, 20)?;
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&v);
        Ok(addr)
    }

    fn hex_nibble(b: u8) -> Result<u8, &'static str> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => Err("invalid hex"),
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：hex 解析（长度 + 字符集）");
        let addr = parse_address("0x0000000000000000000000000000000000000001").unwrap();
        println!("address = 0x{}", hex20(&addr));
        assert!(parse_address("0x1234").is_err());
        println!("关键：先验长度再解码；不要用 regex 扫 hex\n");
    }
}

// ============================================================================
// 场景 4：JSON-RPC 批量响应（轻量提取）
// ============================================================================
/// **生产问题**：`eth_getLogs` 批量返回巨大 JSON；热路径应流式或 simd_json，
/// 教学版演示「只提取 id + result 数组长度」的增量扫描。
pub mod json_rpc_batch {
    pub struct RpcResponse<'a> {
        pub id: u64,
        pub result_items: usize,
        pub raw: &'a str,
    }

    pub fn parse_minimal(body: &str) -> Result<RpcResponse<'_>, &'static str> {
        let id_pos = body.find("\"id\":").ok_or("no id")?;
        let after_id = &body[id_pos + 5..];
        let id: u64 = after_id
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .map_err(|_| "bad id")?;

        let result_pos = body.find("\"result\":[").ok_or("no result array")?;
        let arr = &body[result_pos + 10..];
        let items = count_top_level_commas(arr);

        Ok(RpcResponse { id, result_items: items, raw: body })
    }

    fn count_top_level_commas(s: &str) -> usize {
        let mut depth = 0i32;
        let mut count = 0usize;
        for c in s.chars() {
            match c {
                '[' | '{' => depth += 1,
                ']' | '}' if depth > 0 => depth -= 1,
                ',' if depth <= 1 => count += 1,
                _ => {}
            }
            if c == ']' && depth == 0 {
                break;
            }
        }
        count + 1 // n 元素 ≈ n-1 逗号 + 1
    }

    pub fn demonstrate() {
        println!("## 场景 4：JSON-RPC 轻量解析");
        let body = r#"{"jsonrpc":"2.0","id":42,"result":[{"a":1},{"b":2},{"c":3}]}"#;
        let r = parse_minimal(body).unwrap();
        println!("id={} result 约 {} 条", r.id, r.result_items);
        println!("生产用 serde_json/simd-json；这里展示「不全量 DOM」思路\n");
    }
}

// ============================================================================
// 场景 5：Event log —— topic0 过滤 + data 解码
// ============================================================================
/// **生产问题**：indexer 从 receipt logs 里筛 Transfer，topic0 是事件签名哈希。
pub mod event_log {
    use super::*;

    pub const TRANSFER_TOPIC: B256 = {
        let mut t = [0u8; 32];
        // 教学占位：生产里 keccak256("Transfer(address,address,uint256)")
        t[0] = 0xdd;
        t[31] = 0xf2;
        t
    };

    #[derive(Debug)]
    pub struct TransferLog {
        pub from: Address,
        pub to: Address,
        pub value: u128,
    }

    pub struct RawLog<'a> {
        pub topics: [&'a [u8]; 3],
        pub data: &'a [u8],
    }

    pub fn decode_transfer(log: RawLog<'_>) -> Result<TransferLog, &'static str> {
        if log.topics[0] != TRANSFER_TOPIC {
            return Err("not transfer");
        }
        if log.topics[1].len() != 32 || log.topics[2].len() != 32 {
            return Err("bad indexed topic");
        }
        if log.data.len() != 32 {
            return Err("bad data");
        }

        let mut from = [0u8; 20];
        from.copy_from_slice(&log.topics[1][12..32]);
        let mut to = [0u8; 20];
        to.copy_from_slice(&log.topics[2][12..32]);
        let value = u128::from_be_bytes(log.data[16..32].try_into().unwrap());

        Ok(TransferLog { from, to, value })
    }

    pub fn demonstrate() {
        println!("## 场景 5：Event log（topic 过滤 + data word）");
        let t0 = TRANSFER_TOPIC;
        let mut t1 = [0u8; 32];
        t1[31] = 0x0a;
        let mut t2 = [0u8; 32];
        t2[31] = 0x0b;
        let mut data = [0u8; 32];
        data[31] = 100;

        let log = RawLog { topics: [&t0, &t1, &t2], data: &data };
        let tr = decode_transfer(log).unwrap();
        println!("from=0x{} to=0x{} value={}", hex20(&tr.from), hex20(&tr.to), tr.value);
        println!("关键：先比 topic0（Bloom/索引），再解码 indexed + data\n");
    }
}

// ============================================================================
// 场景 6：EIP-712 typed data hash（结构化字段解析）
// ============================================================================
/// **生产问题**：钱包签名前要解析 typedData JSON，按 schema 算 domain separator + struct hash。
pub mod eip712_minimal {
    use super::*;

    #[derive(Debug)]
    pub struct Domain {
        pub name: &'static str,
        pub chain_id: u64,
        pub verifying_contract: Address,
    }

    pub fn domain_separator(d: &Domain) -> B256 {
        // 教学简化：真实实现要对 typeHash + encodeData 做 keccak
        let mut buf = Vec::new();
        buf.extend_from_slice(d.name.as_bytes());
        buf.extend_from_slice(&d.chain_id.to_be_bytes());
        buf.extend_from_slice(&d.verifying_contract);
        keccak_like(&buf)
    }

    fn keccak_like(bytes: &[u8]) -> B256 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut out = [0u8; 32];
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        out[..8].copy_from_slice(&h.finish().to_be_bytes());
        out
    }

    pub fn demonstrate() {
        println!("## 场景 6：EIP-712 domain separator（schema 驱动）");
        let d = Domain {
            name: "MyDApp",
            chain_id: 1,
            verifying_contract: [0x11; 20],
        };
        let sep = domain_separator(&d);
        println!("domain_separator = 0x{}…", hex8(&sep));
        println!("关键：解析 JSON → 强类型 struct → 按 EIP-712 规则 hash；不要手写字符串拼接\n");
    }
}

fn hex20(b: &Address) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn hex8(b: &B256) -> String {
    b.iter().take(8).map(|x| format!("{:02x}", x)).collect()
}

pub fn demonstrate() {
    abi_calldata::demonstrate();
    rlp_tx::demonstrate();
    hex_address::demonstrate();
    json_rpc_batch::demonstrate();
    event_log::demonstrate();
    eip712_minimal::demonstrate();
}
