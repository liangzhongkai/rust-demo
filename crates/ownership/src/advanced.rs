//! # 所有权进阶特性
//!
//! 深入探讨部分移动、所有权优化等高级主题

#![allow(dead_code)]

use std::mem;

/// 演示部分移动（Partial Move）
pub fn partial_move() {
    println!("## 部分移动（Partial Move）");

    #[derive(Debug)]
    struct Person {
        name: String,
        age: u32,
        email: String,
    }

    let person = Person {
        name: String::from("Alice"),
        age: 30,
        email: String::from("alice@example.com"),
    };

    // 部分移动：只移动 name 字段
    let name = person.name;
    // person.name 不再有效
    // 但 person.age 和 person.email 仍然有效！

    println!("移动后的 name: {}", name);
    println!("仍然可以访问 person.age: {}", person.age);
    println!("仍然可以访问 person.email: {}", person.email);

    // println!("{:?}", person); // 错误！person.name 已被移动
    println!("部分移动后，整个结构体不能作为整体使用\n");
}

/// 演示通过 .clone() 避免移动
pub fn cloning_avoids_move() {
    println!("## Clone 避免移动");

    let s1 = String::from("hello");
    let s2 = s1.clone(); // 深拷贝

    println!("s1 = {}, s2 = {}", s1, s2);
    println!("Clone 创建堆数据的深拷贝，两个变量都有效\n");
}

/// 演示 Copy trait 的限制
pub fn copy_limitations() {
    println!("## Copy trait 的限制");

    #[derive(Debug, Copy, Clone)]
    struct Point {
        x: i32,
        y: i32,
    }

    let p1 = Point { x: 1, y: 2 };
    let p2 = p1; // Copy，p1 仍然有效

    println!("p1 = {:?}, p2 = {:?}", p1, p2);

    // 含有堆数据的结构体不能实现 Copy
    #[derive(Debug)]
    struct NotCopy {
        s: String, // String 有堆数据
    }

    let n1 = NotCopy {
        s: String::from("hello"),
    };
    let _n2 = n1; // Move
    // println!("{:?}", n1); // 错误！

    println!("Copy 只适用于可以按位复制的类型\n");
}

/// 演示 RVO（返回值优化）
pub fn return_value_optimization() {
    println!("## 返回值优化（RVO / NRVO）");

    // 现代 Rust 会优化返回值的移动
    let s = create_string();
    println!("创建的字符串: {}", s);

    // 编译器直接在调用者的位置构造对象
    // 避免了不必要的移动

    println!("编译器会优化返回值的移动，直接在目标位置构造\n");
}

fn create_string() -> String {
    let s = String::from("optimized");
    s // 编译器可能直接在调用者的栈槽构造 s
}

/// 演示所有权与循环
pub fn ownership_in_loops() {
    println!("## 循环中的所有权");

    let strings = vec![
        String::from("one"),
        String::from("two"),
        String::from("three"),
    ];

    // for 循环获取向量的所有权并迭代
    for s in strings {
        // 每次迭代，向量的一个元素被移动到 s
        println!("{}", s);
        // s 在这里被 drop
    }
    // strings 在这里不可用，所有权已在循环中转移

    println!("迭代器会移动元素的所有权\n");

    // 使用引用避免移动
    let strings = vec![
        String::from("one"),
        String::from("two"),
        String::from("three"),
    ];

    for s in &strings {
        // s 是 &String，没有所有权转移
        println!("{}", s);
    }
    // strings 仍然可用
    println!("使用 & 创建借用迭代器，保持所有权\n");
}

/// 演示闭包捕获的所有权
pub fn closure_capture() {
    println!("## 闭包的所有权捕获");

    let s = String::from("hello");

    // FnOnce：消耗捕获的变量
    let consume = || {
        println!("Consuming: {}", s);
        // s 在闭包内被移动
        // drop(s); // 显式 drop
    };
    consume();
    // consume(); // 错误！只能调用一次（FnOnce）
    // s 不再可用

    let mut list = vec![1, 2, 3];
    // FnMut：可变借用
    let _mut_capture = || {
        list.push(4); // 闭包捕获了可变引用
        // 注意：这个闭包实际无法调用，因为 immutable_capture 已经持有了 list 的不可变借用
        // 这是一个借用冲突的示例
    };

    // Fn：不可变借用
    let immutable_capture = || {
        println!("Immutable: {:?}", list.len());
    };

    immutable_capture();
    immutable_capture(); // 可以多次调用
    println!("Fn 闭包可以多次调用\n");
}

/// 演示 Copy 类型在闭包中的行为
pub fn copy_in_closures() {
    println!("## Copy 类型在闭包中");

    let x = 5;

    // Copy 类型被复制到闭包
    let print_x = || {
        println!("x = {}", x);
    };

    print_x();
    print_x();
    println!("x 仍然有效: {}", x);

    println!("Copy 类型在闭包捕获时被复制\n");
}

/// 演示 move 关键字
pub fn move_keyword() {
    println!("## move 关键字");

    let s = String::from("hello");

    // move 强制闭包获取捕获值的所有权
    let moved_closure = move || {
        println!("Moved: {}", s);
    };

    moved_closure();

    // s 不再可用，即使闭包只读取它
    // println!("{}", s); // 错误！

    println!("move 关键字强制转移所有权到闭包\n");
}

/// 演示所有权与线程
pub fn ownership_with_threads() {
    println!("## 线程间的所有权转移");

    let s = String::from("hello");

    // move 强制将所有权转移到新线程
    let handle = std::thread::spawn(move || {
        println!("From thread: {}", s);
        // s 在这个线程结束时被 drop
    });

    handle.join().unwrap();

    // s 不再可用
    // println!("{}", s); // 错误！

    println!("线程必须拥有捕获的数据，因为父线程可能先结束\n");
}

/// 演示 mem::take 和 mem::replace
pub fn take_and_replace() {
    println!("## mem::take 和 mem::replace");

    #[derive(Debug)]
    struct Buffer {
        data: Vec<u8>,
    }

    impl Buffer {
        fn new() -> Self {
            Buffer {
                data: vec![1, 2, 3, 4, 5],
            }
        }

        fn clear(&mut self) {
            // 使用 take 取出值，留下默认值
            let old_data = mem::take(&mut self.data);
            println!("取出的数据: {:?}", old_data);
            // self.data 现在是空的 Vec
        }

        fn replace_with(&mut self, new_data: Vec<u8>) -> Vec<u8> {
            mem::replace(&mut self.data, new_data)
        }
    }

    let mut buf = Buffer::new();
    println!("原始 buffer: {:?}", buf);

    buf.clear();
    println!("clear 后: {:?}\n", buf);

    let old = buf.replace_with(vec![10, 20]);
    println!("被替换的数据: {:?}", old);
    println!("新 buffer: {:?}\n", buf);
}

/// 运行所有进阶示例
pub fn demonstrate() {
    partial_move();
    cloning_avoids_move();
    copy_limitations();
    return_value_optimization();
    ownership_in_loops();
    closure_capture();
    copy_in_closures();
    move_keyword();
    ownership_with_threads();
    take_and_replace();
}
