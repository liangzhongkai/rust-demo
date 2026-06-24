# Profile Optimization 性能分析与优化深度实践

> perf / flamegraph / criterion / P99 histogram —— 从 HFT/Web3 生产场景泛化到通用 profiling 策略

## 模块

| 文件 | 内容 |
|------|------|
| `util.rs` | `LatencyHistogram` / `bench_ns` / `StageTimer` / `HotspotCounter` / `AllocCounter` |
| `basics.rs` | perf 工作流、criterion、P99 vs mean、分桶、release、warmup |
| `hft.rs` | 7 个 HFT 场景（tick P99/orderbook 扫描/FIX 解析/锁竞争/分配/分支/稳态 bench） |
| `web3.rs` | 6 个 Web3 场景（block replay/mempool 过滤/Merkle/RPC 分解/trie cache/bundle deadline） |
| `pitfalls.rs` | 8 个 profiling 陷阱 |
| `strategies.rs` | 8 条泛化应对策略 + 决策矩阵 |

## 运行

```bash
cargo run -p profile-optimization

# 性能验证（推荐）
cargo build --release -p profile-optimization
perf record -g --call-graph dwarf ./target/release/profile-optimization
```

## 设计说明

- **纯 std**：无 criterion/perf 依赖，教学可运行；生产对照命令写在注释
- **可测量**：每个场景含 slow/fast 或 before/after 对比
- **生产对照**：每个场景标注「生产问题 → profiling 套路 → 关键约束」

## 相关 crate

- `zero-cost` — 优化后用 profile 验证零成本假设
- `simd` — SIMD 收益必须用 criterion/perf 证明
- `parsing` / `networking` — 热路径解析与 I/O 的 profiling 对象
