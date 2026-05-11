//! # Web3：`unsafe` 常见触发点（与缓解）
//!
//! - **不可信输入**：mmap / 大块 state 上用 **边界先行** 的投影，绝不先 `transmute` 再检查。
//! - **FFI（密码学、节点）**：输出缓冲区由 Rust 固定栈数组或 `Vec` 拥有；调用前写清契约。
//! - **opaque 生命周期**：`Box::into_raw` ↔ `Box::from_raw` 成双出现，配对文档化。
//! - **常量时间**：真正需求应委托 `subtle` / OpenSSL timing-safe API；这里只演示 **别把密钥留在未初始化内存**。
//!
//! 生产链路还要：模糊测试解码器、`cargo miri` 对纯 Rust 核心、Valgrind/ASan 对 FFI。

#![allow(dead_code)]

use std::mem::MaybeUninit;

// ---------------------------------------------------------------------------
// 1) 不可信 RLP 风格：长度前缀 + 载荷（先边界，再无拷贝视图）
// ---------------------------------------------------------------------------

/// `tag | len | payload`，`len` 为 u16 LE，总长度不超过 `buf`。
pub fn parse_tagged_blob(buf: &[u8]) -> Option<(u8, &[u8])> {
    if buf.len() < 3 {
        return None;
    }
    let tag = buf[0];
    let len = u16::from_le_bytes([buf[1], buf[2]]) as usize;
    let end = 3usize.checked_add(len)?;
    if end > buf.len() {
        return None;
    }
    Some((tag, &buf[3..end]))
}

/// 在 **已通过** `parse_tagged_blob` 的 payload 上，将前 32 字节视为 `address` 材料（零拷贝）。
///
/// 纯 safe：`try_into` 与 mmap -backed 的 `&[u8]` 同等适用；若非要从 `*const u8` 接续 C 指针，
/// 再用 `unsafe { &*(ptr as *const [u8; 32]) }` **且必须先验证 len ≥ 32 与对齐**。
pub fn payload_as_hash_prefix(payload: &[u8]) -> Option<&[u8; 32]> {
    <&[u8; 32]>::try_from(payload.get(..32)?).ok()
}

#[test]
fn tagged_blob_rejects_ovf() {
    let mut v = vec![1u8, 0xff, 0xff];
    v.extend(std::iter::repeat(0u8).take(10));
    assert!(parse_tagged_blob(&v).is_none());

    let good = vec![9u8, 4, 0, 1, 2, 3, 4];
    let (tag, pl) = parse_tagged_blob(&good).unwrap();
    assert_eq!(tag, 9);
    assert_eq!(pl, &[1, 2, 3, 4]);
}

// ---------------------------------------------------------------------------
// 2) FFI 模式：托管输出缓冲 —— MaybeUninit 栈数组 + 「写后 assume_init」
// ---------------------------------------------------------------------------

/// 模拟外部 C 函数：`dst` 必须指向至少 32 可写字节，`len` 恒为 32。
///
/// SAFETY：`dst` 必须有效且可写，`len == 32`。
unsafe extern "C" fn sham_hash_stub(dst: *mut u8, len: usize) {
    debug_assert!(len >= 32);
    let s = core::slice::from_raw_parts_mut(dst, len);
    for (i, b) in s.iter_mut().enumerate() {
        *b = i as u8;
    }
}

/// 封装：调用方永远不接触裸指针。
pub fn compute_commitment_digest_stub() -> [u8; 32] {
    let mut out = MaybeUninit::<[u8; 32]>::uninit();
    // SAFETY：栈上固定大小数组；`sham_hash_stub` 写入满 32 字节。
    unsafe {
        sham_hash_stub(out.as_mut_ptr().cast::<u8>(), 32);
        out.assume_init()
    }
}

// ---------------------------------------------------------------------------
// 3) Opaque handle：`Box::into_raw` / `from_raw`（节点 / 钱包 FFI 常见）
// ---------------------------------------------------------------------------

