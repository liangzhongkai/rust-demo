//! # 所有权常见陷阱
//!
//! 展示常见的所有权错误和如何诊断修复

#![allow(dead_code)]

/// 陷阱1：在循环中意外移动
pub fn pitfall_loop_move() {
    println!("## 陷阱1：循环中的意外移动");

    let strings = vec![
        String::from("one"),
        String::from("two"),
        String::from("three"),
    ];

    // 错误示例（已注释）
    // for s in strings {
    //     if s.len() > 3 {
    //         // 继续处理 s
    //     }
    //     // s 在这里被 drop
    // }
    // // strings 不再可用！

    println!("错误：for 循环消耗了向量，后续无法使用");
    println!("解决方案：使用 &strings 借用\n");

    // 正确做法
    for s in &strings {
        if s.len() > 3 {
            println!("长字符串: {}", s);
        }
    }
    println!("strings 仍然可用: {:?}\n", strings);
}

/// 陷阱2：收集器中的移动
pub fn pitfall_collector() {
    println!("## 陷阱2：迭代器收集");

    let strings = vec![
        String::from("one"),
        String::from("two"),
        String::from("three"),
    ];

    // map 闭包移动了所有权
    // let lengths: Vec<_> = strings.iter().map(|s| s.len()).collect();

    // 如果要同时保留原数据和结果
    let lengths: Vec<_> = strings.iter().map(|s| s.len()).collect();
    println!("字符串: {:?}, 长度: {:?}", strings, lengths);

    println!("使用 iter() 创建借用迭代器，避免移动\n");
}

/// 陷阱3：Option 的移动
pub fn pitfall_option() {
    println!("## 陷阱3：Option 的移动");

    let maybe_string = Some(String::from("hello"));

    // 错误：unwrap 移动了值
    // let s = maybe_string.unwrap();
    // println!("{:?}", maybe_string); // 错误！

    // 解决方案1：使用 as_ref()
    let s = maybe_string.as_ref().unwrap();
    println!("使用 as_ref: {}", s);
    println!("Option 仍然可用: {:?}\n", maybe_string);

    // 解决方案2：先检查再使用
    if let Some(s) = &maybe_string {
        println!("使用 if let: {}", s);
    }
    println!("Option 仍然可用: {:?}\n", maybe_string);
}

/// 陷阱4：部分移动导致整体不可用
pub fn pitfall_partial_move() {
    println!("## 陷阱4：部分移动的影响");

    #[derive(Debug)]
    struct Data {
        important: String,
        metadata: String,
    }

    let data = Data {
        important: String::from("critical"),
        metadata: String::from("info"),
    };

    let _important = data.important;
    // data.important 被移动
    // 整个 data 不能再使用

    // println!("{:?}", data); // 错误！
    // 但 data.metadata 仍然可用
    println!("metadata 仍然可用: {}", data.metadata);

    println!("解决方案：先借用，再决定是否移动");
    let data = Data {
        important: String::from("critical"),
        metadata: String::from("info"),
    };

    // 只借用
    println!("借用 important: {}", data.important);
    println!("整个 data 仍可用: {:?}\n", data);
}

/// 陷阱5：闭包的 FnOnce 限制
pub fn pitfall_fn_once() {
    println!("## 陷阱5：FnOnce 闭包只能调用一次");

    let data = vec![1, 2, 3];

    // 闭包移动了 data
    let process = || {
        println!("Processing: {:?}", data);
        data
    };

    process(); // 第一次调用 OK
    // process(); // 第二次调用错误！

    println!("FnOnce 闭包消耗了捕获的值");
    println!("解决方案：不移动或使用引用\n");

    // 使用引用
    let data = vec![1, 2, 3];
    let process_ref = || {
        println!("Processing: {:?}", data);
    };

    process_ref();
    process_ref(); // 可以多次调用
    println!("使用引用的闭包可以多次调用\n");
}

