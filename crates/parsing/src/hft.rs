//! # HFT 生产场景下的解析
//!
//! 高频交易的硬约束：
//! - **延迟**：热路径禁止堆分配、禁止 UTF-8 校验、禁止 JSON
//! - **吞吐**：定长/可预测布局，配合 SIMD 字节搜索
//! - **正确**：显式错误 + 可 resync，脏包不能拖垮整条 feed
//!
//! 下面 7 个场景对应 FIX/SBE/ITCH/自定义二进制等真实写法。

#![allow(dead_code)]

pub type Px = i64;
pub type Qty = i64;

const SOH: u8 = 0x01;

// ============================================================================
// 场景 1：FIX tag-value 零拷贝解析
// ============================================================================
/// **生产问题**：柜台/交易所 FIX 会话每秒数万条，字段查找不能 `HashMap::get`
/// 更不能 `split('=').collect()`。
///
/// **解析套路**：一次线性扫描，输出 `Field` 视图；热字段用 tag 数字 switch。
pub mod fix_zero_copy {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Side {
        Buy,
        Sell,
    }

    #[derive(Debug)]
    pub struct NewOrder<'a> {
        pub cl_ord_id: &'a [u8],
        pub side: Side,
        pub px: Px,
        pub qty: Qty,
    }

    #[derive(Debug)]
    struct Field<'a> {
        tag: u16,
        value: &'a [u8],
    }

    fn scan_fields<'a>(raw: &'a [u8]) -> impl Iterator<Item = Field<'a>> + 'a {
        raw.split(|&b| b == SOH).filter_map(|chunk| {
            let eq = chunk.iter().position(|&b| b == b'=')?;
            let tag = std::str::from_utf8(&chunk[..eq]).ok()?.parse().ok()?;
            Some(Field { tag, value: &chunk[eq + 1..] })
        })
    }

    pub fn parse_new_order(raw: &[u8]) -> Result<NewOrder<'_>, &'static str> {
        let mut cl_ord_id = None;
        let mut side = None;
        let mut px = None;
        let mut qty = None;

        for f in scan_fields(raw) {
            match f.tag {
                11 => cl_ord_id = Some(f.value),
                54 => {
                    side = Some(match f.value {
                        b"1" => Side::Buy,
                        b"2" => Side::Sell,
                        _ => return Err("invalid side"),
                    });
                }
                44 => px = Some(parse_fix_decimal(f.value)?),
                38 => qty = Some(parse_fix_decimal(f.value)?),
                _ => {}
            }
        }

        Ok(NewOrder {
            cl_ord_id: cl_ord_id.ok_or("missing 11")?,
            side: side.ok_or("missing 54")?,
            px: px.ok_or("missing 44")?,
            qty: qty.ok_or("missing 38")?,
        })
    }

    /// FIX 价格常用字符串；生产里更常见的是直接传整数 tick。
    fn parse_fix_decimal(bytes: &[u8]) -> Result<i64, &'static str> {
        let s = std::str::from_utf8(bytes).map_err(|_| "bad utf8")?;
        let mut acc: i64 = 0;
        let mut sign = 1i64;
        let mut seen_dot = false;
        let mut scale = 0i32;

        for (i, c) in s.bytes().enumerate() {
            match c {
                b'-' if i == 0 => sign = -1,
                b'.' => seen_dot = true,
                b'0'..=b'9' => {
                    acc = acc
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((c - b'0') as i64))
                        .ok_or("overflow")?;
                    if seen_dot {
                        scale += 1;
                    }
                }
                _ => return Err("bad decimal"),
            }
        }
        // 归一化到 1e4 tick（示例）
        for _ in 0..(4 - scale) {
            acc = acc.checked_mul(10).ok_or("overflow")?;
        }
        Ok(acc * sign)
    }

    pub fn demonstrate() {
        println!("## 场景 1：FIX 零拷贝（tag 扫描 + switch）");
        let raw = b"8=FIX.4.2\x0111=ORD42\x0135=D\x0154=1\x0144=100.25\x0138=10\x0110=128\x01";
        let o = parse_new_order(raw).unwrap();
        println!(
            "cl_ord_id={:?} side={:?} px={} qty={}",
            std::str::from_utf8(o.cl_ord_id).unwrap(),
            o.side,
            o.px,
            o.qty
        );
        println!("关键：Field 是 `&[u8]` 视图；只在最终需要时才转 i64\n");
    }
}

