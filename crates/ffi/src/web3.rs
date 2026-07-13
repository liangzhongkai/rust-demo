//! # Web3：FFI 典型落脚点（密码学、节点 IPC、钱包）
//!
//! 与 `unsafe-rust::web3` 侧重「解码契约」不同，这里强调 **跨语言边界的所有权**
//! 与 **输出缓冲纪律**：真正把 segfault / 密钥泄漏类事故挡在生产之外。
//!
//! ## 本章对齐的线上问题
//!
//! | 场景 | FFI 形状 | Rust 封装要点 |
//! |------|-----------|----------------|
//! | libsecp256k1 / BN254 | `dst,len` + `msg,len` | `MaybeUninit` + **写满后再 `assume_init`** |
//! | JSON-RPC over UDS | blocking fd | Tokio：`spawn_blocking` + 超时；别把 `CString` 建在异步回调里 |
//! | signing extension | opaque handle | `Box::into_raw` / `from_raw` 成对；文档「单次 destroy」 |
//!
//! Stub 仍为 crate 内 `extern "C"`，避免额外 native 依赖。

#![allow(dead_code)]

use std::mem::MaybeUninit;
use std::slice;

/// 模拟签名／哈希扩展：`dst` 至少 64 字节；写入确定性图案便于断言。
///
/// # Safety
///
/// `dst` 有效且长度 ≥ `dst_len`；`msg` 可读长度 `msg_len`。
#[no_mangle]
pub unsafe extern "C" fn demo_crypto_dual_out(
    dst: *mut u8,
    dst_len: usize,
    msg: *const u8,
    msg_len: usize,
) -> i32 {
    if dst.is_null() || msg.is_null() || dst_len < 64 {
        return -1;
    }
    let msg_sl = slice::from_raw_parts(msg, msg_len);
    let dst_sl = slice::from_raw_parts_mut(dst, dst_len);
    for i in 0..64 {
        dst_sl[i] = i as u8;
    }
    dst_sl[0] = msg_sl.len() as u8;
    dst_sl[1] = msg_sl.first().copied().unwrap_or(0);
    0
}

/// Safe façade：`r || s` 风格的 64 字节复合输出留在栈上数组。
///
/// 生产迁移：
/// - 把 `demo_crypto_dual_out` 换成真实 `secp256k1_ecdsa_signature_serialize_compact` 等；
/// - **密钥材料**优先走 `Zeroize`，不要在演示之外手写 XOR「清零」假装安全。
pub fn sign_compact_stub(msg: &[u8]) -> Result<[u8; 64], ()> {
    let mut out = MaybeUninit::<[u8; 64]>::uninit();
    let rc =
        unsafe { demo_crypto_dual_out(out.as_mut_ptr().cast::<u8>(), 64, msg.as_ptr(), msg.len()) };
    if rc != 0 {
        return Err(());
    }
    Ok(unsafe { out.assume_init() })
}

/// Web3 IPC-style：**opaque session**，C 侧只保存指针。
#[repr(C)]
pub struct ChainSession {
    pub chain_id: u64,
    pub rpc_epoch: u64,
}

pub type ChainSessionHandle = *mut ChainSession;

#[no_mangle]
pub extern "C" fn demo_chain_session_create(chain_id: u64, rpc_epoch: u64) -> ChainSessionHandle {
    let b = Box::new(ChainSession {
        chain_id,
        rpc_epoch,
    });
    Box::into_raw(b)
}

/// # Safety
///
/// `h` 必须为 `demo_chain_session_create` 的有效返回值且仅销毁一次。
#[no_mangle]
pub unsafe extern "C" fn demo_chain_session_destroy(h: ChainSessionHandle) {
    if h.is_null() {
        return;
    }
    drop(Box::from_raw(h));
}

/// ❌ 陷阱：假定 C 已写满输出前就 `assume_init` / 读取 —— 演示用零初始化代替（浪费但安全）。
///
/// 对照 `sign_compact_stub`：真实 FFI 必须用 `MaybeUninit`，写满后再 `assume_init`。
pub fn sign_compact_zeroed_wasteful(msg: &[u8]) -> Result<[u8; 64], ()> {
    let mut out = [0u8; 64]; // ❌ 语义上「假装已初始化」；大缓冲浪费且易误导
    let rc = unsafe { demo_crypto_dual_out(out.as_mut_ptr(), 64, msg.as_ptr(), msg.len()) };
    if rc != 0 {
        return Err(());
    }
    Ok(out)
}

/// ❌ 陷阱：创建 session 后忘记 `destroy` —— 裸指针是 `Copy`，`forget` 无效；泄漏来自未调用 destroy。
pub fn session_create_and_leak(chain_id: u64, rpc_epoch: u64) {
    let h = demo_chain_session_create(chain_id, rpc_epoch);
    let _ = h; // ❌ 从不调用 demo_chain_session_destroy → Box 永驻堆上
}

pub fn demonstrate() {
    println!("## Web3：签名／哈希 FFI —— MaybeUninit vs 零初始化");
    let msg = b"hello rollup";
    let sig_ok = sign_compact_stub(msg).expect("stub ok");
    let sig_trap = sign_compact_zeroed_wasteful(msg).expect("stub ok");
    println!(
        "  ✅ `MaybeUninit` 前 8 bytes = {:?}",
        &sig_ok[..8].to_vec()
    );
    println!(
        "  ❌ 零初始化栈数组前 8 bytes = {:?}（结果相同但语义错误、大输出浪费）",
        &sig_trap[..8].to_vec()
    );
    assert_eq!(sig_ok, sig_trap);

    println!("## Web3：opaque session（into_raw / from_raw 成对）");
    let h = demo_chain_session_create(8453, 17);
    unsafe {
        println!("  ✅ create → chain_id = {}", (*h).chain_id);
        demo_chain_session_destroy(h);
        println!("  ✅ destroy 一次，Box 回收");
    }
    session_create_and_leak(1, 0);
    println!("  ❌ `session_create_and_leak` 未调用 destroy → 堆上 Box 泄漏");
    println!("规则：输出缓冲 `MaybeUninit`；handle `create`/`destroy` 文档化配对\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_out_propagates_msg_hint() {
        let sig = sign_compact_stub(&[9u8, 8, 7]).unwrap();
        assert_eq!(sig[0], 3); // msg len
        assert_eq!(sig[1], 9); // first byte
    }

    #[test]
    fn session_roundtrip_drop() {
        let h = demo_chain_session_create(1, 0);
        unsafe {
            assert_eq!((*h).chain_id, 1);
            demo_chain_session_destroy(h);
        }
    }
}
