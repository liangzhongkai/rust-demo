//! # 所有权基础概念
//!
//! 展示所有权系统的核心规则和行为

/// 演示栈上数据的 Copy 行为
pub fn stack_copy() {
    println!("## 栈上数据（Copy）");

    let x = 5;
    let y = x; // Copy，x 仍然有效

    println!("x = {}, y = {}", x, y);
    println!("栈上类型实现了 Copy，赋值后原变量仍然有效\n");
}

/// 演示堆上数据的 Move 行为
pub fn heap_move() {
    println!("## 堆上数据（Move）");

    let s1 = String::from("hello");
    // String 内部结构：ptr (堆) + capacity + len (栈)

    let s2 = s1; // Move！s1 不再有效
    // 所有权转移：s1 的栈数据复制到 s2，堆数据的指针也复制
    // 但 s1 被标记为无效，防止 double free

    // println!("{}", s1); // 编译错误！value borrowed here after move
    println!("s2 = {}", s2);
    println!("堆上类型没有 Copy，赋值后所有权发生转移\n");
}

/// 演示函数调用中的所有权转移
pub fn function_ownership() {
    println!("## 函数调用中的所有权");

    let s = String::from("hello world");

    // 传递所有权到函数
    let len = calculate_length(s); // s 被移动进函数
    // s 不再有效

    println!("字符串长度: {}", len);
    // println!("{}", s); // 编译错误！

    println!("传参时，所有权被转移到函数参数\n");
}

/// 接收所有权的函数
fn calculate_length(s: String) -> usize {
    s.len()
    // s 在这里离开作用域，被 drop
    // 堆内存 "hello world" 被释放
}

/// 演示返回值的所有权转移
pub fn return_ownership() {
    println!("## 返回值的所有权");

    // 方式1：创建并返回
    let s1 = gives_ownership();
    println!("函数返回的所有权: s1 = {}", s1);

    // 方式2：传入并返回
    let s2 = String::from("hello");
    let s3 = takes_and_gives_back(s2);
    // s2 的所有权被移动进函数，然后被移动到 s3
    println!("传入并返回: s3 = {}", s3);

    println!("函数可以转移所有权进和出\n");
}

fn gives_ownership() -> String {
    let s = String::from("yours");
    s // s 被移动出，调用者获得所有权
}

fn takes_and_gives_back(mut s: String) -> String {
    s.push_str(" again");
    s // 所有权返回给调用者
}

/// 演示元组的"多重返回"技巧
pub fn tuple_pattern() {
    println!("## 元组模式：同时返回值和所有权");

    let s1 = String::from("hello");

    // 使用元组返回多个值
    let (s2, len) = calculate_length_with_return(s1);

    println!("字符串 '{}' 的长度是 {}", s2, len);
    println!("这是借用机制出现之前的常见模式，现在推荐使用引用\n");
}

fn calculate_length_with_return(s: String) -> (String, usize) {
    let length = s.len();
    (s, length) // 返回字符串和长度
}

/// 演示作用域和 Drop
pub fn scope_and_drop() {
    println!("## 作用域和 Drop");

    {
        let s = String::from("inner scope");
        println!("内部作用域: {}", s);
        // s 在这里被 drop，内存被释放
    }
    // s 不再可用

    let s = String::from("outer scope");
    println!("外部作用域: {}", s);
    // s 在这里被 drop

    println!("RAII（资源获取即初始化）模式自动管理资源\n");
}

/// 自定义 Drop trait
pub fn custom_drop() {
    println!("## 自定义 Drop trait");

    #[derive(Debug)]
    struct CustomDrop(i32);

    impl Drop for CustomDrop {
        fn drop(&mut self) {
            println!("Dropping CustomDrop({})", self.0);
        }
    }

    let _c1 = CustomDrop(1);
    let _c2 = CustomDrop(2);

    println!("创建两个实例");
    // Drop 按创建的相反顺序调用：c2 先 drop，然后 c1
    println!("离开作用域时，按相反顺序调用 Drop\n");
}

/// 演示变量的重新绑定
pub fn rebinding() {
    println!("## 变量的重新绑定");

    let x = 5;
    println!("x = {}", x);

    let x = x + 1; // 遮蔽（shadow），不是 move
    println!("x = {} (遮蔽)", x);

    let x = "now a string";
    println!("x = {} (类型改变)", x);

    let s = String::from("hello");
    let s = s; // 自赋值也触发 move！
    println!("s = {}", s);

    println!("遮蔽允许重用变量名，但类型可以改变\n");
}

/// 运行所有基础示例
pub fn demonstrate() {
    stack_copy();
    heap_move();
    function_ownership();
    return_ownership();
    tuple_pattern();
    scope_and_drop();
    custom_drop();
    rebinding();
}
