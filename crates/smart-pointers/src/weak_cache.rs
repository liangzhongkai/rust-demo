//! 场景：缓存/索引不应“钉死”被引用对象（缩略图缓存、弱引用表、事件总线订阅者）
//!
//! **权衡**
//! - `Weak<T>`：不增加强引用计数；`upgrade()` 可能为 `None`（对象已释放）——调用方必须处理。
//! - 适合打破 `Rc`/`Arc` 环，或“有则取、无则重建”的二级缓存。
//! - **泛化**：**可选、可丢弃**的关联 → `Weak`；**必须随主体存活** → `Rc`/`Arc` 强引用。

use std::sync::{Arc, Weak};

pub fn demonstrate() {
    let resource = Arc::new("expensive-db-handle".to_string());
    let cache_slot: Weak<String> = Arc::downgrade(&resource);
    println!(
        "  缓存 Weak upgrade: {:?}",
        cache_slot.upgrade().as_deref()
    );
    drop(resource);
    println!(
        "  释放主体后 upgrade: {:?}（须回源或重建）",
        cache_slot.upgrade()
    );
}
