//! # 泛化：从 HFT/Web3 场景到通用栈/堆决策策略
//!
//! 把前两章具体业务里的内存布局套路抽象出来，得到一张
//! **「问题类型 → 推荐策略」决策矩阵**：
//!
//! | 问题类型           | 标志特征                     | 首选策略                          |
//! |--------------------|------------------------------|-----------------------------------|
//! | 1. 定长小对象      | size ≤ 64B, Copy             | 栈值 / [T; N]                       |
//! | 2. 有界可变长      | max 已知，常见 case 很小     | InlineBuffer / SmallVec             |
//! | 3. 流式解析        | 输入 buffer 已存在           | 借用 slice，零拷贝 decode           |
//! | 4. 批量攒批        | 固定 batch size              | 栈 [T; N] batch + flush             |
//! | 5. 请求级临时      | 生命周期 = 一个 request/block| Bump arena + reset                  |
//! | 6. 长期共享        | 跨线程 / 跨 async task       | Arc + 预分配堆                      |
//! | 7. 复用缓冲        | 循环内重复 alloc               | clear + reserve / thread_local      |
//! | 8. 递归 / 深结构   | 深度无界                     | 显式堆栈 Vec，限制 depth            |

#![allow(dead_code)]

use crate::util::BumpArena;

// ============================================================================
// 策略 1：定长小对象 —— 栈 Copy
// ============================================================================
/// 问题：ID、价格、hash 等固定宽度字段。
/// 模式：`Copy` struct 或 `[u8; N]`，禁止 `Vec<u8>` 包 32-byte hash。
///
/// HFT: hft::l2_topn_stack（Level Copy）
/// Web3: web3::fixed_hash（Hash32）
pub mod fixed_copy {
    pub fn demonstrate() {
        println!("## 策略 1：定长 Copy 放栈");

        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        struct Id(u64);

        let a = Id(42);
        let b = a;
        assert_eq!(a, b);
        println!("  Id = {:?}, size = {} bytes", a, std::mem::size_of::<Id>());
        println!("  可直接做 HashMap key，无 indirection\n");
    }
}

// ============================================================================
// 策略 2：有界可变 —— InlineBuffer
// ============================================================================
/// 问题：长度可变但有 tight bound，常见 case 很小。
/// 模式：前 N 个 inline 栈，超出 spill 堆。
///
/// HFT: hft::fill_inline
/// Web3: web3::block_arena（logs InlineBuffer）
pub mod inline_spill {
    use crate::util::InlineBuffer;

    pub fn demonstrate() {
        println!("## 策略 2：InlineBuffer（栈 N + 堆 spill）");

        let mut buf: InlineBuffer<u32, 4> = InlineBuffer::default();
        for i in 0..6 {
            buf.push(i);
        }
        let sum: u32 = buf.iter().sum();
        println!("  sum = {}, heap_spill = {}", sum, buf.heap_spill_count());
        println!("  比纯 Vec 省掉常见 case 的首次 malloc\n");
    }
}

// ============================================================================
// 策略 3：零拷贝解析 —— 借用输入
// ============================================================================
/// 问题：decode wire format。
/// 模式：输出 `&'a [u8]` / `&'a str` tied to input lifetime。
///
/// HFT: hft::zero_alloc_parse
/// Web3: web3::calldata_decode
pub mod zero_copy_decode {
    pub fn parse_token<'a>(line: &'a str) -> Option<(&'a str, u64)> {
        let mut parts = line.splitn(2, ':');
        let key = parts.next()?;
        let val: u64 = parts.next()?.parse().ok()?;
        Some((key, val))
    }

    pub fn demonstrate() {
        println!("## 策略 3：零拷贝 parse（借用输入）");

        let line = "price:10050";
        let (k, v) = parse_token(line).unwrap();
        println!("  key = {}, val = {}（无 String alloc）\n", k, v);
    }
}

// ============================================================================
// 策略 4：栈 batch —— 攒满再 flush
// ============================================================================
/// 问题：高频小消息，syscall / lock 开销主导。
/// 模式：`[T; BATCH]` + len，满则 flush。
///
/// HFT: hft::tick_batch
pub mod stack_batch {
    pub fn demonstrate() {
        println!("## 策略 4：栈 batch buffer");

        const N: usize = 8;
        let mut batch = [0u32; N];
        let mut len = 0usize;
        let mut flushes = 0u32;

        for i in 0..20u32 {
            batch[len] = i;
            len += 1;
            if len == N {
                flushes += 1;
                len = 0;
            }
        }
        if len > 0 {
            flushes += 1;
        }
        println!("  20 items, batch={} → {} flushes\n", N, flushes);
    }
}