// ============================================================================
// 场景 2：SBE 定长二进制帧
// ============================================================================
/// **生产问题**：CME/ICE 等用 SBE/FIX Binary，字段偏移在 schema 里固定。
/// 解析 = `from_le_bytes` + 边界检查一次。
pub mod sbe_fixed_layout {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    pub struct Tick {
        pub ts_ns: u64,
        pub px: Px,
        pub qty: Qty,
    }

    pub const TICK_SIZE: usize = 24;

    #[inline(always)]
    pub fn parse_tick(buf: &[u8]) -> Option<Tick> {
        if buf.len() < TICK_SIZE {
            return None;
        }
        Some(Tick {
            ts_ns: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            px: i64::from_le_bytes(buf[8..16].try_into().ok()?),
            qty: i64::from_le_bytes(buf[16..24].try_into().ok()?),
        })
    }

    pub fn parse_batch(buf: &[u8]) -> impl Iterator<Item = Tick> + '_ {
        buf.chunks_exact(TICK_SIZE).filter_map(parse_tick)
    }

    pub fn demonstrate() {
        println!("## 场景 2：SBE 定长帧（chunks_exact）");
        let mut wire = Vec::new();
        for i in 0..3u64 {
            wire.extend_from_slice(&(1_000 + i).to_le_bytes());
            wire.extend_from_slice(&(100_00i64 + i as i64).to_le_bytes());
            wire.extend_from_slice(&((i + 1) as i64).to_le_bytes());
        }
        let ticks: Vec<_> = parse_batch(&wire).collect();
        println!("解析 {} 笔 tick，首笔 px={}", ticks.len(), ticks[0].px);
        println!("关键：schema 驱动偏移；编译器可把循环向量化\n");
    }
}

// ============================================================================
// 场景 3：长度前缀 framing + TCP 半包重组
// ============================================================================
/// **生产问题**：行情网关走 TCP，一次 read 可能只有半帧；不能假设 buffer 完整。
pub mod length_prefixed_reassembly {
    #[derive(Default)]
    pub struct Reassembler {
        buf: Vec<u8>,
    }

    #[derive(Debug)]
    pub struct Frame {
        pub msg_type: u8,
        pub payload: Vec<u8>,
    }

    impl Reassembler {
        pub fn feed(&mut self, chunk: &[u8]) -> Vec<Frame> {
            self.buf.extend_from_slice(chunk);
            let mut out = Vec::new();
            loop {
                if self.buf.len() < 5 {
                    break;
                }
                let len = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as usize;
                if len > 1_048_576 {
                    // 脏长度：丢弃首字节 resync（见场景 7）
                    self.buf.drain(..1);
                    continue;
                }
                if self.buf.len() < 4 + len {
                    break;
                }
                let msg_type = self.buf[4];
                let payload = self.buf[5..4 + len].to_vec();
                out.push(Frame { msg_type, payload });
                self.buf.drain(..4 + len);
            }
            out
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：长度前缀 + 半包重组");
        let mut r = Reassembler::default();
        let part1 = r.feed(&[0, 0, 0, 6, 1, b'H', b'e', b'l', b'l', b'o']);
        let part2 = r.feed(&[0, 0, 0, 3, 2, b'X', b'Y']); // 第二帧一次到位
        println!("第一批 = {} 帧", part1.len());
        println!("第二批 = {} 帧", part2.len());
        println!("关键：`Reassembler` 持有未完成字节；parse 必须是增量式\n");
    }
}

// ============================================================================
// 场景 4：memchr SIMD 字段定位
// ============================================================================
/// **生产问题**：FIX 消息 50+ tag，但策略只关心 3 个；全量 scan 浪费 CPU。
///
/// **解析套路**：用 `memmem::find` 直接定位 `b"44="` 等锚点（SIMD 加速）。
pub mod simd_field_lookup {
    use memchr::memmem;

