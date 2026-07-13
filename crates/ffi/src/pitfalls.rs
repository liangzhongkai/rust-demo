//! # FFI 常见陷阱与诊断
//!
//! 生产事故里反复出现的 5 个 FFI 陷阱：
//! - 现象（监控 / 测试里看到什么）
//! - 根因（ABI / 所有权层面发生了什么）
//! - 解决方案（对照代码 + 预防）
//!
//! 不放「刻意触发 UB」的示例 —— 用 **可安全运行的对照函数** 说明差异。

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：`Vec` / `String` 增长导致悬挂指针
// ============================================================================
/// **现象**：vendor 回调偶发 SIGSEGV，尤其在流量尖刺后。
/// **根因**：`vec.as_mut_ptr()` 交给 native 长期保存，`push` 触发 relocation。
/// **修法**：`'static` slab、`Pin`、或 native 只在调用栈生命周期内借用。
pub mod dangling_vec_ptr {
    /// ❌ 把指针交给「假装长期缓存」的侧车，同时继续 `push` 触发 relocation。
    pub fn cached_ptr_moves_after_push() -> (usize, usize, usize, usize) {
        let mut buf = Vec::with_capacity(2);
        buf.push(0);
        buf.push(0);
        let cap_before = buf.capacity();
        let cached = buf.as_ptr() as usize;
        for _ in 0..1024 {
            buf.push(0);
        }
        let after = buf.as_ptr() as usize;
        (cached, after, cap_before, buf.capacity())
    }

    /// ✅ Rust 侧拥有 scratch，vendor 只在单次调用期内借用。
    pub fn borrow_for_one_call(scratch: &mut [u64], n: usize) -> &[u64] {
        debug_assert!(n <= scratch.len());
        &scratch[..n]
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：`Vec` / `String` 增长导致悬挂指针");
        let (before, after, cap_before, cap_after) = cached_ptr_moves_after_push();
        let relocated = before != after;
        println!(
            "  ❌ push 后指针 {} → {}，capacity {} → {}（relocation = {}）",
            before, after, cap_before, cap_after, relocated
        );
        let mut scratch = [0u64; 8];
        let view = borrow_for_one_call(&mut scratch, 3);
        println!(
            "  ✅ 单次调用借用 scratch len = {}，不长期缓存裸指针",
            view.len()
        );
        println!("规则：vendor 若缓存指针 → `'static` slab 或 `Pin`，禁止边缓存边 `push`\n");
    }
}

// ============================================================================
// 陷阱 2：panic 跨过 FFI
// ============================================================================
/// **现象**：C 侧栈帧损坏、随机 abort、或「catch 不到」的崩溃。
/// **根因**：Rust unwind 进入 C/C++ 是未定义行为。
/// **修法**：`panic = abort`（edge crate）；或边界 `catch_unwind` 映射为错误码。
pub mod panic_across_ffi {
    use std::panic::catch_unwind;

    fn vendor_internal_bug() -> i32 {
        panic!("vendor returned garbage → Rust 侧 assert 失败");
    }

    /// ❌ 裸调 FFI 包装，panic 会尝试 unwind 穿过 `extern "C"`。
    fn call_vendor_unchecked() -> i32 {
        vendor_internal_bug()
    }

    /// ✅ 在边界 `catch_unwind`，把 panic 翻译成可观测错误。
    fn call_vendor_with_catch() -> Result<i32, &'static str> {
        catch_unwind(|| vendor_internal_bug()).map_err(|_| "panic → errno -99")
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：panic 跨过 FFI");
        let caught = call_vendor_with_catch();
        println!(
            "  ❌ `call_vendor_unchecked()` 会 unwind 进 C → UB（此处不实际调用）"
        );
        println!(
            "  ✅ `catch_unwind` 结果 = {:?}（panic 被挡在 Rust 边界内）",
            caught
        );
        println!("规则：`-sys` crate 设 `panic = abort`；或在 safe façade 统一 catch\n");
    }
}

// ============================================================================
// 陷阱 3：`CString::new` 含有 inner NUL → `Err`
// ============================================================================
/// **现象**：路径 / 用户名传入 signer CLI 时莫名失败。
/// **根因**：`CString::new` 拒绝内嵌 `\0`，直接 `.unwrap()` 会 panic。
/// **修法**：边界显式 `Result`；percent-encode 或替换 NUL。
pub mod cstring_inner_nul {
    use std::ffi::CString;

