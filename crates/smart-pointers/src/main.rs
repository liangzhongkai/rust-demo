//! # 智能指针：生产向场景与权衡
//!
//! 本 crate 用标准库类型演示常见用法，并把具体问题抽象成可复用的应对策略。
//! 运行：`cargo run -p smart-pointers`

mod arc_threads;
mod box_owned;
mod cow_api;
mod rc_shared;
mod summary;
mod weak_cache;

fn main() {
    println!("=== Smart pointers：场景、权衡与泛化策略 ===\n");

    println!("--- 1. Box：递归结构 / 大块单所有权（AST、堆上固定布局） ---");
    box_owned::demonstrate();

    println!("\n--- 2. Rc + Weak：单线程共享与打破父子环（文档树、共享子表达式） ---");
    rc_shared::demonstrate();

    println!("\n--- 3. Arc：跨线程共享只读数据（配置、模板） ---");
    arc_threads::demonstrate();

    println!("\n--- 4. Weak：不延长主体生命（缓存槽、弱订阅） ---");
    weak_cache::demonstrate();

    println!("\n--- 5. Cow：借用与拥有统一 API（路径规范化、零拷贝读路径） ---");
    cow_api::demonstrate();

    summary::print_decision_cheat_sheet();
}
