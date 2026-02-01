//! # 所有权在实际场景中的应用
//!
//! 展示所有权在真实代码中的应用模式

#![allow(dead_code)]

/// 实际场景1：构建器模式（Builder Pattern）
pub mod builder_pattern {
    #[derive(Debug)]
    pub struct HttpRequest {
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<String>,
    }

    pub struct RequestBuilder {
        request: Option<HttpRequest>,
    }

    impl RequestBuilder {
        pub fn new() -> Self {
            Self {
                request: Some(HttpRequest {
                    method: "GET".to_string(),
                    url: String::new(),
                    headers: Vec::new(),
                    body: None,
                }),
            }
        }

        pub fn method(mut self, method: impl Into<String>) -> Self {
            if let Some(req) = &mut self.request {
                req.method = method.into();
            }
            self
        }

        pub fn url(mut self, url: impl Into<String>) -> Self {
            if let Some(req) = &mut self.request {
                req.url = url.into();
            }
            self
        }

        pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
            if let Some(req) = &mut self.request {
                req.headers.push((key.into(), value.into()));
            }
            self
        }

        pub fn body(mut self, body: impl Into<String>) -> Self {
            if let Some(req) = &mut self.request {
                req.body = Some(body.into());
            }
            self
        }

        // 消费 self，返回构建的对象
        pub fn build(mut self) -> HttpRequest {
            self.request.take().expect("build called twice")
        }
    }

    pub fn demonstrate() {
        println!("## 实际场景1：构建器模式");

        let request = RequestBuilder::new()
            .method("POST")
            .url("https://api.example.com/users")
            .header("Content-Type", "application/json")
            .header("Authorization", "Bearer token")
            .body(r#"{"name":"Alice"}"#)
            .build();

        println!("构建的请求: {:?}", request);
        println!("优点：链式调用，类型安全，build 后无法修改\n");
    }
}

/// 实际场景2：类型状态模式（Typestate Pattern）
pub mod typestate_pattern {
    pub struct Connection;

    pub struct Disconnected;
    pub struct Connected;
    pub struct Authenticated;

    pub struct ConnectionState<S> {
        _marker: std::marker::PhantomData<S>,
        connection: Option<Connection>,
    }

    impl ConnectionState<Disconnected> {
        pub fn new() -> Self {
            Self {
                _marker: std::marker::PhantomData,
                connection: Some(Connection),
            }
        }

        // 只有在 Disconnected 状态才能连接
        pub fn connect(mut self) -> Result<ConnectionState<Connected>, String> {
            println!("连接到服务器...");
            Ok(ConnectionState {
                _marker: std::marker::PhantomData,
                connection: self.connection.take(),
            })
        }
    }

    impl ConnectionState<Connected> {
        // 只有在 Connected 状态才能认证
        pub fn authenticate(
            mut self,
            token: &str,
        ) -> Result<ConnectionState<Authenticated>, String> {
            println!("使用 token {} 认证...", token);
            Ok(ConnectionState {
                _marker: std::marker::PhantomData,
                connection: self.connection.take(),
            })
        }

        pub fn disconnect(mut self) -> ConnectionState<Disconnected> {
            println!("断开连接");
            ConnectionState {
                _marker: std::marker::PhantomData,
                connection: self.connection.take(),
            }
        }
    }

    impl ConnectionState<Authenticated> {
        // 只有在 Authenticated 状态才能执行操作
        pub fn send_request(&self, data: &str) {
            println!("发送请求: {}", data);
        }

        pub fn logout(mut self) -> ConnectionState<Connected> {
            println!("退出登录");
            ConnectionState {
                _marker: std::marker::PhantomData,
                connection: self.connection.take(),
            }
        }
    }

    pub fn demonstrate() {
        println!("## 实际场景2：类型状态模式");

        // 编译时强制状态转换顺序
        let conn = ConnectionState::<Disconnected>::new();
        let conn = conn.connect().unwrap();
        let conn = conn.authenticate("secret").unwrap();
        conn.send_request("GET /api/data");

        // conn.send_request("again"); // 如果移动到这里会编译错误
        // 编译器确保操作在正确的状态下执行
        println!("编译时强制执行正确的状态转换顺序\n");
    }
}

/// 实际场景3：零拷贝解析
pub mod zero_copy_parsing {
    /// 解析结果只是对输入字符串的切片
    pub struct ParsedCommand<'a> {
        pub name: &'a str,
        pub args: Vec<&'a str>,
    }

    pub fn parse(input: &str) -> Option<ParsedCommand<'_>> {
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        Some(ParsedCommand {
            name: parts[0],
            args: parts[1..].to_vec(),
        })
    }

    pub fn demonstrate() {
        println!("## 实际场景3：零拷贝解析");

        let input = String::from("LOAD file.dat 1024");
        let command = parse(&input).unwrap();

        println!("命令名: {}", command.name);
        println!("参数: {:?}", command.args);
        println!("input 仍然有效: {}", input);

        println!("零拷贝：只借用原始字符串，不分配新内存\n");
    }
}

