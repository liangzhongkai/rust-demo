//! # 生命周期常见陷阱与诊断
//!
//! 现象 → 根因 → 修法。生命周期错误是 **编译期** 拒绝，比运行时 panic 好；
//! 但 async / self-referential / 缓存引用是生产里最高频的「解不开 borrow」来源。

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：返回局部引用 —— E0515
// ============================================================================
/// **现象**：`fn bad() -> &str { let s = "x"; &s }` 编译失败。
/// **根因**：返回值指向栈上变量，函数返回后失效。
/// **修法**：返回 `String` / `Cow` / 调用方传入 `&mut String` 写入。
pub mod return_local_ref {
    pub fn good() -> String {
        "owned".to_string()
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：返回局部引用");
        println!("bad: fn f() -> &str {{ let s = String::from(\"x\"); &s }}  // E0515");
        println!("good: 返回 owned 或让调用方提供 buffer\n");
        let _ = good();
    }
}

// ============================================================================
// 陷阱 2：视图活得比 buffer 久 —— dangling reference
// ============================================================================
/// **现象**：parse 出 `OrderView`，buffer 回收后仍读 cl_ord_id。
/// **根因**：结构体持 `'buf` 但 buffer 被 drop / reuse。
/// **修法**：视图不跨 scope；或边界 promote 为 owned。
pub mod view_outlives_buffer {
    pub struct OrderView<'buf> {
        pub id: &'buf [u8],
    }

    pub fn parse<'buf>(raw: &'buf [u8]) -> OrderView<'buf> {
        OrderView { id: raw }
    }

    pub fn good_pattern() {
        let raw = b"ORD-001".to_vec();
        let view = parse(&raw);
        let _id = view.id;
        // raw 在此 scope 结束前必须保持存活 —— 编译器强制
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：视图活得比 buffer 久");
        good_pattern();
        println!("反模式：把 OrderView 存进全局 HashMap 而 buffer 来自 ring slot reuse");
        println!("修法：parse → 立刻 to_vec / Arc，再入库\n");
    }
}

// ============================================================================
// 陷阱 3：自引用结构 —— 编译器无法证明内部指针合法
// ============================================================================
/// **现象**：想存 `struct Parser { buf: Vec<u8>, cur: &u8 }`。
/// **根因**：移动 struct 时内部引用会失效。
/// **修法**：索引代替指针；Pin + unsafe；或分开存 `Vec` + offset。
pub mod self_referential {
    use std::marker::PhantomData;

    pub struct SafeParser {
        buf: Vec<u8>,
        offset: usize,
    }

    impl SafeParser {
        pub fn new(data: &[u8]) -> Self {
            Self {
                buf: data.to_vec(),
                offset: 0,
            }
        }

        pub fn current(&self) -> Option<u8> {
            self.buf.get(self.offset).copied()
        }

        pub fn advance(&mut self) {
            if self.offset < self.buf.len() {
                self.offset += 1;
            }
        }
    }

    /// 仅演示：自引用需要 Pin，生产优先用索引。
    pub struct PinPlaceholder<'a> {
        _marker: PhantomData<&'a u8>,
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：自引用结构");
        let mut p = SafeParser::new(b"abc");
        println!("current={:?}", p.current());
        p.advance();
        println!("用 offset 替代 `&buf[i]` 内部引用\n");
        let _ = PinPlaceholder { _marker: PhantomData };
    }
}

// ============================================================================
// 陷阱 4：Iterator 生命周期不匹配 —— 返回 `impl Iterator<Item=&str>` 来自局部
// ============================================================================
/// **现象**：函数返回迭代器，但数据源是局部 `Vec`。
/// **根因**：迭代器持有对局部数据的引用。
/// **修法**：调用方拥有数据；或返回 owned iterator / collect。
pub mod iterator_lifetime {
    pub fn good_lines(text: &str) -> Vec<String> {
        text.split_whitespace().map(|s| s.to_string()).collect()
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：Iterator 生命周期");
        let words = good_lines("hello world");
        println!("owned items: {:?}\n", words);
    }
}

