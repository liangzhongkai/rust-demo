//! 决策备忘：将场景压缩成可扫一眼的清单

pub fn print_decision_cheat_sheet() {
    println!("\n  ┌─ Lifetimes：场景与泛化策略 ────────────────────────────────");
    println!("  │ 热路径解析、wire/frame buffer      → View<'buf> 零拷贝");
    println!("  │ tick/block/sim 内临时对象          → Arena；整块 drop");
    println!("  │ 借还是 clone 不确定                → Cow<'a, T>");
    println!("  │ 跨线程 / channel / async spawn       → 边界 Arc/Vec/String");
    println!("  │ 多种短借输入的回调                 → HRTB for<'a> Fn(&'a T)");
    println!("  │ 配置 / 符号 / ABI 长寿命           → &'static / intern / OnceLock");
    println!("  │ 解析 vs 持久化                     → 两阶段 parse → commit");
    println!("  │ 缓存                               → 存 owned；get 时临时 &str");
    println!("  │ 切忌                               → 返回局部 &；cache 借来的 slice");
    println!("  │ 自引用                             → 索引替代；或 Pin+unsafe");
    println!("  └──────────────────────────────────────────────────────────────");
}
