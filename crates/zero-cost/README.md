# Zero-Cost Abstraction 深度实践

> 单态化、静态分派、newtype、迭代器消除 —— 从 HFT/Web3 生产场景泛化到通用性能策略

## 模块

| 文件 | 内容 |
|------|------|
| `basics.rs` | 单态化、静态 vs 动态分派、newtype、迭代器融合与消除 |
| `hft.rs` | 7 个 HFT 场景（定点价/静态策略/内联解码/const 环缓冲/订单簿泛型/热路径分派/编译期配置） |
| `web3.rs` | 6 个 Web3 场景（U256/ABI 泛型编码/事件过滤/Merkle 泛型哈希/地址 newtype/VM 静态分派） |
| `pitfalls.rs` | 8 个零成本相关陷阱 |
| `strategies.rs` | 8 条泛化应对策略 + 反例 |

## 运行

```bash
cargo run -p zero-cost
cargo test -p zero-cost
```

## 相关 crate

- `iterators` — 迭代器适配器如何在 LLVM 层被消除
- `generics` — 单态化与 trait bound 基础
- `profile-optimization` — 用 perf/criterion 验证「零成本」假设
- `simd` — 单态化 + 内联后 SIMD 自动向量化
