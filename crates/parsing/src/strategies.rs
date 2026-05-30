//! # 泛化：从 HFT/Web3 场景到通用解析策略
//!
//! | 问题类型           | 标志特征                     | 首选套路                          |
//! |--------------------|------------------------------|-----------------------------------|
//! | 1. 零拷贝视图      | 大 buffer、字段多、热路径    | `&[u8]` 视图 + 生命周期           |
//! | 2. 定长/schema     | 偏移已知、二进制 wire        | `from_*_bytes` + chunks_exact     |
//! | 3. 流式 framing    | TCP/WebSocket/P2P 半包       | 状态机 Reassembler                |
//! | 4. 锚点搜索        | 少量热字段、长消息           | memchr / memmem 定位              |
//! | 5. 分派表          | 多消息类型、变长 body        | match type + consumed bytes       |
//! | 6. 错误恢复        | 不可信 feed、坏行/脏帧       | skip + metric + resync            |
//! | 7. 强类型边界      | 外部 hex/JSON/RPC            | 显式 Error enum + 长度先验        |
//! | 8. Schema 驱动     | ABI/SBE/EIP-712              | 代码生成 / 宏，不要手写偏移       |

#![allow(dead_code)]

// ============================================================================
// 策略 1：零拷贝视图
// ============================================================================
pub mod zero_copy {
    pub struct Token<'a> {
        pub kind: &'a str,
        pub span: &'a [u8],
    }

    pub fn lex<'a>(input: &'a [u8]) -> impl Iterator<Item = Token<'a>> + 'a {
        input.split(|&b| b.is_ascii_whitespace()).filter_map(|chunk| {
            if chunk.is_empty() {
                return None;
            }
            let kind = if chunk.starts_with(b"0x") { "hex" } else { "word" };
            Some(Token { kind, span: chunk })
        })
    }

    pub fn demonstrate() {
        println!("## 策略 1：零拷贝 Token 视图");
        let input = b"transfer 0xabc 1000";
        for t in lex(input) {
            println!("  {} @{:?}", t.kind, std::str::from_utf8(t.span).unwrap());
        }
        println!("HFT: fix_zero_copy | Web3: hex_address\n");
    }
}

// ============================================================================
// 策略 2：定长 schema 解析
// ============================================================================
pub mod fixed_schema {
    pub struct Header {
        pub magic: [u8; 2],
        pub version: u16,
        pub len: u32,
    }

    pub fn parse_header(buf: &[u8]) -> Option<Header> {
        if buf.len() < 8 {
            return None;
        }
        Some(Header {
            magic: buf[0..2].try_into().ok()?,
            version: u16::from_le_bytes(buf[2..4].try_into().ok()?),
            len: u32::from_le_bytes(buf[4..8].try_into().ok()?),
        })
    }

    pub fn demonstrate() {
        println!("## 策略 2：定长 schema");
        let wire = b"MD\x01\x00\x10\x00\x00\x00";
        let h = parse_header(wire).unwrap();
        println!("magic={:?} ver={} len={}", std::str::from_utf8(&h.magic), h.version, h.len);
        println!("HFT: sbe_fixed_layout | Web3: ABI static words\n");
    }
}

// ============================================================================
// 策略 3：流式 Reassembler
// ============================================================================
pub mod streaming_reassembler {
    #[derive(Default)]
    pub struct State {
        buf: Vec<u8>,
    }

    impl State {
        pub fn push<F>(&mut self, chunk: &[u8], mut on_frame: F)
        where
            F: FnMut(&[u8]),
        {
            self.buf.extend_from_slice(chunk);
            while self.buf.len() >= 4 {
                let n = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as usize;
                if self.buf.len() < 4 + n {
                    break;
                }
                on_frame(&self.buf[4..4 + n]);
                self.buf.drain(..4 + n);
            }
        }
    }

    pub fn demonstrate() {
        println!("## 策略 3：流式 Reassembler");
        let mut st = State::default();
        let mut count = 0u32;
        st.push(b"\x00\x00\x00\x02AB", |_| count += 1);
        st.push(b"C\x00\x00\x00\x01X", |_| count += 1);
        println!("完整帧 = {}", count);
        println!("HFT: length_prefixed_reassembly | Web3: devp2p RLPx framing\n");
    }
}

// ============================================================================
// 策略 4：锚点搜索（热字段）
// ============================================================================
pub mod anchor_search {
    use memchr::memmem;

    pub fn extract<'a>(hay: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
        let start = memmem::find(hay, key)? + key.len();
        let end = memchr::memchr(b',', &hay[start..]).map(|i| start + i).unwrap_or(hay.len());
        Some(&hay[start..end])
    }

    pub fn demonstrate() {
        println!("## 策略 4：锚点搜索");
        let line = b"symbol=BTCUSDT,px=65000,qty=0.5";
        println!("px = {:?}", std::str::from_utf8(extract(line, b"px=").unwrap()));
        println!("HFT: simd_field_lookup | Web3: JSON 字段提取（生产用 simd-json）\n");
    }
}