    /// ❌ 用户输入直接 `.unwrap()` —— 含 NUL 时 panic 或误用。
    fn path_to_cstring_trap(raw: &str) -> CString {
        CString::new(raw).expect("user path must never have NUL — 不成立")
    }

    /// ✅ 失败变业务错误，不把 UB 推迟到 C 侧。
    fn path_to_cstring_safe(raw: &str) -> Result<CString, std::ffi::NulError> {
        CString::new(raw)
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：`CString::new` 含有 inner NUL → `Err`");
        let ok = path_to_cstring_safe("safe/path").unwrap();
        let bad = path_to_cstring_safe("bad\0path");
        println!("  ✅ 无 NUL → CString len = {}", ok.as_bytes().len());
        println!("  ❌ 含 NUL → {:?}", bad);
        println!(
            "  ❌ `.unwrap()` 版在 `bad\\0path` 上会 panic（`path_to_cstring_trap` 勿用）"
        );
        println!("规则：所有用户输入走 `Result`；老库只吃 `char*` 时先清洗编码\n");
    }
}

// ============================================================================
// 陷阱 4：`bool` / `enum` / `usize` ABI
// ============================================================================
/// **现象**：跨语言 struct 字段错位、枚举值对不上。
/// **根因**：Rust 默认 `enum` / `bool` 布局不保证与 C 一致。
/// **修法**：`#[repr(C)]`；`bool` 改 `u8` / `c_bool`；header typedef `size_t`。
pub mod abi_repr {
    use std::mem::size_of;

    enum RustSide {
        Bid,
        Ask,
    }

    #[repr(C)]
    enum CSide {
        Bid = 0,
        Ask = 1,
    }

    #[repr(C)]
    struct CWireFlags {
        is_buy: u8, // ✅ C 侧 1 字节
        _pad: [u8; 3],
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：`bool` / `enum` / `usize` ABI");
        println!(
            "  ❌ Rust 裸 enum size = {}（无 `repr(C)` 勿过 FFI 边）",
            size_of::<RustSide>()
        );
        println!(
            "  ✅ `#[repr(C)]` enum size = {}，Bid discriminant = {}",
            size_of::<CSide>(),
            CSide::Bid as i32
        );
        println!(
            "  ✅ 布尔语义用 u8 字段 size = {}（勿直接传 Rust `bool`）",
            size_of::<CWireFlags>()
        );
        println!(
            "  `usize` = {} 字节，header 层仍建议 typedef 与 `size_t` 对齐文档\n",
            size_of::<usize>()
        );
    }
}

// ============================================================================
// 陷阱 5：线程亲和与 `Send`
// ============================================================================
/// **现象**：多线程 poll 同一 vendor handle 偶发死锁 / 数据竞争。
/// **根因**：C API 文档要求「每线程一个 handle」，Rust `Arc` 不等于 `Send`。
/// **修法**：每线程 handle；或读写锁封装 vendor 文档约束。
pub mod thread_affinity {
    /// 模拟 vendor 返回的原始 handle —— **不实现 `Send`**。
    struct VendorNicHandle(*mut ());

    // ❌ 错误示范（勿复制）：
    // unsafe impl Send for VendorNicHandle {}

    /// ✅ 用 `!Send` 标记 + 单线程包装，编译期阻止跨线程传递。
    struct PerThreadVendor {
        _handle: VendorNicHandle,
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：线程亲和与 `Send`");
        println!(
            "  ❌ `Arc<VendorNicHandle>` + 多线程 poll —— vendor 文档禁止时仍不安全"
        );
        println!(
            "  ✅ `PerThreadVendor` 不 impl `Send`，迫使每线程 `create` 独立 handle"
        );
        let _nic = PerThreadVendor {
            _handle: VendorNicHandle(std::ptr::null_mut()),
        };
        println!("规则：读 vendor 头文件里的 thread-safety 段，再决定 Rust 封装形状\n");
    }
}

pub fn demonstrate() {
    dangling_vec_ptr::demonstrate();
    panic_across_ffi::demonstrate();
    cstring_inner_nul::demonstrate();
    abi_repr::demonstrate();
    thread_affinity::demonstrate();
}
