//! 场景：批处理 / MapReduce 式 CPU 并行（如离线报表、图片分块、科学计算）
//!
//! **权衡**
//! - `spawn` + `move`：每个任务一份所有权，适合“任务自带数据”；大结构体要考虑 `Arc` 或分块避免重复 clone。
//! - 线程数 ≈ 物理核数：过多线程在 CPU 密集型任务上只会增加调度开销（上下文切换、缓存颠簸）。
//! - `join`：不 join 会“泄漏”线程句柄与未处理 panic；生产里通常要统一收集结果或用结构化并发（如 scoped、线程池）。

use std::thread;
use std::time::Instant;

fn chunk_sum(slice: &[i32]) -> i64 {
    slice.iter().map(|&x| x as i64).sum()
}

pub fn demonstrate() {
    let data: Vec<i32> = (0..8_000_000).map(|i| i % 97).collect();
    let n = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);

    let chunk_size = data.len().div_ceil(n);
    let t0 = Instant::now();

    let mut handles = Vec::with_capacity(n);
    for i in 0..n {
        let start = i * chunk_size;
        if start >= data.len() {
            break;
        }
        let end = (start + chunk_size).min(data.len());
        let chunk = data[start..end].to_vec(); // 生产里常为 Arc<Vec<_>> 或 &'static 分块，避免整块 clone
        handles.push(thread::spawn(move || chunk_sum(&chunk)));
    }

    let chunk_count = handles.len();
    let partial: i64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    let elapsed = t0.elapsed();

    println!("  spawn+join 分块求和: sum={partial}, chunks={chunk_count}, {:?}", elapsed);
    println!("  → 泛化：CPU 并行 = 切分数据 + 每块独立计算 + 归约；线程数对齐硬件，避免盲目 spawn。");

    // Panic 传播：子线程 panic 时 join 得到 Err
    let bad = thread::spawn(|| panic!("模拟某分片解析失败"));
    match bad.join() {
        Ok(_) => {}
        Err(e) => println!("  join 收到子线程 panic: {e:?}"),
    }
    println!("  → 泛化：生产里用 Result 类型、边界检查、或 supervisor 重启策略，而不是裸 panic。");
}
