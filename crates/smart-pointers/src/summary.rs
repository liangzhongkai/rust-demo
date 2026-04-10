//! 将上述场景压缩成决策备忘（供 main 打印）

pub fn print_decision_cheat_sheet() {
    println!("\n  ┌─ 智能指针与共享策略备忘 ───────────────────────────────────");
    println!("  │ 递归/大对象+单所有者     → Box；dyn Trait 胖指针也常配合 Box");
    println!("  │ 单线程多所有者+只读/共享 → Rc；可变 → Rc<RefCell<_>>（注意 panic 借用规则）");
    println!("  │ 跨线程共享所有权         → Arc；可变 → Mutex / RwLock / 原子（按争用选）");
    println!("  │ 环引用 / 可选关联        → 一侧用 Weak；缓存不延长寿命 → Weak");
    println!("  │ 接口要兼容纳借用与拥有   → Cow（热路径读多、写少时划算）");
    println!("  │ 能明确生命周期且只读     → 优先 & / 结构化并发传引用，避免多余 Arc");
    println!("  └──────────────────────────────────────────────────────────────");
}