/// 实际场景4：向量重分配与指针失效
pub mod vec_reallocation {
    pub fn demonstrate() {
        println!("## 实际场景4：向量重分配");

        let mut vec = Vec::with_capacity(4);
        vec.push(1);
        vec.push(2);
        vec.push(3);
        vec.push(4);

        // 引用第一个元素
        let first = &vec[0];
        let _last_ptr = vec.as_ptr();

        println!("容量: {}, 长度: {}", vec.capacity(), vec.len());
        println!("第一个元素: {}", first);

        // 如果 push 超过容量，向量会重新分配
        // vec.push(5); // 错误！不能在持有引用时修改

        // 引用必须先失效
        println!("引用失效后可以继续添加");
        // 现在 push 是安全的
        vec.push(5);

        println!("新的容量: {}", vec.capacity());
        println!("所有元素: {:?}", vec);

        println!("借用规则防止悬垂指针\n");
    }
}

/// 实际场景5：Rc 和共享所有权
pub mod shared_ownership {
    use std::rc::Rc;

    #[derive(Debug)]
    struct Node {
        value: i32,
        // 在后面的智能指针章节会详细讨论
    }

    pub fn demonstrate() {
        println!("## 实际场景5：共享所有权场景");

        // 当需要多个所有者时，使用 Rc
        let data = Rc::new(vec![1, 2, 3, 4, 5]);

        println!("原始引用计数: {}", Rc::strong_count(&data));

        let data2 = Rc::clone(&data);
        let data3 = Rc::clone(&data);

        println!("克隆后引用计数: {}", Rc::strong_count(&data));

        println!("通过任一引用访问: {:?}", data);
        println!("通过任一引用访问: {:?}", data2);
        println!("通过任一引用访问: {:?}", data3);

        println!("Rc 允许多个所有者，但只适用于单线程\n");
    }
}

/// 实际场景6：所有权的性能优化
pub mod performance_optimization {
    use std::time::Instant;

    pub fn demonstrate() {
        println!("## 实际场景6：所有权性能影响");

        const N: usize = 1_000_000;

        // 移动的代价很小（只是复制指针）
        let start = Instant::now();
        let mut s = String::from("hello world");
        for _ in 0..N {
            s = move_and_return(s);
        }
        let move_time = start.elapsed();

        // 克隆的代价大（复制堆数据）
        let start = Instant::now();
        let mut s = String::from("hello world");
        for _ in 0..N {
            s = clone_and_return(&s);
        }
        let clone_time = start.elapsed();

        println!("Move {} 次: {:?}", N, move_time);
        println!("Clone {} 次: {:?}", N, clone_time);
        println!("Move 比 Clone 快得多");

        println!("合理使用所有权可以避免不必要的克隆\n");
    }

    fn move_and_return(s: String) -> String {
        s
    }

    fn clone_and_return(s: &String) -> String {
        s.clone()
    }
}

/// 实际场景7：通过所有权强制 API 使用顺序
pub mod api_enforcement {
    pub struct FileWriter;

    pub struct Opened;
    pub struct Closed;

    pub struct File<S> {
        _state: std::marker::PhantomData<S>,
        path: String,
    }

    impl File<Closed> {
        pub fn open(path: impl Into<String>) -> File<Opened> {
            let path = path.into();
            println!("打开文件: {}", path);
            File {
                _state: std::marker::PhantomData,
                path,
            }
        }
    }

    impl File<Opened> {
        pub fn write(&self, data: &str) {
            println!("写入: {}", data);
        }

        pub fn close(self) -> File<Closed> {
            println!("关闭文件");
            File {
                _state: std::marker::PhantomData,
                path: self.path,
            }
        }
    }

    pub fn demonstrate() {
        println!("## 实际场景7：API 使用顺序强制");

        let file = File::<Closed>::open("test.txt");
        file.write("Hello, World!");
        let _file = file.close();

        // file.write("again"); // 编译错误！已关闭
        println!("类型系统强制正确的资源使用顺序\n");
    }
}

/// 实际场景8：所有权与错误恢复
pub mod error_recovery {
    pub struct Transaction {
        operations: Vec<String>,
    }

    impl Transaction {
        pub fn new() -> Self {
            Self {
                operations: Vec::new(),
            }
        }

        pub fn add_operation(&mut self, op: String) {
            self.operations.push(op);
        }

        // 消费 self，返回结果
        pub fn commit(self) -> Result<String, String> {
            if self.operations.is_empty() {
                return Err("No operations to commit".to_string());
            }
            let result = format!("Committed {} operations", self.operations.len());
            println!("Transaction committed");
            Ok(result)
        }

        // 或者允许回滚
        pub fn rollback(mut self) -> Vec<String> {
            let ops = std::mem::take(&mut self.operations);
            println!("Transaction rolled back");
            ops
        }
    }

    pub fn demonstrate() {
        println!("## 实际场景8：错误恢复");

        let mut tx = Transaction::new();
        tx.add_operation("INSERT".to_string());
        tx.add_operation("UPDATE".to_string());

        // 可以提交或回滚，但不能两者都做
        match tx.commit() {
            Ok(msg) => println!("{}", msg),
            Err(e) => println!("Error: {}", e),
        }

        println!("消费所有权防止重复操作\n");
    }
}

/// 运行所有实际场景示例
pub fn demonstrate() {
    builder_pattern::demonstrate();
    typestate_pattern::demonstrate();
    zero_copy_parsing::demonstrate();
    vec_reallocation::demonstrate();
    shared_ownership::demonstrate();
    performance_optimization::demonstrate();
    api_enforcement::demonstrate();
    error_recovery::demonstrate();
}
