# Stack vs Heap 栈与堆深度实践

> 内存布局决策 —— 从 HFT/Web3 生产场景泛化到通用栈/堆策略

## 模块

| 文件 | 内容 |
|------|------|
| `util.rs` | `AllocCounter` / `InlineBuffer` / `RingBuffer` / `BumpArena` / `bench_ns` |
| `basics.rs` | 栈帧 vs 堆、Copy 语义、Vec 增长、栈数组 |
| `hft.rs` | 7 个 HFT 场景（L2 Top-N/零分配解析/环形缓冲/InlineBuffer/批量攒批/enum 策略/delta 预分配） |
| `web3.rs` | 6 个 Web3 场景（Hash32/calldata 解码/mempool 过滤/Merkle proof/Block header/Bump arena） |
| `pitfalls.rs` | 8 个栈/堆陷阱 |
| `strategies.rs` | 8 条泛化应对策略 + 决策矩阵 |

## 运行

```bash
cargo run -p stack-vs-heap
cargo test -p stack-vs-heap
```

### 生产验证（分配分析）

```bash
cargo build --release -p stack-vs-heap

# heaptrack（WSL2 通常可用）
heaptrack ./target/release/stack-vs-heap

# 或 dhat（需添加 dhat crate 到被测二进制）
```

## 设计说明

- **纯 std**：无 smallvec/dhat 依赖，教学可运行；生产对照命令写在注释
- **可测量**：basics/hft 含 slow vs fast 或 alloc 计数对比
- **生产对照**：每个场景标注「生产问题 → 内存套路 → 关键约束」

## 相关 crate

- `arena-allocators` — Bump/TypedArena 深入
- `custom-allocators` — GlobalAlloc 定制
- `profile-optimization` — heaptrack/dhat 配合 P99 验证
- `zero-cost` — 栈 enum 分派与零成本抽象
- `copy-vs-clone` — Copy 语义补充
