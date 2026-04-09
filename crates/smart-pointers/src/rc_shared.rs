//! 场景：单线程内多所有者（配置树、共享子树、UI 文档模型、解析器里的共享子表达式）
//!
//! **权衡**
//! - `Rc<T>`：共享**不可变**数据时 refcount 便宜；要改内容需 `Rc<RefCell<T>>` 或 `Rc<Mutex<T>>`（后者多线程）。
//! - `Rc` 默认不 `Send`，不能跨线程传裸 `Rc`（跨线程用 `Arc`）。
//! - **循环引用**会泄漏：`A` 持 `Rc<B>`，`B` 持 `Rc<A>` → 永不清零；一边改成 `Weak` 打破环。
//! - **泛化**：单线程共享读 → `Rc`；共享写 + 可预测借用 → `RefCell`；跨线程 → `Arc` + `Mutex`/`RwLock`。

use std::cell::RefCell;
use std::rc::{Rc, Weak};

pub fn demonstrate() {
    let shared_expr = Rc::new("subexpr".to_string());
    let owner_a = Rc::clone(&shared_expr);
    let owner_b = Rc::clone(&shared_expr);
    println!(
        "  两处共享同一子树: strong_count={}",
        Rc::strong_count(&shared_expr)
    );
    drop(owner_a);
    println!(
        "  drop 一处后: strong_count={}",
        Rc::strong_count(&shared_expr)
    );
    drop(owner_b);
    drop(shared_expr);

    // 父子图：子用 Weak 指父，避免 Rc 环泄漏
    struct Node {
        name: String,
        parent: RefCell<Weak<Node>>,
    }
    let root = Rc::new(Node {
        name: "root".into(),
        parent: RefCell::new(Weak::new()),
    });
    let child = Rc::new(Node {
        name: "child".into(),
        parent: RefCell::new(Rc::downgrade(&root)),
    });
    *child.parent.borrow_mut() = Rc::downgrade(&root);
    println!(
        "  Weak 指父: child→parent name = {:?}",
        child.parent.borrow().upgrade().map(|p| p.name.clone())
    );
    drop(root);
    println!(
        "  drop root 后 child.upgrade(parent) = {:?}",
        child.parent.borrow().upgrade().map(|p| p.name.clone())
    );
}
