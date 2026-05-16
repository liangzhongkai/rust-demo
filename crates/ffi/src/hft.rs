//! # HFT：FFI 常见生产落脚点（与 Rust 侧的封装）
//!
//! 典型链路：**供应商 `.so` / 内核旁路 SDK / colo 时钟库** → `extern "C"`。
//! Rust 的价值在于 **把非法输入挡在边界内侧**，并把 **`unsafe` 收窄到单个函数**。
//!
//! ## 本章解决的「真实投诉」类问题
//!
//! | 线上症状 | 常见根因 | 本节对应的封装要点 |
//! |----------|----------|---------------------|
//! | 随机 SIGSEGV | 长度未校验就把裸指针交给 C | `slice::len` 先行，`NULL`/`align` 契约写清 |
//! | 抖动尖刺 | 热路径里分配 / `CString` 频繁构造 | 调用方传入 **复用 scratch**，边界外不做分配 |
//! | 解密不了的回补行情 | 大小端假设错误 | `from_*_bytes` 显式 endian；不要在文档里写「默认 LE」却不 enforce |
//!
//! HFT 示例中的 `demo_vendor_unpack_ticks` 由 **`c/vendor_ticks.c`**
//!（与同目录 [`build.rs`](../build.rs)）静态编译链接；Rust 侧仅保留 `extern "C"` 声明与 safe façade。
//! **Python**：同符号可通过 `ctypes` 加载共享库运行，见 [`python/vendor_ticks_demo.py`](../python/vendor_ticks_demo.py) 与 `bash c/build_shared.sh`。

#![allow(dead_code)]

use std::mem::size_of;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorRc {
    Ok,
    NullPointer,
    BadAlignment,
    BufferTooSmall,
    UnexpectedVendorReturn,
}

impl VendorRc {
    fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::Ok),
            -1 => Some(Self::NullPointer),
            -2 => Some(Self::BadAlignment),
            -3 => Some(Self::BufferTooSmall),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// C 符号（由 `c/vendor_ticks.c` + `build.rs` 链接）
// ---------------------------------------------------------------------------

unsafe extern "C" {
    /// # Safety
    ///
    /// `dst` / `src` / `written` 必须可写／可读／可写；`src_len` 为字节长度。
    fn demo_vendor_unpack_ticks(
        dst: *mut u64,
        dst_cap: usize,
        src: *const u8,
        src_len: usize,
        written: *mut usize,
    ) -> i32;
}

/// Safe façade：**Rust 拥有 scratch**，vendor 只在调用期内借用裸指针。
///
/// 生产要点：
/// - scratch 通常来自线程局部 arena / ring slot，避免每条消息 `Vec::with_capacity`。
/// - 若 vendor API **缓存指针**，必须把缓冲区升级为 `'static` slab（并在运维文档写明上限）。
pub fn unpack_ticks_via_vendor<'a>(
    src: &[u8],
    scratch: &'a mut [u64],
) -> Result<&'a [u64], VendorRc> {
    if src.len() % size_of::<u64>() != 0 {
        return Err(VendorRc::BadAlignment);
    }
    let need = src.len() / size_of::<u64>();
    if need > scratch.len() {
        return Err(VendorRc::BufferTooSmall);
    }
    let mut written = 0usize;
    let rc = unsafe {
        demo_vendor_unpack_ticks(
            scratch.as_mut_ptr(),
            scratch.len(),
            src.as_ptr(),
            src.len(),
            &mut written,
        )
    };
    match VendorRc::from_i32(rc) {
        Some(VendorRc::Ok) => Ok(&scratch[..written]),
        Some(e) => Err(e),
        None => Err(VendorRc::UnexpectedVendorReturn),
    }
}

/// 模拟「vendor 需要的 scratch 下限」——便于单测固定容量策略。
pub fn vendor_scratch_words_for(bytes: usize) -> Option<usize> {
    if bytes % size_of::<u64>() != 0 {
        return None;
    }
    Some(bytes / size_of::<u64>())
}

pub fn demonstrate() {
    println!("## HFT：vendor tick unpack（Rust 拥有 scratch）");
    let wire: Vec<u8> = [1u64, 2, 3]
        .into_iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let mut scratch = vec![0u64; 8];
    let ticks = unpack_ticks_via_vendor(&wire, &mut scratch).expect("vendor ok");
    println!("解码 ticks = {:?}", ticks);

    println!("## HFT：scratch 过小 → 边界返回 Err（而非 UB）");
    let tiny = &mut [0u64; 1];
    assert!(unpack_ticks_via_vendor(&wire, tiny).is_err());
    println!("过小 scratch 被拒绝\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpack_roundtrip_three_ticks() {
        let wire: Vec<u8> = [42u64, 43, 44]
            .into_iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let mut scratch = vec![0u64; 16];
        let got = unpack_ticks_via_vendor(&wire, &mut scratch).unwrap();
        assert_eq!(got, &[42, 43, 44]);
    }
}
