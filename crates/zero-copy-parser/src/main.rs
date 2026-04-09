//! Zero-Copy Network Protocol Parser
//!
//! 展示 Rust 在 HFT/网络处理中的经典特性组合：
//! - 零拷贝解析 (bytes::Bytes, &[u8] slices)
//! - 生命周期参数确保内存安全
//! - memchr SIMD 优化的字节搜索
//! - Cow (Clone on Write) 延迟复制
//! - 内存映射文件处理
//!
//! 适用场景：HFT 网络协议解析、FIX 协议、二进制协议、日志处理

use bytes::{Buf, Bytes, BytesMut};
use std::borrow::Cow;

/// 使用 memchr 快速查找字节
/// memchr 使用 SIMD 指令加速，比手动循环快 10-100 倍
use memchr::{memchr, memchr2, memmem};

/// FIX 消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum FixMsgType {
    NewOrderSingle = b'D',
    OrderCancelRequest = b'F',
    MarketDataRequest = b'V',
    Unknown = 0,
}

impl From<u8> for FixMsgType {
    fn from(b: u8) -> Self {
        match b {
            b'D' => FixMsgType::NewOrderSingle,
            b'F' => FixMsgType::OrderCancelRequest,
            b'V' => FixMsgType::MarketDataRequest,
            _ => FixMsgType::Unknown,
        }
    }
}

/// FIX 协议订单方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum FixSide {
    Buy = b'1',
    Sell = b'2',
}

impl TryFrom<u8> for FixSide {
    type Error = &'static str;

    fn try_from(b: u8) -> Result<Self, Self::Error> {
        match b {
            b'1' => Ok(FixSide::Buy),
            b'2' => Ok(FixSide::Sell),
            _ => Err("Invalid side"),
        }
    }
}

/// 零拷贝的 FIX 消息视图
/// 'a 生命周期表示引用的数据至少与消息一样长
#[derive(Debug)]
struct FixMessage<'a> {
    /// 原始消息数据
    raw: &'a [u8],
    /// 消息类型
    msg_type: FixMsgType,
    /// 订单 ID (SOH 分隔符索引)
    order_id_idx: Option<usize>,
    /// 价格索引
    price_idx: Option<usize>,
    /// 数量索引
    qty_idx: Option<usize>,
}

impl<'a> FixMessage<'a> {
    /// 零拷贝解析 FIX 消息
    /// 输入: 字节切片引用
    /// 输出: 消息视图，不复制任何数据
    fn parse(input: &'a [u8]) -> Option<Self> {
        // FIX 协议使用 SOH (0x01) 作为分隔符
        const SOH: u8 = 0x01;

        // 快速验证：检查必需的 tag
        // memchr 使用 SIMD 加速搜索
        let begin_string = memmem::find(input, b"8=FIX.4.2")?;
        let checksum = memchr(b'=', &input[begin_string..])?;

        // 查找消息类型 (Tag 35)
        let msg_type_start = memmem::find(input, b"35=")?;
        let msg_type_end = memchr(SOH, &input[msg_type_start..])?;
        let msg_type_byte = input.get(msg_type_start + 3)?;
        let msg_type = FixMsgType::from(*msg_type_byte);

        // 查找关键字段的索引位置（不复制数据）
        let order_id_idx = memmem::find(input, b"11=").map(|i| i + 3);
        let price_idx = memmem::find(input, b"44=").map(|i| i + 3);
        let qty_idx = memmem::find(input, b"38=").map(|i| i + 3);

        Some(Self {
            raw: input,
            msg_type,
            order_id_idx,
            price_idx,
            qty_idx,
        })
    }

