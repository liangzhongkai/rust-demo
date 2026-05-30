//! # 解析底层机制
//!
//! 这一节回答四个根本问题，后面 HFT/Web3 章节里所有套路都建立在这之上：
//!
//! 1. 「解析」在 Rust 里通常是什么形态？（`&[u8]` 视图 + 显式错误）
//! 2. 组合子 vs 递归下降：什么时候用哪种？
//! 3. 零拷贝意味着什么？生命周期参数在保护什么？
//! 4. 流式输入为什么必须做成状态机，而不是一次性 `parse_all`？

#![allow(dead_code)]

/// 最小 Parser 协议：输入是 *剩余字节*，输出是 *值 + 剩余*。
/// 这是 nom/winnow/slice 系列库的共同抽象。
pub mod parser_trait {
    pub type ParseResult<'a, T> = Result<(T, &'a [u8]), &'static str>;

    pub trait Parser<'a, T> {
        fn parse(&self, input: &'a [u8]) -> ParseResult<'a, T>;
    }

    pub struct Tag(pub &'static str);

    impl<'a> Parser<'a, &'a [u8]> for Tag {
        fn parse(&self, input: &'a [u8]) -> ParseResult<'a, &'a [u8]> {
            input
                .strip_prefix(self.0.as_bytes())
                .ok_or("tag mismatch")
                .map(|rest| (self.0.as_bytes(), rest))
        }
    }

    pub struct TakeUntil(pub u8);

    impl<'a> Parser<'a, &'a [u8]> for TakeUntil {
        fn parse(&self, input: &'a [u8]) -> ParseResult<'a, &'a [u8]> {
            let pos = input.iter().position(|&b| b == self.0).ok_or("delimiter not found")?;
            Ok((&input[..pos], &input[pos + 1..]))
        }
    }

    pub fn demonstrate() {
        println!("## 1. Parser = (value, rest) 协议");

        let msg = b"8=FIX.4.2\x0111=abc\x0135=D\x01";
        let (_, rest) = Tag("8=FIX.4.2\x01").parse(msg).unwrap();
        let (field, rest) = TakeUntil(0x01).parse(rest).unwrap();

        println!("首个字段 = {:?}", std::str::from_utf8(field).unwrap());
        println!("剩余 {} 字节待继续解析", rest.len());
        println!("组合子 = 顺序调用 parse，把 rest 传给下一个\n");
    }
}

/// 递归下降：用函数调用栈表达语法树。
/// 适合 DSL、配置文件、嵌套结构；不适合热路径二进制帧。
pub mod recursive_descent {
    #[derive(Debug, PartialEq)]
    pub enum Expr {
        Num(i64),
        Add(Box<Expr>, Box<Expr>),
    }

    fn parse_num(input: &str) -> Option<(i64, &str)> {
        let end = input
            .char_indices()
            .find(|(_, c)| !c.is_ascii_digit())
            .map(|(i, _)| i)
            .unwrap_or(input.len());
        if end == 0 {
            return None;
        }
        let (head, tail) = input.split_at(end);
        Some((head.parse().ok()?, tail))
    }

    fn parse_factor(input: &str) -> Option<(Expr, &str)> {
        let (n, rest) = parse_num(input)?;
        Some((Expr::Num(n), rest))
    }

    fn parse_expr(input: &str) -> Option<(Expr, &str)> {
        let (mut left, mut rest) = parse_factor(input)?;
        while let Some(r) = rest.strip_prefix('+') {
            let (right, tail) = parse_factor(r)?;
            rest = tail;
            left = Expr::Add(Box::new(left), Box::new(right));
        }
        Some((left, rest))
    }

    pub fn demonstrate() {
        println!("## 2. 递归下降（DSL / 配置）");
        let (ast, rest) = parse_expr("12+34+5").unwrap();
        println!("AST = {:?}", ast);
        println!("未消费 = {:?}", rest);
        println!("热路径二进制帧请用定长布局，不要走递归\n");
    }
}

/// 零拷贝视图：解析结果持有对输入 buffer 的引用。
pub mod zero_copy_view {
    #[derive(Debug)]
    pub struct Field<'a> {
        pub tag: u16,
        pub value: &'a [u8],
    }

    /// 扫描 FIX 风格 tag=value\x01，不分配 String。
    pub fn scan_fields<'a>(input: &'a [u8]) -> impl Iterator<Item = Field<'a>> + 'a {
        input.split(|&b| b == 0x01).filter_map(|chunk| {
            let eq = chunk.iter().position(|&b| b == b'=')?;
            let tag = std::str::from_utf8(&chunk[..eq]).ok()?.parse().ok()?;
            Some(Field { tag, value: &chunk[eq + 1..] })
        })
    }

    pub fn demonstrate() {
        println!("## 3. 零拷贝 Field 视图");
        let raw = b"11=ORD001\x0135=D\x0138=100\x01";
        for f in scan_fields(raw) {
            println!("  tag={} value={:?}", f.tag, std::str::from_utf8(f.value).unwrap());
        }
        println!("全程无 String/Vec 分配\n");
    }
}

/// 流式状态机：TCP 半包 / WebSocket 帧 / 区块链 P2P 都必须增量喂入。
pub mod streaming_state_machine {
    #[derive(Default)]
    pub struct Framer {
        buf: Vec<u8>,
    }

    impl Framer {
        pub fn push(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
            self.buf.extend_from_slice(chunk);
            let mut out = Vec::new();
            loop {
                if self.buf.len() < 4 {
                    break;
                }
                let len = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as usize;
                if self.buf.len() < 4 + len {
                    break;
                }
                out.push(self.buf[4..4 + len].to_vec());
                self.buf.drain(..4 + len);
            }
            out
        }
    }

    pub fn demonstrate() {
        println!("## 4. 流式 framing 状态机");
        let mut framer = Framer::default();
        let a = framer.push(b"\x00\x00\x00\x03AB");
        let b = framer.push(b"C\x00\x00\x00\x02XY");
        println!("第一批完整帧 = {:?}", a);
        println!("第二批（含半包续传）= {:?}", b);
        println!("规则：parse 函数要接受 `&mut buffer`，而不是假设 input 完整\n");
    }
}

pub fn demonstrate() {
    parser_trait::demonstrate();
    recursive_descent::demonstrate();
    zero_copy_view::demonstrate();
    streaming_state_machine::demonstrate();
}
