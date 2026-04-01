//! 场景：API 同时接受借用与拥有（日志前缀、路由规范化、序列化层“可能零拷贝”）
//!
//! **权衡**
//! - `Cow<'a, T>`：`Borrowed` 不分配；需要独占修改时 `to_mut()` 才 clone（clone-on-write）。
//! - 比“一律 `String`”省分配；比“只收 `&str`”更灵活（调用方可传拥有数据）。
//! - **泛化**：读路径热、写路径少 → `Cow`；永远要修改 → 直接 `String`/`Vec`；只读且生命周期清晰 → `&str`/`&[T]`。

use std::borrow::Cow;

fn normalize_path<'a>(input: Cow<'a, str>) -> Cow<'a, str> {
    if input.contains('\\') {
        Cow::Owned(input.replace('\\', "/"))
    } else {
        input
    }
}

pub fn demonstrate() {
    let borrowed = normalize_path(Cow::Borrowed("a/b/c"));
    println!("  借用路径无反斜杠: {borrowed}（未分配新 String）");

    let owned = normalize_path(Cow::Owned(String::from(r"a\b\c")));
    println!("  含反斜杠时 COW 写分支: {owned}");
}