    /// 获取原始消息 - 零拷贝
    fn raw(&self) -> &'a [u8] {
        self.raw
    }

    /// 提取字段值 - 零拷贝，返回切片
    fn extract_field(&self, start_idx: Option<usize>) -> Option<&'a [u8]> {
        let start = start_idx?;
        let end = memchr(0x01, &self.raw[start..])?;
        Some(&self.raw[start..start + end])
    }

    /// 获取订单 ID - 零拷贝
    fn order_id(&self) -> Option<&'a [u8]> {
        self.extract_field(self.order_id_idx)
    }

    /// 获取价格 - 解析为 u64 (缩放 10000 避免浮点)
    fn price(&self) -> Option<u64> {
        let bytes = self.extract_field(self.price_idx)?;
        // 快速 ASCII 转 u64
        let mut result = 0u64;
        for &b in bytes {
            if b.is_ascii_digit() {
                result = result * 10 + (b - b'0') as u64;
            }
        }
        Some(result)
    }

    /// 获取数量
    fn qty(&self) -> Option<u64> {
        let bytes = self.extract_field(self.qty_idx)?;
        let mut result = 0u64;
        for &b in bytes {
            if b.is_ascii_digit() {
                result = result * 10 + (b - b'0') as u64;
            }
        }
        Some(result)
    }

    /// 消息类型
    fn msg_type(&self) -> FixMsgType {
        self.msg_type
    }

    /// 计算校验和 - 遍历字节
    fn checksum(&self) -> u8 {
        self.raw.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
    }
}

