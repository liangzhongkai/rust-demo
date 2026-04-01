//! 场景：递归结构、大对象只走单所有权路径（AST、深度嵌套 JSON DOM、插件 `dyn Trait`）
//!
//! **权衡**
//! - `Box<T>`：在堆上固定大小，栈上只留指针；适合递归类型与“已知唯一所有者”的大块数据。
//! - 与 `Vec`/栈数组对比：过深的栈递归会溢出；`Box` 把层级摊到堆上。
//! - **泛化**：凡“编译期大小未知或递归”且**不需要共享**，优先 `Box`；需要共享再看 `Rc`/`Arc`。

#[allow(dead_code)]
enum IntList {
    Nil,
    Cons(i32, Box<IntList>),
}

pub fn demonstrate() {
    let list = IntList::Cons(
        1,
        Box::new(IntList::Cons(2, Box::new(IntList::Cons(3, Box::new(IntList::Nil))))),
    );
    let sum = sum_list(&list);
    println!("  Box 递归链表求和: sum={sum}（单所有权，无共享）");

    let big_on_heap: Box<[u8; 4096]> = Box::new([0u8; 4096]);
    println!(
        "  大块只传指针: Box 在栈上占 {} 字节（仅指针+元数据），数据在堆",
        std::mem::size_of_val(&big_on_heap)
    );
}

fn sum_list(node: &IntList) -> i32 {
    match node {
        IntList::Nil => 0,
        IntList::Cons(x, rest) => x + sum_list(rest),
    }
}
