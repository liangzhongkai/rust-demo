//! 场景：线上排查线程卡死 / CPU 飙高时需要可读线程名（profiler、jstack 风格）
//!
//! **权衡**
//! - `ThreadBuilder::name` 在调试工具里可见；名称长度在部分平台有限制。
//! - 与业务 ID 结合时要避免敏感信息写进线程名。

use std::thread;

pub fn demonstrate() {
    let h = thread::Builder::new()
        .name("worker-parse-chunk-3".into())
        .spawn(|| {
            // 实际工作省略
        })
        .expect("spawn named thread");

    println!("  已 spawn 命名线程: {:?}", h.thread().name());
    h.join().unwrap();
    println!("  → 泛化：可观测性（命名、tracing span）与性能开销取平衡。");
}