/// 陷阱6：match 中的移动
pub fn pitfall_match_move() {
    println!("## 陷阱6：Match 中的移动");

    let maybe_string = Some(String::from("hello"));

    // match 移动了值
    match maybe_string {
        Some(s) => {
            println!("Got: {}", s);
            // s 在这里被 drop
        }
        None => {}
    }

    // maybe_string 不再可用
    // println!("{:?}", maybe_string); // 错误！

    println!("解决方案：使用引用匹配");
    let maybe_string = Some(String::from("hello"));

    match &maybe_string {
        Some(s) => {
            println!("Got: {}", s);
        }
        None => {}
    }

    println!("Option 仍可用: {:?}\n", maybe_string);
}

/// 陷阱7：结构体更新语法的移动
pub fn pitfall_struct_update() {
    println!("## 陷阱7：结构体更新语法");

    #[derive(Debug)]
    struct Config {
        host: String,
        port: u16,
        debug: bool,
    }

    let base = Config {
        host: String::from("localhost"),
        port: 8080,
        debug: false,
    };

    // ..base 移动了剩余字段
    let _modified = Config {
        port: 9090,
        ..base // base 被部分移动
    };

    // base 不再可用
    // println!("{:?}", base); // 错误！

    println!("..base 移动了整个 base");
    println!("解决方案：先克隆再更新\n");

    let base = Config {
        host: String::from("localhost"),
        port: 8080,
        debug: false,
    };

    let _modified = Config {
        port: 9090,
        host: base.host.clone(),
        ..base
    };

    println!("base: {:?}", base);
    println!("modified: Config {{ host: \"{}\", port: 9090, debug: {} }}\n", base.host, base.debug);
}

/// 陷阱8：在错误处理中丢失值
pub fn pitfall_error_handling() {
    println!("## 陷阱8：错误处理中的移动");

    fn process(value: String) -> Result<String, String> {
        if value.is_empty() {
            Err(String::from("empty value"))
        } else {
            Ok(value)
        }
    }

    let input = String::from("hello");

    // 错误处理移动了值
    let result = process(input);
    // input 不再可用

    match result {
        Ok(s) => println!("Success: {}", s),
        Err(e) => println!("Error: {}", e),
    }

    println!("值被移动到函数，错误时无法访问");
    println!("解决方案：传递引用\n");

    fn process_ref(value: &str) -> Result<(), String> {
        if value.is_empty() {
            Err(String::from("empty value"))
        } else {
            Ok(())
        }
    }

    let input = String::from("hello");
    let result = process_ref(&input);

    match result {
        Ok(()) => println!("Success, input still available: {}", input),
        Err(e) => println!("Error: {}, input: {}", e, input),
    }
    println!();
}

/// 陷阱9：集合的 drain
pub fn pitfall_drain() {
    println!("## 陷阱9：Drain 与移动");

    let mut vec = vec![1, 2, 3, 4, 5];

    // drain 迭代器移动元素
    let drained: Vec<_> = vec.drain(1..3).collect();
    println!("Drained: {:?}", drained);
    println!("Original after drain: {:?}", vec);

    println!("drain 消耗了元素，但保留了容器");
    println!("注意：drained 的元素被移动出来\n");
}

/// 陷阱10：Self 的移动
pub fn pitfall_self_move() {
    println!("## 陷阱10：Self 移动");

    struct Builder {
        data: String,
    }

    impl Builder {
        fn new() -> Self {
            Builder {
                data: String::new(),
            }
        }

        fn build(self) -> String {
            // self 被移动，调用者失去所有权
            self.data
        }
    }

    let mut builder = Builder::new();
    builder.data = String::from("hello");

    // build 移动了 builder
    let result = builder.build();
    println!("Build result: {}", result);
    // builder 不再可用

    println!("这是建造者模式的常见用法");
    println!("强制使用者按顺序调用方法\n");
}

/// 运行所有陷阱示例
pub fn demonstrate() {
    pitfall_loop_move();
    pitfall_collector();
    pitfall_option();
    pitfall_partial_move();
    pitfall_fn_once();
    pitfall_match_move();
    pitfall_struct_update();
    pitfall_error_handling();
    pitfall_drain();
    pitfall_self_move();
}
