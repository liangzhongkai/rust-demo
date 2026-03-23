//! 场景：同一块只读缓冲区上多线程并行（解析协议包、扫描大文件 mmap、共享配置快照）
//!
//! **权衡**
//! - 普通 `spawn` + `move` 无法持有栈上 `&[u8]`（生命周期不够长）。
//! - `thread::scope` 保证所有子线程在 scope 结束前 join，从而允许借用父栈/堆上的数据。
//! - 仍要保证被借用的数据在 scope 内不被父线程非法修改（通常用只读切片或内部同步）。

use std::thread;

fn partial_xor(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, &b| a ^ b)
}

pub fn demonstrate() {
    let payload: Vec<u8> = (0u8..=255).cycle().take(4096).collect();

    let results = thread::scope(|s| {
        let mid = payload.len() / 2;
        let (left_slice, right_slice) = payload.split_at(mid);
        let left = s.spawn(move || partial_xor(left_slice));
        let right = s.spawn(move || partial_xor(right_slice));
        (left.join().unwrap(), right.join().unwrap())
    });

    println!("  scoped 并行 XOR 两半: left={:02x} right={:02x}", results.0, results.1);
    println!("  → 泛化：需要“短期并行 + 共享只读视图”时用 scoped；写共享状态仍需 Mutex 或通道。");
}