// ============================================================================
// 陷阱 5：async / spawn 借局部 —— `'static` 不满足
// ============================================================================
/// **现象**：`tokio::spawn(async { foo(&buf).await })` 编译失败。
/// **根因**：future 可能比 `buf` 活得更久。
/// **修法**：`move` + `Arc<[u8]>` / `Bytes`；或在 async 块外 clone。
pub mod async_borrow {
    use std::sync::Arc;
    use std::thread;

    pub fn spawn_with_owned(data: Arc<[u8]>) {
        thread::spawn(move || {
            let _ = data.len();
        })
        .join()
        .unwrap();
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：async/spawn 借局部");
        let buf = b"eth_subscribe".to_vec();
        spawn_with_owned(Arc::from(buf));
        println!("spawn 前 Arc::from；async 与 thread 同规则\n");
    }
}

// ============================================================================
// 陷阱 6：协变误用 —— 长生命周期赋给短生命周期槽位
// ============================================================================
/// **现象**：把 `&'static str` 塞进只该活 `'conn` 的结构，后来 conn 断开仍读。
/// **根因**：&T 协变；`'static: 'conn` 允许「升级」，但语义上 conn 已死。
/// **修法**：类型上用 `'conn` 标注，禁止 `'static` 混进 conn-scoped cache。
pub mod covariance_misuse {
    pub struct ConnCache<'conn> {
        pub last_method: Option<&'conn str>,
    }

    pub fn on_message<'conn>(cache: &mut ConnCache<'conn>, method: &'conn str) {
        cache.last_method = Some(method);
    }

    pub fn demonstrate() {
        println!("## 陷阱 6：协变 / conn-scoped cache");
        let frame = "eth_getBlockByNumber".to_string();
        let mut cache = ConnCache { last_method: None };
        on_message(&mut cache, &frame);
        println!("last_method={:?}", cache.last_method);
        println!("勿把 `'static` 配置字符串塞进 `'conn` cache 冒充 frame 内数据\n");
    }
}

// ============================================================================
// 陷阱 7：多引用 struct 与 elision 混淆
// ============================================================================
/// **现象**：`struct Event<'a> { topic: &'a [u8], label: &'a str }` 但两字段来自不同 buffer。
/// **根因**：强行用同一 `'a` 约束，导致合法代码编不过或错误代码能过。
/// **修法**：拆分 `'topic` / `'label` 两个 lifetime。
pub mod multiple_lifetimes {
    pub struct Event<'topic, 'label> {
        pub topic: &'topic [u8],
        pub label: &'label str,
    }

    pub fn merge<'t, 'l>(topic: &'t [u8], label: &'l str) -> Event<'t, 'l> {
        Event { topic, label }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：多 lifetime 字段");
        let t = [1u8, 2, 3];
        let l = "Transfer";
        let ev = merge(&t, l);
        println!("topic={:?} label={}\n", ev.topic, ev.label);
    }
}

// ============================================================================
// 陷阱 8：trait object 默认 `'static` —— `Box<dyn Trait>` 隐式约束
// ============================================================================
/// **现象**：想存 `Box<dyn Parser<'a>>` 进 vec，生命周期绕不清。
/// **根因**：`dyn Trait` 默认 `'static`；带引用 trait 需 `Box<dyn Parser<'a> + 'a>`。
/// **修法**：热路径用泛型单态化；或 owned 解析结果。
pub mod trait_object_static {
    pub trait Handler {
        fn name(&self) -> &str;
    }

    pub struct LogHandler {
        tag: String,
    }

    impl Handler for LogHandler {
        fn name(&self) -> &str {
            &self.tag
        }
    }

    pub fn dispatch(h: &dyn Handler) {
        println!("handler={}", h.name());
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：trait object 默认 `'static`");
        let h = LogHandler { tag: "indexer".into() };
        dispatch(&h);
        println!("`Box<dyn Trait>` 默认 `'static`；带借用的 trait 优先泛型\n");
    }
}

pub fn demonstrate() {
    return_local_ref::demonstrate();
    view_outlives_buffer::demonstrate();
    self_referential::demonstrate();
    iterator_lifetime::demonstrate();
    async_borrow::demonstrate();
    covariance_misuse::demonstrate();
    multiple_lifetimes::demonstrate();
    trait_object_static::demonstrate();
}
