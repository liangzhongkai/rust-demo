# Parsing 深度实践

> 组合子、递归下降、零拷贝视图 —— 从 HFT/Web3 生产场景泛化到通用解析策略

## 模块

| 文件 | 内容 |
|------|------|
| `basics.rs` | Parser 协议、递归下降、零拷贝、流式状态机 |
| `hft.rs` | 7 个 HFT 生产场景（FIX/SBE/framing/ITCH/CSV/resync） |
| `web3.rs` | 6 个 Web3 场景（ABI/RLP/hex/RPC/event/EIP-712） |
| `pitfalls.rs` | 8 个常见陷阱 |
| `strategies.rs` | 8 条泛化应对策略 + 反例 |

## 运行

```bash
cargo run -p parsing
```

## 相关 crate

- `zero-copy-parser` — FIX 零拷贝深入示例
- `iterators` — 解析后与迭代器管道组合（`chunks_exact` / `filter_map`）
