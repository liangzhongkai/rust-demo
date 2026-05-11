//! # 泛化：问题类型 → 应对策略矩阵
//!
//! 从 HFT / Web3 的 `unsafe` 用法抽出 **与工作负载无关** 的工程规则。

#![allow(dead_code)]

pub fn demonstrate() {
    println!("### 矩阵：问题特征 → Rust 对策");
    println!("|----------------------|--------------------------------------------|");
    println!("| 对齐 / padding       | `#[repr(C, align(N))]`，测 `size_of!`       |");
    println!("| 二进制 reinterpret   | 先 `len`/`split_at`，再 `&*(ptr as *const T)` |");
    println!("| 跨线程无锁队列       | `UnsafeCell` + 明确 memory order + 角色类型 |");
    println!("| FFI 出参             | `MaybeUninit` / `Vec` 拥有缓冲，薄封装      |");
    println!("| 不可信字节           | 解析器返回 `Option`/`Result`，拒绝先相信指针 |");
    println!("| 密钥比较             | 专用 constant-time 库，避免 `==` 短路      |");
    println!("| opaque 传给 C       | `Box::into_raw` + 文档化单次 `from_raw`/destroy |");
    println!("|----------------------|--------------------------------------------|\n");

    strategy_isolate_unsafe::demonstrate();
    strategy_type_brand::demonstrate();
    strategy_verification::demonstrate();
    strategy_queue_pick::demonstrate();
}

// ---------------------------------------------------------------------------
// 策略 1：把 `unsafe` 收敛到单文件 / 单函数“圣杯”
// ---------------------------------------------------------------------------
pub mod strategy_isolate_unsafe {
    /// 伪代码式签名：业务只看见 `Option`。
    pub fn safe_face(buf: &[u8]) -> Option<u32> {
        if buf.len() < 4 {
            return None;
        }
        let v = u32::from_le_bytes(buf[0..4].try_into().ok()?);
        Some(v)
    }

    pub fn demonstrate() {
        println!("## 策略 1：unsafe 集中到边界");
        println!(
            "  解码优先用 **纯 safe** (`from_le_bytes`)；只有 profiling 确认热点才局部 `unsafe`。",
        );
        println!("  demo: {:?}", safe_face(&[1, 0, 0, 0]));
        println!();
    }
}

// ---------------------------------------------------------------------------
// 策略 2：用类型拆分协议（crate 内：`SpscProducer` / `SpscConsumer`）
// ---------------------------------------------------------------------------
pub mod strategy_type_brand {
    pub fn demonstrate() {
        println!("## 策略 2：用类型锁线程角色");
        println!("  见 `hft::spsc_u64_pair`：`SpscProducer` 仅有 `push`，`SpscConsumer` 仅有 `pop`。");
        println!(
            "  仍须在代码评审里 insist：**每端单线程**，否则再好的类型也挡不住数据竞争。\n",
        );
    }
}

// ---------------------------------------------------------------------------
// 策略 3：验证金字塔 —— 单元 / 模糊 / Miri / Sanitizer
// ---------------------------------------------------------------------------
pub mod strategy_verification {
    pub fn demonstrate() {
        println!("## 策略 3：验证分层");
        println!("  1. 单元测试：`size_of`、`try_split`、ring 填满/掏空边界。");
        println!("  2. `cargo fuzz` / proptest：对任意 `Vec<u8>` 解码不 panic。");
        println!("  3. Miri：纯 Rust unsafe 回放未定义行为。");
        println!("  4. FFI：ASan/Valgrind；`bindgen` 生成代码配合 `#[deny(improper_ctypes)]`（在适用处）。\n");
    }
}

// ---------------------------------------------------------------------------
// 策略 4：无锁 / 有界队列选型（本 crate 无额外依赖，仅决策表）
// ---------------------------------------------------------------------------
pub mod strategy_queue_pick {
    pub fn demonstrate() {
        println!("## 策略 4：SPSC / MPMC 选型（与是否手写 unsafe）");
        println!("| 场景              | 参考 crate / 做法                    | 何时仍手写 ring        |");
        println!("|-------------------|--------------------------------------|------------------------|");
        println!("| 单线程 `async`   | `tokio::sync::mpsc`（有界）          | 跨 FFI C 回调投料      |");
        println!("| SPSC 固定容量    | `heapless::spsc::Queue`（`no_std`）  | 与 C 共享内存布局      |");
        println!("| SPSC / MPMC 通用 | `crossbeam-queue`                    | 极端延迟 + 定制 cache  |");
        println!("| 阻塞式跨线程     | `std::sync::mpsc`                    | —                      |");
        println!("|-------------------|--------------------------------------|------------------------|");
        println!(
            "  原则：**先选成熟 crate**；只有布局、内存序或与 C 段共享缓冲不可妥协时再局部 `unsafe`。\n",
        );
    }
}
