//! 场景：多线程读同一份数据（全局配置、只读 schema、预热缓存、worker 池共享任务模板）
//!
//! **权衡**
//! - `Arc<T>`：原子 refcount，比 `Rc` 贵；读多写少时仍常比“每线程 clone 一份大配置”划算。
//! - 需要**可变**共享：`Arc<Mutex<T>>` 粗锁简单但争用大；`Arc<RwLock<T>>` 读多写少更好；热点计数考虑原子类型。
//! - **泛化**：跨线程共享所有权 → `Arc`；只借不拥有 → 生命周期允许时用 `&` + `scope`/`channel` 传引用更轻。

use std::sync::Arc;
use std::thread;

pub fn demonstrate() {
    let config: Arc<Vec<&'static str>> = Arc::new(vec!["mode=prod", "region=ap"]);
    let mut handles = Vec::new();
    for id in 0..3 {
        let cfg = Arc::clone(&config);
        handles.push(thread::spawn(move || {
            format!("worker{id}: {:?}", cfg.as_slice())
        }));
    }
    let outs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for line in outs {
        println!("  {line}");
    }
    println!(
        "  全部 join 后 Arc strong_count={}（仅剩 main 里这一份）",
        Arc::strong_count(&config)
    );
}
