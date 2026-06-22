# Mock 管道（默认 10 秒）
cargo run -p eth-mempool-crawler

# 指定时长
cargo run -p eth-mempool-crawler -- mock --duration 5

# 示例
cargo run -p eth-mempool-crawler --example mempool_flow
cargo run -p eth-mempool-crawler --example task_architecture

# 测试
cargo test -p eth-mempool-crawler


cargo check -p eth-mempool-crawler 通过
cargo test -p eth-mempool-crawler 2 个测试通过
cargo run -p eth-mempool-crawler -- mock --duration 1 正常结束并输出 summary


cargo test -p eth-mempool-crawler                               2 passed
cargo run -p eth-mempool-crawler -- mock --duration 2           正常结束，输出 Summary
cargo run -p eth-mempool-crawler --example mempool_flow         正常
cargo run -p eth-mempool-crawler --example task_architecture    正常 