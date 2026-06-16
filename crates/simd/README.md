# SIMD 向量化深度实践

> AVX2 intrinsics、自动向量化、SoA 布局 —— 从 HFT/Web3 生产场景泛化到通用 SIMD 策略

## 模块

| 文件 | 内容 |
|------|------|
| `util.rs` | `sum_f64` / `sum_i64` / `dot_f64` / `bytes_eq_32` 等 AVX2 实现 + 标量回退 |
| `basics.rs` | Lane 宽度、显式 vs 自动向量化、水平归约、FMA、feature detect |
| `hft.rs` | 7 个 HFT 场景（最优价扫描/VWAP/FIX 锚点/延迟直方图/rolling sum/跨所 spread/批量风控） |
| `web3.rs` | 6 个 Web3 场景（Merkle 层/topic 过滤/地址白名单/RLP 扫描/Bloom/selector 提取） |
| `pitfalls.rs` | 8 个 SIMD 相关陷阱 |
| `strategies.rs` | 8 条泛化应对策略 + 决策矩阵 |

## 运行

```bash
cargo run -p simd
cargo test -p simd
```

## 设计说明

- **Stable Rust**：使用 `std::arch::x86_64` + `is_x86_feature_detected!`，非 nightly `portable_simd`
- **可移植**：非 x86_64 或旧 CPU 自动走标量路径，示例仍可编译运行
- **生产对照**：每个场景标注「生产问题 → SIMD 套路 → 关键约束」

## 相关 crate

- `zero-cost` — 单态化 + 内联后 LLVM 自动向量化
- `parsing` — 二进制/JSON 热路径解析
- `profile-optimization` — criterion / perf 验证 SIMD 收益
