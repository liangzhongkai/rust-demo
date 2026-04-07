use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use pin_project_lite::pin_project;

// ─────────────────────────────────────────────────────────────
// 手动实现的 Timeout 组合 Future（题 6.4 的运行示例）
//
// 用 pin_project_lite 安全投影内部 #[pin] 字段，
// poll 时先查内部 future，再查 deadline。
// ─────────────────────────────────────────────────────────────
pin_project! {
    struct Timeout<F> {
        #[pin]
        future: F,
        #[pin]
        deadline: tokio::time::Sleep,
    }
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, &'static str>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if let Poll::Ready(output) = this.future.poll(cx) {
            return Poll::Ready(Ok(output));
        }
        if this.deadline.poll(cx).is_ready() {
            return Poll::Ready(Err("timeout"));
        }
        Poll::Pending
    }
}

pub async fn run_demos() {
    println!("══════════ 六、Pin、Future 状态机与性能 ══════════\n");
    demo_future_size();
    println!();
    demo_recursive_async().await;
    println!();
    demo_manual_future().await;
}

// ─────────────────────────────────────────────────────────────
// 题 6.1: async fn 编译后的状态机大小
//
// 跨 await 持有的局部变量全部计入 Future 体积。
// 大数组直接膨胀 Future；改 Vec 或缩短作用域可大幅减小。
// ─────────────────────────────────────────────────────────────
fn demo_future_size() {
    println!("【Demo 6.1】async fn 状态机大小测量\n");

    async fn with_u64() {
        let x = 0u64;
        tokio::time::sleep(Duration::from_millis(1)).await;
        let _ = &x; // x 跨 await 存活
    }

    async fn with_big_array() {
        let buf = [0u8; 16384]; // 16 KB 栈上数组
        tokio::time::sleep(Duration::from_millis(1)).await;
        let _ = &buf; // buf 跨 await 存活 → Future 至少 16 KB
    }

    async fn with_vec() {
        let buf = vec![0u8; 16384]; // 堆分配，Future 只存 Vec 头（24 bytes）
        tokio::time::sleep(Duration::from_millis(1)).await;
        let _ = &buf;
    }

    async fn with_scoped_array() {
        let _val = {
            let buf = [0u8; 16384];
            buf[0] // buf 在此 drop，不跨 await
        };
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    let f1 = with_u64();
    let f2 = with_big_array();
    let f3 = with_vec();
    let f4 = with_scoped_array();

    println!(
        "    with_u64         (u64 跨 await):       {:>6} bytes",
        std::mem::size_of_val(&f1)
    );
    println!(
        "    with_big_array   ([u8;16384] 跨 await): {:>6} bytes  ← 膨胀！",
        std::mem::size_of_val(&f2)
    );
    println!(
        "    with_vec         (Vec<u8> 跨 await):    {:>6} bytes  ← Vec 只存指针",
        std::mem::size_of_val(&f3)
    );
    println!(
        "    with_scoped_array(数组不跨 await):      {:>6} bytes",
        std::mem::size_of_val(&f4)
    );

    drop((f1, f2, f3, f4));

    println!();
    println!("    ↑ 跨 await 的 [u8; 16384] 直接膨胀 Future");
    println!("      改用 Vec 或限制作用域可保持 Future 小巧");
}

// ─────────────────────────────────────────────────────────────
// 题 6.2: 递归 async fn 需要 Box::pin
//
// async fn 编译为状态机枚举，递归导致枚举包含自身 → 无穷大小。
// Box::pin 在堆上分配，将递归部分固定为 8 字节指针。
// ─────────────────────────────────────────────────────────────
async fn demo_recursive_async() {
    println!("【Demo 6.2】递归 async fn 需要 Box::pin\n");

    struct Node {
        val: i32,
        children: Vec<Node>,
    }

    // 直接 async fn 递归无法编译：
    //   async fn sum(node: &Node) -> i32 {
    //       for c in &node.children { sum(c).await; }
    //   }
    //   error[E0733]: recursion in an async fn requires boxing

    // Box::pin 打断编译期大小推导
    fn sum_tree(node: &Node) -> Pin<Box<dyn Future<Output = i32> + '_>> {
        Box::pin(async move {
            let mut total = node.val;
            for child in &node.children {
                total += sum_tree(child).await;
            }
            total
        })
    }

    let tree = Node {
        val: 1,
        children: vec![
            Node {
                val: 2,
                children: vec![
                    Node { val: 4, children: vec![] },
                    Node { val: 5, children: vec![] },
                ],
            },
            Node {
                val: 3,
                children: vec![Node { val: 6, children: vec![] }],
            },
        ],
    };

    let result = sum_tree(&tree).await;
    println!("    树:       1");
    println!("            /   \\");
    println!("           2     3");
    println!("          / \\     \\");
    println!("         4   5     6");
    println!();
    println!("    sum_tree = {result}  (1+2+3+4+5+6 = 21)");
    println!();
    println!("    ↑ 每层递归 Box::pin 一次堆分配（8 字节指针）");
    println!("      深递归场景可改为迭代 + 手动栈避免分配");
}

// ─────────────────────────────────────────────────────────────
// 题 6.4: 手动实现 Future trait + pin-project
//
// 自定义 Timeout<F> 组合器：给任意 Future 加超时。
// 使用 pin_project_lite 安全投影 Pin 字段。
// ─────────────────────────────────────────────────────────────
async fn demo_manual_future() {
    println!("【Demo 6.4】手动实现 Future trait（Timeout 组合器）\n");
    println!("    自定义 Timeout<F>: 给任意 Future 加 deadline\n");

    // 快任务：50ms 完成，deadline 200ms → 成功
    let start = Instant::now();
    let result = Timeout {
        future: tokio::time::sleep(Duration::from_millis(50)),
        deadline: tokio::time::sleep(Duration::from_millis(200)),
    }
    .await;
    match result {
        Ok(()) => println!("    快任务 (50ms/200ms):  ✅ 完成于 {:?}", start.elapsed()),
        Err(e) => println!("    快任务 (50ms/200ms):  ❌ {e} 于 {:?}", start.elapsed()),
    }

    // 慢任务：500ms 完成，deadline 200ms → 超时
    let start = Instant::now();
    let result = Timeout {
        future: tokio::time::sleep(Duration::from_millis(500)),
        deadline: tokio::time::sleep(Duration::from_millis(200)),
    }
    .await;
    match result {
        Ok(()) => println!("    慢任务 (500ms/200ms): ✅ 完成于 {:?}", start.elapsed()),
        Err(e) => println!("    慢任务 (500ms/200ms): ❌ {e} 于 {:?}", start.elapsed()),
    }

    println!();
    println!("    Timeout<F> 实现要点:");
    println!("      1. #[pin] 标记内部 Future → 保证自引用安全");
    println!("      2. self.project() 安全拆解 Pin<&mut Self>");
    println!("      3. 先 poll future，再 poll deadline");
}