// ============================================================================
// 策略 5：请求级 arena —— 摊销 malloc
// ============================================================================
/// 问题：一个 request/block 内大量短生命周期小对象。
/// 模式：BumpArena，结束 reset() 一次释放。
///
/// Web3: web3::block_arena
pub mod request_arena {
    use super::*;

    pub fn demonstrate() {
        println!("## 策略 5：Bump arena（请求级堆）");

        let mut arena = BumpArena::new(256);
        for i in 0..20 {
            let buf = arena.alloc_bytes(32);
            buf[0] = i;
        }
        println!("  20 × 32B allocs → {} chunk(s)", arena.chunk_count());
        arena.reset();
        println!("  reset 后 chunk 清空，下次 request 复用模式\n");
    }
}

// ============================================================================
// 策略 6：预分配 + reuse —— reserve / clear
// ============================================================================
/// 问题：循环内 Vec 反复增长收缩。
/// 模式：`with_capacity` + `clear()` 保留 capacity。
///
/// HFT: hft::delta_buffer
pub mod preallocate_reuse {
    pub fn demonstrate() {
        println!("## 策略 6：预分配 + clear reuse");

        let mut buf: Vec<u32> = Vec::with_capacity(1024);
        for round in 0..5 {
            for i in 0..500 {
                buf.push(i + round);
            }
            let _sum: u32 = buf.iter().sum();
            buf.clear();
        }
        println!("  5 轮 × 500 push，capacity 保持 = {}\n", buf.capacity());
    }
}

// ============================================================================
// 策略 7：thread-local  scratch —— 跨调用复用
// ============================================================================
/// 问题：热路径需要临时 String/Vec，但不想每次 alloc。
/// 模式：`thread_local!` + `RefCell<Vec<u8>>` take/return。
///
/// HFT: 日志/metrics 格式化 buffer
pub mod thread_local_scratch {
    use std::cell::RefCell;

    thread_local! {
        static SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(4096));
    }

    pub fn format_ids(ids: &[u64]) -> String {
        SCRATCH.with(|cell| {
            let mut buf = cell.borrow_mut();
            buf.clear();
            for (i, id) in ids.iter().enumerate() {
                if i > 0 {
                    buf.push(b',');
                }
                buf.extend_from_slice(id.to_string().as_bytes());
            }
            String::from_utf8(buf.clone()).unwrap()
        })
    }

    pub fn demonstrate() {
        println!("## 策略 7：thread_local scratch buffer");

        let ids = [1u64, 2, 3, 4, 5];
        let s = format_ids(&ids);
        println!("  formatted = {}（scratch 复用，仅最终 String alloc）\n", s);
    }
}

// ============================================================================
// 策略 8：决策矩阵 —— 何时栈 / 堆 / arena
// ============================================================================
pub mod decision_matrix {
    pub fn demonstrate() {
        println!("## 策略 8：栈 / 堆 / arena 决策矩阵");
        println!("  ┌─────────────────────┬──────────────┬─────────────────────────┐");
        println!("  │ 场景                │ 推荐         │ 避免                    │");
        println!("  ├─────────────────────┼──────────────┼─────────────────────────┤");
        println!("  │ ≤64B 定长           │ 栈 Copy      │ Vec<u8> 包 32B hash     │");
        println!("  │ 有界小集合          │ [T;N]/Inline │ 每次 Vec::new()         │");
        println!("  │ 解析 wire data      │ &slice       │ format!/to_string       │");
        println!("  │ 热循环临时 Vec      │ reuse+clear  │ 循环内 collect          │");
        println!("  │ block/request 临时  │ Bump arena   │ 逐条 Box/Vec            │");
        println!("  │ 跨线程共享          │ Arc<T>       │ 栈上 Rc 试图跨线程       │");
        println!("  │ >8KB 单对象         │ Vec/mmap     │ 栈上大数组              │");
        println!("  │ 递归深度无界        │ 显式 Vec 栈  │ 裸递归                  │");
        println!("  └─────────────────────┴──────────────┴─────────────────────────┘");
        println!();
        println!("  生产 checklist:");
        println!("    1. 热路径 alloc count = 0？（heaptrack / dhat 验证）");
        println!("    2. 栈数组总大小 < 4KB？");
        println!("    3. String 只出现在 IO / logging 边界？");
        println!("    4. Vec 有 reserve 且循环内 clear reuse？");
        println!("    5. 定长 digest/address 用 [u8; N]？\n");
    }
}

pub fn demonstrate() {
    fixed_copy::demonstrate();
    inline_spill::demonstrate();
    zero_copy_decode::demonstrate();
    stack_batch::demonstrate();
    request_arena::demonstrate();
    preallocate_reuse::demonstrate();
    thread_local_scratch::demonstrate();
    decision_matrix::demonstrate();
}