/// Rust 侧状态；C 侧只存 `*mut c_void` 风格的不透明句柄。
pub struct SessionState {
    pub chain_id: u64,
    pub label: String,
}

pub type SessionHandle = *mut SessionState;

/// 创建：所有权交给调用方（通常立即交给 C 注册表）。
#[no_mangle]
pub extern "C" fn session_create(chain_id: u64) -> SessionHandle {
    let s = Box::new(SessionState {
        chain_id,
        label: format!("chain-{chain_id}"),
    });
    Box::into_raw(s)
}

/// 销毁：每个由 `session_create` 返回的指针 **必须且仅能** 调用一次。
///
/// # Safety
///
/// `h` 必须为 `session_create` 返回的有效指针，且未在此前的 `session_destroy` 中释放。
#[no_mangle]
pub unsafe extern "C" fn session_destroy(h: SessionHandle) {
    if h.is_null() {
        return;
    }
    drop(Box::from_raw(h));
}

/// Safe 便捷封装：单测 / Rust-only 集成用。
pub fn session_run<T>(chain_id: u64, f: impl FnOnce(&mut SessionState) -> T) -> T {
    let mut s = Box::new(SessionState {
        chain_id,
        label: format!("chain-{chain_id}"),
    });
    let out = f(&mut s);
    drop(s);
    out
}

#[test]
fn session_create_destroy_sound() {
    let h = session_create(1);
    unsafe {
        assert_eq!((*h).chain_id, 1);
        session_destroy(h);
    }
}

// ---------------------------------------------------------------------------
// 4) 常量时间与侧信道（说明性）
// ---------------------------------------------------------------------------

/// 真实业务请使用 `subtle::Choice` 或带 `#[inline(never)]` + 规范实现的专业库。
/// 这里演示：**不要**对密钥材料使用可能分支泄露的比较。
pub fn insecure_bytes_eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

/// 朴素逐字节异或累加（教学用；密文比较仍应使用专业 API）。
pub fn xor_reduce_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut acc = 0u8;
    for i in 0..32 {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

pub fn demonstrate() {
    println!("## Web3 · 长度前缀解析（先边界，再子切片）");
    let blob = vec![0x0du8, 4, 0, 1, 2, 3, 4];
    if let Some((tag, pl)) = parse_tagged_blob(&blob) {
        println!("  tag=0x{:02x}, payload={:?}", tag, pl);
    }
    println!("  错误长度 → None，避免越界读 state trie / receipt 缓冲。\n");

    println!("## Web3 · 零拷贝 32 字节视图");
    let mut long = vec![0u8; 40];
    long[0..32].copy_from_slice(&[7u8; 32]);
    if let Some(h) = payload_as_hash_prefix(&long) {
        println!("  first word of hash-like view: {}", h[0]);
    }
    println!("  与 mmap 大文件结合时：同一模式，先 `parse` 再投影。\n");

    println!("## Web3 · FFI 输出缓冲（MaybeUninit + 单一 unsafe 块）");
    let d = compute_commitment_digest_stub();
    println!("  stub digest[0..4] = {:?}\n", &d[..4]);

    println!("## Web3 · opaque 句柄（into_raw → from_raw）");
    let h = session_create(42161);
    unsafe {
        println!(
            "  session chain_id={}, label={}",
            (*h).chain_id,
            (*h).label
        );
        session_destroy(h);
    }
    println!("  配对原则：每条 FFI 创建的指针在文档里写明 **单次 destroy**，避免 double-free / use-after-free。\n");

    println!("## Web3 · 侧信道提示");
    println!("  `==` 比较密钥材料易短路；生产用 subtle / 平台 crypto。");
    println!(
        "  xor_reduce 示例 (32B): {}\n",
        xor_reduce_eq_32(&[1u8; 32], &[1u8; 32])
    );
}
