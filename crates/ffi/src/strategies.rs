//! # 泛化：从 HFT / Web3 FFI 到通用应对策略
//!
//! 把前两章的具体封装抽象成一张 **「故障类型 → 首选对策」** 决策矩阵，
//! 便于在任何带 `extern "C"` 的项目里做 code review checklist。
//!
//! | # | 故障类型 | HFT 锚例 | Web3 锚例 | 首选对策 |
//! |---|-----------|-----------|-----------|-----------|
//! | 1 | 裸指针生命周期 | vendor 缓存 `src` | UDS 读缓冲复用 | Rust 侧 **拥有** buffer；native 仅短期借用 |
//! | 2 | 输出缓冲未初始化 | DMA 描述符填充 | digest / signature out | `MaybeUninit` + 写满再 `assume_init` |
//! | 3 | opaque handle 泄漏 | NIC completion queue | wallet session | `into_raw`/`from_raw` 文档化配对；计数断言 |
//! | 4 | ABI / repr | packed wire vs host | ABI-encoded tuple | `repr(C)`；显式 endian；bindgen + `-sys` crate |
//! | 5 | 阻塞 FFI + async | sync decode on Tokio | `web3_*` RPC | `spawn_blocking` + deadline |
//! | 6 | 并发契约不明 | multi-queue NIC | lib 内部全局锁 | 每线程 handle；队列边界单一 writer |
//! | 7 | 观测黑洞 | stall 无日志 | RPC hang | 边界返回 `Result`/errno；tracing span |
//!
//! 下列工具函数刻意 **不带领域名词**，可直接复制到其他 crate。

#![allow(dead_code)]

use std::ffi::CString;
use std::time::{Duration, Instant};

/// **策略 1**：把「可能含 NUL 的路径字符串」转成 C API 接受的形态。
///
/// - `Ok(CString)`：可安全传给 `extern "C"`
/// - `Err`：调用方降级（替换字符、改用 fd、换 API）
///
/// Web3：某些老旧 signer CLI；HFT：嵌入式配置文件路径。
pub fn path_to_cstring_lossy(raw: &str) -> Result<CString, std::ffi::NulError> {
    CString::new(raw.as_bytes())
}

/// **策略 2**：对「已知可能阻塞」的 FFI 封装超时语义（示意）。
///
/// 真实生产：应在线程池任务里调用并在超时后 **放弃 future**，同时评估 native 侧是否仍需 teardown。
pub fn ffi_with_deadline<F, T>(max_wait: Duration, f: F) -> Result<T, &'static str>
where
    F: FnOnce() -> T,
{
    let start = Instant::now();
    let out = f();
    if start.elapsed() > max_wait {
        return Err("ffi exceeded deadline (wall-clock heuristic)");
    }
    Ok(out)
}

pub fn demonstrate() {
    println!("## 泛化策略：路径 → CString（失败显式）");
    match path_to_cstring_lossy("safe/path/no-nul") {
        Ok(cs) => println!("CString len = {}", cs.as_bytes().len()),
        Err(_) => unreachable!(),
    }
    assert!(path_to_cstring_lossy("bad\0path").is_err());
    println!("含 NUL 的路径被拒绝 —— 不把 UB 推迟到 C 侧\n");

    println!("## 泛化策略：FFI + deadline（示意）");
    let ok = ffi_with_deadline(Duration::from_secs(1), || 42).unwrap();
    println!("deadline 包裹返回值 = {}\n", ok);

    println!("## Review checklist（拷贝到 PR 模板）");
    println!(
        "- [ ] 所有 `unsafe extern \"C\"` 块旁是否有 `# Safety` / 契约段落？\n\
         - [ ] 缓冲区所有权箭头是否 **单向**（Rust→C 或 C→Rust）且无双向别名？\n\
         - [ ] panic 策略是否与 `-sys` crate 一致（abort vs catch_unwind）？\n\
         - [ ] Miri / sanitizers / fuzz 至少覆盖解码路径中的一条？\n"
    );
}
