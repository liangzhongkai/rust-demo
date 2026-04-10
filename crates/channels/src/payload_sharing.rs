//! 场景：消息体很大或需多订阅读同一份只读数据（视频帧元数据、大 JSON、共享配置快照）
//!
//! **权衡**
//! - `send` **移动**所有权：每消息一份 `Vec<u8>` 会复制/搬移堆数据，成本高。
//! - `Arc<T>`：多消息共享同一分配，clone 只增引用计数；适合 **只读** 共享。要写需 `Arc<Mutex<T>>` 等，回到锁争用问题。
//! - **泛化**：通道传的是「所有权转移」；大对象要么移一次、要么共享智能指针、要么传句柄（id）再查存储。

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

pub fn demonstrate() {
    let blob = Arc::new(vec![7u8; 16]);
    let blob_tx = Arc::clone(&blob);
    let (tx, rx) = mpsc::channel();

    let t = thread::spawn(move || {
        for i in 0..2 {
            let payload = Arc::clone(&blob_tx);
            tx.send((i, payload)).expect("receiver alive");
        }
    });

    while let Ok((id, shared)) = rx.recv() {
        println!(
            "  收到 id={id}, Arc strong_count={}, 首字节={}",
            Arc::strong_count(&shared),
            shared[0]
        );
    }
    t.join().expect("sender panicked");
    println!(
        "  main 仍持有原始 blob，strong_count={}",
        Arc::strong_count(&blob)
    );
}
