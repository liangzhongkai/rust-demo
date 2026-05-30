//! # 解析常见陷阱与诊断
//!
//! 生产事故里反复出现的 8 个解析陷阱：
//! - 现象（监控/日志里看到什么）
//! - 根因（内存/协议层面发生了什么）
//! - 修法（一行改法 + 风格预防）

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：假设输入完整 —— TCP 半包
// ============================================================================
pub mod assume_complete_input {
    pub fn bad_parse(buf: &[u8]) -> Option<u32> {
        // ❌ 若改成 unwrap 或硬索引，半包会直接 panic
        if buf.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes(buf[0..4].try_into().ok()?))
    }

    pub fn good_parse(buf: &[u8]) -> Option<u32> {
        if buf.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes(buf[0..4].try_into().ok()?))
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：假设输入完整");
        let half = b"\x00\x00";
        println!(
            "半包：未检查 len 时用 buf[0..4] 会 panic；good = {:?}",
            good_parse(half)
        );
        println!("规则：任何 slice 索引前先 `len()` 检查；流式用 Reassembler\n");
    }
}

// ============================================================================
// 陷阱 2：热路径 UTF-8 校验
// ============================================================================
pub mod utf8_on_hot_path {
    pub fn slow_tag(raw: &[u8]) -> Option<&str> {
        // ❌ FIX 字段本质是 ASCII，却每次 from_utf8
        std::str::from_utf8(raw).ok()
    }

    pub fn fast_tag(raw: &[u8]) -> Option<&[u8]> {
        // ✅ 保持 bytes，只在日志/DB 边界转 str
        if raw.iter().all(|&b| b.is_ascii()) {
            Some(raw)
        } else {
            None
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：热路径 UTF-8");
        let v = b"ORD001";
        assert!(slow_tag(v).is_some());
        assert!(fast_tag(v).is_some());
        println!("二进制协议里优先 `&[u8]`；UTF-8 校验留给边界层\n");
    }
}

// ============================================================================
// 陷阱 3：整数溢出 silently wrapping
// ============================================================================
pub mod integer_overflow {
    pub fn bad_price(bytes: &[u8]) -> i64 {
        let mut acc = 0i64;
        for &b in bytes {
            if b.is_ascii_digit() {
                // ❌ release 下 silently wrap；debug 下可能 panic
                acc = acc.wrapping_mul(10).wrapping_add((b - b'0') as i64);
            }
        }
        acc
    }

    pub fn good_price(bytes: &[u8]) -> Result<i64, &'static str> {
        let mut acc = 0i64;
        for &b in bytes {
            if b.is_ascii_digit() {
                acc = acc
                    .checked_mul(10)
                    .and_then(|v| v.checked_add((b - b'0') as i64))
                    .ok_or("overflow")?;
            }
        }
        Ok(acc)
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：整数溢出");
        let huge = b"999999999999999999999";
        println!("bad(wrap) = {} good = {:?}", bad_price(huge), good_price(huge));
        println!("规则：外部输入一律 checked_* / saturating_* / 显式 Err\n");
    }
}

// ============================================================================
// 陷阱 4：unwrap 切片 —— 脏包 panic
// ============================================================================
pub mod unwrap_slice {
    pub fn bad(data: &[u8]) -> u8 {
        data[4] // ❌ 越界 panic
    }

    pub fn good(data: &[u8]) -> Result<u8, &'static str> {
        data.get(4).copied().ok_or("short input")
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：unwrap 切片");
        let short = b"abc";
        // bad(short) would panic
        println!("good(short) = {:?}", good(short));
        println!("规则：`.get()` / `try_into()` / `?`；feed handler 绝不能 panic\n");
    }
}

// ============================================================================
// 陷阱 5：parse 循环里分配 String
// ============================================================================
pub mod alloc_in_loop {
    pub fn slow_fields(raw: &[u8]) -> Vec<String> {
        raw.split(|&b| b == 0x01)
            .filter_map(|c| std::str::from_utf8(c).ok().map(|s| s.to_string()))
            .collect()
    }

    pub fn fast_fields<'a>(raw: &'a [u8]) -> Vec<&'a [u8]> {
        raw.split(|&b| b == 0x01).filter(|c| !c.is_empty()).collect()
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：循环内 to_string");
        let raw = b"a=1\x01b=2\x01c=3\x01";
        println!("slow len={} fast len={}", slow_fields(raw).len(), fast_fields(raw).len());
        println!("规则：解析阶段输出视图；分配推迟到业务层确实需要 owned 值时\n");
    }
}

// ============================================================================
// 陷阱 6：丢失错误上下文
// ============================================================================
pub mod no_error_context {
    pub fn bad_hex(s: &str) -> Result<[u8; 20], &'static str> {
        if s.len() != 42 {
            return Err("bad"); // ❌ 无法区分长度/字符/前缀问题
        }
        Ok([0u8; 20])
    }

    #[derive(Debug)]
    pub enum HexError {
        WrongLength { got: usize },
        InvalidChar { pos: usize },
    }

    pub fn good_hex(s: &str) -> Result<[u8; 20], HexError> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        if s.len() != 40 {
            return Err(HexError::WrongLength { got: s.len() });
        }
        Ok([0u8; 20])
    }

    pub fn demonstrate() {
        println!("## 陷阱 6：丢失错误上下文");
        println!("bad('0x1') = {:?}", bad_hex("0x1"));
        println!("good('0x1') = {:?}", good_hex("0x1"));
        println!("规则：thiserror + 字段 offset/tag；链上调试靠上下文\n");
    }
}

// ============================================================================
// 陷阱 7：组合子过度回溯
// ============================================================================
pub mod combinator_backtrack {
    pub fn ambiguous_or<'a>(a: &'a [u8], b: &'a [u8], input: &'a [u8]) -> Option<&'a [u8]> {
        // ❌ 先尝试长分支失败再回溯，重复扫描
        if input.starts_with(a) {
            Some(&input[a.len()..])
        } else if input.starts_with(b) {
            Some(&input[b.len()..])
        } else {
            None
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：组合子回溯");
        let input = b"ORDER:12345";
        println!("rest = {:?}", ambiguous_or(b"ORDER:", b"ORD:", input));
        println!("规则：LL(1) 用 peek 分派；二进制用 length/type 前缀，避免 or_else 链\n");
    }
}

// ============================================================================
// 陷阱 8：字节序/endian 混淆
// ============================================================================
pub mod endian_confusion {
    pub fn bad_wire(buf: &[u8]) -> u64 {
        // ❌ 以太坊/网络协议常 big-endian，却 from_le
        u64::from_le_bytes(buf[0..8].try_into().unwrap())
    }

    pub fn good_eth_uint(buf: &[u8]) -> Result<u128, &'static str> {
        if buf.len() != 32 {
            return Err("word size");
        }
        Ok(u128::from_be_bytes(buf[16..32].try_into().unwrap()))
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：字节序混淆");
        let word = {
            let mut w = [0u8; 32];
            w[31] = 1;
            w
        };
        println!("ABI uint = {} (expect 1)", good_eth_uint(&word).unwrap());
        println!("规则：schema 写清 endian；单元测试用已知向量\n");
    }
}

pub fn demonstrate() {
    assume_complete_input::demonstrate();
    utf8_on_hot_path::demonstrate();
    integer_overflow::demonstrate();
    unwrap_slice::demonstrate();
    alloc_in_loop::demonstrate();
    no_error_context::demonstrate();
    combinator_backtrack::demonstrate();
    endian_confusion::demonstrate();
}
