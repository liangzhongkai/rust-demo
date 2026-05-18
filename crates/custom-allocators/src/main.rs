//! 可执行入口：等价于调用库内 `run_all_demonstrations`。
//!
//! ## 可选：进程级挂钩（每个二进制至多一处）
//!
//! ```ignore
//! use std::alloc::System;
//! use custom_allocators::basics::StatsAllocator;
//!
//! #[global_allocator]
//! static GLOBAL: StatsAllocator<System> = StatsAllocator::new(System);
//! ```

fn main() {
    custom_allocators::run_all_demonstrations();
}