// ============================================================================
// 策略 5：类型分派 + consumed
// ============================================================================
pub mod typed_dispatch {
    pub enum Msg<'a> {
        Ping,
        Payload(&'a [u8]),
    }

    pub fn parse_one(input: &[u8]) -> Result<(Msg<'_>, usize), &'static str> {
        match input.first() {
            Some(0) => Ok((Msg::Ping, 1)),
            Some(1) if input.len() >= 3 => Ok((Msg::Payload(&input[2..3]), 3)),
            _ => Err("unknown"),
        }
    }

    pub fn parse_all(mut buf: &[u8]) -> Vec<Msg<'_>> {
        let mut out = Vec::new();
        while !buf.is_empty() {
            match parse_one(buf) {
                Ok((m, n)) => {
                    out.push(m);
                    buf = &buf[n..];
                }
                Err(_) => break,
            }
        }
        out
    }

    pub fn demonstrate() {
        println!("## 策略 5：分派 + consumed");
        let wire = [0u8, 1, 0, b'x', 0];
        println!("msgs = {:?}", parse_all(&wire).len());
        println!("HFT: itch_dispatch | Web3: ABI selector match\n");
    }
}

// ============================================================================
// 策略 6：错误恢复 + 指标
// ============================================================================
pub mod error_recovery {
    pub struct Metrics {
        pub ok: u64,
        pub err: u64,
    }

    pub fn parse_lines(data: &str) -> (Vec<i64>, Metrics) {
        let mut out = Vec::new();
        let mut m = Metrics { ok: 0, err: 0 };
        for line in data.lines() {
            match line.parse::<i64>() {
                Ok(v) => {
                    m.ok += 1;
                    out.push(v);
                }
                Err(_) => m.err += 1,
            }
        }
        (out, m)
    }

    pub fn demonstrate() {
        println!("## 策略 6：错误恢复 + metric");
        let (v, m) = parse_lines("1\n2\nx\n3\n");
        println!("values={:?} ok={} err={}", v, m.ok, m.err);
        println!("HFT: csv_replay_recovery | Web3: 批量 RPC 里跳过坏条目\n");
    }
}

// ============================================================================
// 策略 7：强类型边界校验
// ============================================================================
pub mod typed_boundary {
    #[derive(Debug)]
    pub enum ParseError {
        TooShort { need: usize, got: usize },
        BadChar { pos: usize },
    }

    pub fn parse_u32_words(words: &[&str]) -> Result<Vec<u32>, ParseError> {
        let mut out = Vec::with_capacity(words.len());
        for (i, w) in words.iter().enumerate() {
            w.parse::<u32>().map_err(|_| ParseError::BadChar { pos: i })?;
            out.push(w.parse().unwrap());
        }
        Ok(out)
    }

    pub fn demonstrate() {
        println!("## 策略 7：强类型边界");
        println!("ok = {:?}", parse_u32_words(&["1", "2"]));
        println!("err = {:?}", parse_u32_words(&["1", "nope"]));
        println!("Web3: hex_address | HFT: fix 字段校验\n");
    }
}

// ============================================================================
// 策略 8：Schema 驱动（不要手写偏移）
// ============================================================================
pub mod schema_driven {
    /// 通用模式：schema 生成 `read_u64_at(offset)`，人工只写业务逻辑。
    pub trait WireSchema {
        const FIELD_A_OFF: usize;
        const FIELD_B_OFF: usize;
    }

    pub struct TickSchema;
    impl WireSchema for TickSchema {
        const FIELD_A_OFF: usize = 0;
        const FIELD_B_OFF: usize = 8;
    }

    pub fn read_u64<S: WireSchema>(buf: &[u8], off: usize) -> Option<u64> {
        buf.get(off..off + 8)
            .and_then(|s| s.try_into().ok())
            .map(u64::from_le_bytes)
    }

    pub fn demonstrate() {
        println!("## 策略 8：Schema 驱动");
        let buf = 42u64.to_le_bytes();
        println!("a = {}", read_u64::<TickSchema>(&buf, TickSchema::FIELD_A_OFF).unwrap());
        println!("HFT: SBE codegen | Web3: alloy sol! / foundry bind\n");
    }
}

// ============================================================================
// 反例：什么时候不要手写 parser
// ============================================================================
pub mod when_not_to_handroll {
    pub fn demonstrate() {
        println!("## 反例：何时不要手写");
        println!("  - 完整 JSON API → serde_json / simd-json");
        println!("  - 复杂 DSL / 嵌套语法 → nom / winnow / pest");
        println!("  - 以太坊 ABI/RLP → alloy / rlp crate");
        println!("  - FIX 全特性 → quickfix 或 codegen");
        println!("  - 手写前先问：有没有标准 schema + 成熟库？\n");
    }
}

pub fn demonstrate() {
    zero_copy::demonstrate();
    fixed_schema::demonstrate();
    streaming_reassembler::demonstrate();
    anchor_search::demonstrate();
    typed_dispatch::demonstrate();
    error_recovery::demonstrate();
    typed_boundary::demonstrate();
    schema_driven::demonstrate();
    when_not_to_handroll::demonstrate();
}