    pub fn find_tag_value<'a>(raw: &'a [u8], tag: &[u8]) -> Option<&'a [u8]> {
        let needle = [tag, b"="].concat();
        let start = memmem::find(raw, &needle)? + needle.len();
        let end = memchr::memchr(0x01, &raw[start..])? + start;
        Some(&raw[start..end])
    }

    pub fn demonstrate() {
        println!("## 场景 4：memchr 锚点查找（跳过大段无关 tag）");
        let raw = b"8=FIX.4.2\x0149=EXCH\x0156=DESK\x0111=Z\x0144=99.50\x0138=7\x01";
        let px = find_tag_value(raw, b"44").unwrap();
        let qty = find_tag_value(raw, b"38").unwrap();
        println!("px={:?} qty={:?}", std::str::from_utf8(px), std::str::from_utf8(qty));
        println!("关键：热字段 O(1) 定位；冷字段才全量 scan\n");
    }
}

// ============================================================================
// 场景 5：ITCH 风格变长消息头
// ============================================================================
/// **生产问题**：NASDAQ ITCH 用 1 字节 type + 变长 body；解析器是 match type 的分派表。
pub mod itch_dispatch {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    pub enum ItchMsg<'a> {
        AddOrder { order_id: u64, px: Px, qty: Qty, side: u8 },
        Delete { order_id: u64 },
        Raw { msg_type: u8, body: &'a [u8] },
    }

    pub fn parse_one(raw: &[u8]) -> Result<(ItchMsg<'_>, usize), &'static str> {
        if raw.is_empty() {
            return Err("empty");
        }
        let t = raw[0];
        match t {
            b'A' if raw.len() >= 1 + 8 + 8 + 8 + 1 => {
                let order_id = u64::from_le_bytes(raw[1..9].try_into().unwrap());
                let px = i64::from_le_bytes(raw[9..17].try_into().unwrap());
                let qty = i64::from_le_bytes(raw[17..25].try_into().unwrap());
                let side = raw[25];
                Ok((ItchMsg::AddOrder { order_id, px, qty, side }, 26))
            }
            b'D' if raw.len() >= 1 + 8 => {
                let order_id = u64::from_le_bytes(raw[1..9].try_into().unwrap());
                Ok((ItchMsg::Delete { order_id }, 9))
            }
            _ => Ok((ItchMsg::Raw { msg_type: t, body: &raw[1..] }, raw.len())),
        }
    }

    pub fn parse_stream(mut buf: &[u8]) -> Vec<ItchMsg<'_>> {
        let mut out = Vec::new();
        while !buf.is_empty() {
            match parse_one(buf) {
                Ok((msg, n)) => {
                    out.push(msg);
                    buf = &buf[n..];
                }
                Err(_) => break,
            }
        }
        out
    }

    pub fn demonstrate() {
        println!("## 场景 5：ITCH 变长分派（match msg_type）");
        let mut wire = Vec::new();
        wire.push(b'A');
        wire.extend_from_slice(&42u64.to_le_bytes());
        wire.extend_from_slice(&100_00i64.to_le_bytes());
        wire.extend_from_slice(&5i64.to_le_bytes());
        wire.push(b'B');
        wire.push(b'D');
        wire.extend_from_slice(&42u64.to_le_bytes());

        let msgs = parse_stream(&wire);
        for m in &msgs {
            println!("  {:?}", m);
        }
        println!("关键：每种 type 独立 `consumed` 长度，流式推进 cursor\n");
    }
}