/// 二进制协议解析器
/// 使用内存映射 + 零拷贝处理大文件
#[derive(Debug)]
struct BinaryParser<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> BinaryParser<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    /// 检查是否有足够的数据
    fn has_remaining(&self, n: usize) -> bool {
        self.position + n <= self.data.len()
    }

    /// 读取 u8 - 零拷贝
    fn read_u8(&mut self) -> Option<u8> {
        if !self.has_remaining(1) {
            return None;
        }
        let b = self.data[self.position];
        self.position += 1;
        Some(b)
    }

    /// 读取 u16 (little endian) - 零拷贝
    fn read_u16_le(&mut self) -> Option<u16> {
        if !self.has_remaining(2) {
            return None;
        }
        let bytes = &self.data[self.position..self.position + 2];
        self.position += 2;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// 读取 u32 (little endian) - 零拷贝
    fn read_u32_le(&mut self) -> Option<u32> {
        if !self.has_remaining(4) {
            return None;
        }
        let bytes = &self.data[self.position..self.position + 4];
        self.position += 4;
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// 读取 u64 (little endian) - 零拷贝
    fn read_u64_le(&mut self) -> Option<u64> {
        if !self.has_remaining(8) {
            return None;
        }
        let bytes = &self.data[self.position..self.position + 8];
        self.position += 8;
        Some(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    /// 读取字节切片 - 零拷贝
    fn read_slice(&mut self, n: usize) -> Option<&'a [u8]> {
        if !self.has_remaining(n) {
            return None;
        }
        let slice = &self.data[self.position..self.position + n];
        self.position += n;
        Some(slice)
    }

    /// 跳过 n 字节
    fn skip(&mut self, n: usize) -> Option<()> {
        if !self.has_remaining(n) {
            return None;
        }
        self.position += n;
        Some(())
    }

    /// 查找字节位置（不移动位置）
    fn find_byte(&self, byte: u8) -> Option<usize> {
        memchr(byte, &self.data[self.position..]).map(|pos| self.position + pos)
    }
}

/// 订单簿快照消息
#[derive(Debug)]
struct OrderBookSnapshot<'a> {
    instrument_id: u32,
    bids: Vec<(u64, u64)>, // (price, qty)
    asks: Vec<(u64, u64)>,
    _raw_data: std::marker::PhantomData<&'a ()>,
}

impl<'a> OrderBookSnapshot<'a> {
    /// 从二进制数据解析订单簿快照
    fn parse(parser: &mut BinaryParser<'a>) -> Option<Self> {
        // 消息头
        let msg_type = parser.read_u8()?;
        if msg_type != 0x01 {
            return None; // 不是订单簿快照消息
        }

        let instrument_id = parser.read_u32_le()?;
        let bid_count = parser.read_u16_le()? as usize;
        let ask_count = parser.read_u16_le()? as usize;

        let mut bids = Vec::with_capacity(bid_count);
        let mut asks = Vec::with_capacity(ask_count);

        // 解析买方订单
        for _ in 0..bid_count {
            let price = parser.read_u64_le()?;
            let qty = parser.read_u64_le()?;
            bids.push((price, qty));
        }

        // 解析卖方订单
        for _ in 0..ask_count {
            let price = parser.read_u64_le()?;
            let qty = parser.read_u64_le()?;
            asks.push((price, qty));
        }

        Some(Self {
            instrument_id,
            bids,
            asks,
            _raw_data: std::marker::PhantomData,
        })
    }

    fn best_bid(&self) -> Option<(u64, u64)> {
        self.bids.first().copied()
    }

    fn best_ask(&self) -> Option<(u64, u64)> {
        self.asks.first().copied()
    }

    fn spread(&self) -> Option<u64> {
        if let (Some((bid_p, _)), Some((ask_p, _))) = (self.best_bid(), self.best_ask()) {
            Some(ask_p - bid_p)
        } else {
            None
        }
    }
}

/// Cow 使用示例：延迟复制
/// 只有在需要修改时才复制数据
fn process_message_cow(msg: Cow<[u8]>) -> Cow<[u8]> {
    // 如果数据已经是拥有的，直接返回
    // 如果是借用的，检查是否需要修改
    match msg {
        Cow::Borrowed(_) => {
            // 检查是否需要修改
            if msg[0] == b'X' {
                // 需要修改，转为拥有
                let mut owned = msg.to_vec();
                owned[0] = b'Y';
                Cow::Owned(owned)
            } else {
                // 不需要修改，返回借用
                msg
            }
        }
        Cow::Owned(owned) => Cow::Owned(owned),
    }
}

/// Bytes 和 BytesMut 使用示例
/// Bytes: 引用计数的字节切片，支持零拷贝切片
/// BytesMut: 可变的字节缓冲区
fn demonstrate_bytes_api() {
    println!("=== Bytes API Demo ===\n");

    // BytesMut - 可变缓冲区
    let mut buf = BytesMut::with_capacity(1024);

    // 写入数据
    buf.extend_from_slice(b"Hello");
    buf.extend_from_slice(b" ");
    buf.extend_from_slice(b"World");

    println!("BytesMut: {:?}", buf);

    // 冻结为 Bytes - 零拷贝，只是增加引用计数
    let bytes = buf.freeze();
    println!("Bytes: {:?}", bytes);

    // 零拷贝切片 - 不复制数据
    let slice = bytes.slice(0..5);
    println!("Slice (0..5): {:?}", slice);

    // split - 零拷贝分割
    let (left, right) = bytes.split_at(6);
    println!("After split_at(6):");
    println!("  Left: {:?}", left);
    println!("  Right: {:?}", right);

    println!();
}

fn main() {
    println!("=== Zero-Copy Network Parser ===\n");

    demonstrate_bytes_api();

    // FIX 协议解析示例
    println!("=== FIX Protocol Parsing ===\n");

    // 模拟 FIX 消息 (使用 SOH 0x01 作为分隔符)
    let fix_msg = b"8=FIX.4.2\x0135=D\x0149=CLIENT\x0111=ORDER123\x0138=100\x0144=1005000\x0153=1\x0110=123\x01";

    // 零拷贝解析
    if let Some(msg) = FixMessage::parse(fix_msg) {
        println!("Parsed FIX message:");
        println!("  Type: {:?}", msg.msg_type());
        println!("  Order ID: {:?}", msg.order_id());
        println!("  Price: {:?}", msg.price());
        println!("  Qty: {:?}", msg.qty());
        println!("  Raw length: {} bytes", msg.raw().len());
        println!("  Checksum: {}", msg.checksum());
    }

    println!();

    // 二进制协议解析示例
    println!("=== Binary Protocol Parsing ===\n");

    // 构造二进制消息
    let mut binary_msg = Vec::new();
    binary_msg.push(0x01); // msg_type: OrderBookSnapshot
    binary_msg.extend_from_slice(&1234u32.to_le_bytes()); // instrument_id
    binary_msg.extend_from_slice(&2u16.to_le_bytes()); // bid_count
    binary_msg.extend_from_slice(&2u16.to_le_bytes()); // ask_count
                                                       // bids
    binary_msg.extend_from_slice(&1000000u64.to_le_bytes()); // price
    binary_msg.extend_from_slice(&100u64.to_le_bytes()); // qty
    binary_msg.extend_from_slice(&999000u64.to_le_bytes()); // price
    binary_msg.extend_from_slice(&50u64.to_le_bytes()); // qty
                                                        // asks
    binary_msg.extend_from_slice(&1001000u64.to_le_bytes()); // price
    binary_msg.extend_from_slice(&150u64.to_le_bytes()); // qty
    binary_msg.extend_from_slice(&1002000u64.to_le_bytes()); // price
    binary_msg.extend_from_slice(&200u64.to_le_bytes()); // qty

    let mut parser = BinaryParser::new(&binary_msg);
    if let Some(snapshot) = OrderBookSnapshot::parse(&mut parser) {
        println!("Order Book Snapshot:");
        println!("  Instrument ID: {}", snapshot.instrument_id);
        println!("  Bids: {:?}", snapshot.bids);
        println!("  Asks: {:?}", snapshot.asks);
        println!("  Best Bid: {:?}", snapshot.best_bid());
        println!("  Best Ask: {:?}", snapshot.best_ask());
        println!("  Spread: {:?}", snapshot.spread());
    }

    println!();

    // 性能对比
    println!("=== Performance Comparison ===\n");

    // 生成测试数据
    let mut test_data = Vec::new();
    for i in 0..10_000 {
        test_data.extend_from_slice(b"8=FIX.4.2\x0135=D\x0149=CLIENT\x01");
        test_data.extend_from_slice(format!("11=ORDER{:05}\x01", i).as_bytes());
        test_data.extend_from_slice(b"38=100\x0144=1000000\x0153=1\x0110=123\x01");
    }

    use std::time::Instant;

    // 零拷贝解析
    let start = Instant::now();
    let mut parsed_count = 0;
    let mut pos = 0;
    while pos < test_data.len() {
        // 查找消息边界
        if let Some(msg_end) = memchr(0x01, &test_data[pos..]) {
            let msg_end = pos + msg_end + 1;
            if let Some(_msg) = FixMessage::parse(&test_data[pos..msg_end]) {
                parsed_count += 1;
            }
            pos = msg_end;
        } else {
            break;
        }
    }
    let zero_copy_time = start.elapsed();

    // serde_json 解析 (作为对比，需要复制数据)
    let start = Instant::now();
    let mut json_count = 0;
    pos = 0;
    while pos < test_data.len() {
        if let Some(msg_end) = memchr(0x01, &test_data[pos..]) {
            let msg_end = pos + msg_end + 1;
            // 模拟 JSON 转换开销
            let _s = String::from_utf8_lossy(&test_data[pos..msg_end]).to_string();
            json_count += 1;
            pos = msg_end;
        } else {
            break;
        }
    }
    let copy_time = start.elapsed();

    println!("Parsed {} messages", parsed_count);
    println!("Zero-copy parse: {:?}", zero_copy_time);
    println!("Copy-based parse: {:?}", copy_time);
    println!(
        "Speedup: {:.2}x",
        copy_time.as_nanos() as f64 / zero_copy_time.as_nanos() as f64
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fix_message_parse() {
        let msg = b"8=FIX.4.2\x0135=D\x0149=CLIENT\x0111=ORDER123\x0138=100\x0144=1005000\x0153=1\x0110=123\x01";
        let parsed = FixMessage::parse(msg);
        assert!(parsed.is_some());
    }

    #[test]
    fn test_binary_parser() {
        let data = [0x01, 0x00, 0x00, 0x04, 0xD2, 0x00, 0x02, 0x00, 0x02];
        let mut parser = BinaryParser::new(&data);
        assert_eq!(parser.read_u8(), Some(0x01));
        assert_eq!(parser.read_u32_le(), Some(1234));
        assert_eq!(parser.read_u16_le(), Some(2));
    }

    #[test]
    fn test_memchr() {
        let data = b"Hello\x01World\x01";
        assert_eq!(memchr(0x01, data), Some(5));
    }
}