// ============================================================================
// 场景 6：CSV tick 回放 + 行级错误恢复
// ============================================================================
/// **生产问题**：研究/回放用 CSV，坏行不能整文件失败；要 skip + metric。
pub mod csv_replay_recovery {
    use super::*;

    #[derive(Debug)]
    pub struct CsvTick {
        pub ts: u64,
        pub px: Px,
        pub qty: Qty,
    }

    pub struct ParseStats {
        pub ok: u64,
        pub skipped: u64,
    }

    pub fn parse_line(line: &str) -> Option<CsvTick> {
        let mut parts = line.split(',');
        let ts: u64 = parts.next()?.parse().ok()?;
        let px: i64 = parts.next()?.parse().ok()?;
        let qty: i64 = parts.next()?.parse().ok()?;
        Some(CsvTick { ts, px, qty })
    }

    pub fn replay(data: &str) -> (Vec<CsvTick>, ParseStats) {
        let mut ticks = Vec::new();
        let mut stats = ParseStats { ok: 0, skipped: 0 };
        for line in data.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            match parse_line(line) {
                Some(t) => {
                    stats.ok += 1;
                    ticks.push(t);
                }
                None => stats.skipped += 1,
            }
        }
        (ticks, stats)
    }

    pub fn demonstrate() {
        println!("## 场景 6：CSV 回放错误恢复");
        let csv = "1000,10000,5\nbad,line\n1001,10010,3\n# comment\n1002,xx,1\n";
        let (ticks, stats) = replay(csv);
        println!("有效 {} 行，跳过 {} 行", stats.ok, stats.skipped);
        println!("首笔 ts={} px={}", ticks[0].ts, ticks[0].px);
        println!("关键：生产 feed 必须「坏包跳过 + 计数」，不能 unwrap 整链路\n");
    }
}

// ============================================================================
// 场景 7：帧同步 resync（脏包自愈）
// ============================================================================
/// **生产问题**：网络 glitch 导致 length 字段损坏；parser 不能死锁在超大 len。
/// 策略：magic byte 扫描重新对齐。
pub mod frame_resync {
    pub const MAGIC: &[u8] = b"MD";

    pub fn resync_and_parse(mut buf: &[u8]) -> Vec<&[u8]> {
        let mut frames = Vec::new();
        while !buf.is_empty() {
            let Some(pos) = find_magic(buf) else {
                break;
            };
            buf = &buf[pos..];
            if buf.len() < 2 + 4 {
                break;
            }
            let len = u32::from_le_bytes(buf[2..6].try_into().unwrap()) as usize;
            if len > 4096 || buf.len() < 6 + len {
                buf = &buf[1..]; // 向前滑 1 字节继续找 magic
                continue;
            }
            frames.push(&buf[6..6 + len]);
            buf = &buf[6 + len..];
        }
        frames
    }

    fn find_magic(buf: &[u8]) -> Option<usize> {
        buf.windows(2).position(|w| w == MAGIC)
    }

    pub fn demonstrate() {
        println!("## 场景 7：magic resync（脏流自愈）");
        let corrupt = b"GARBAGE\x00\x00\x00\xFFMD\x04\x00\x00\x00ABCDMD\x02\x00\x00\x00XY";
        let frames = resync_and_parse(corrupt);
        println!("恢复 {} 帧: {:?}", frames.len(), frames.iter().map(|f| std::str::from_utf8(f).unwrap()).collect::<Vec<_>>());
        println!("关键：parser 要有「放弃 + 重对齐」路径，否则一条脏包拖死 feed\n");
    }
}

pub fn demonstrate() {
    fix_zero_copy::demonstrate();
    sbe_fixed_layout::demonstrate();
    length_prefixed_reassembly::demonstrate();
    simd_field_lookup::demonstrate();
    itch_dispatch::demonstrate();
    csv_replay_recovery::demonstrate();
    frame_resync::demonstrate();
}
