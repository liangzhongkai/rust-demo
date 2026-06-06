# File I/O 深度实践

> BufRead、mmap、原子写、WAL —— 从 HFT/Web3 生产场景泛化到通用文件策略

## 模块

| 文件 | 内容 |
|------|------|
| `basics.rs` | 顺序/随机读、缓冲、持久化语义、分层、seek |
| `hft.rs` | 7 个 HFT 场景（mmap 回放/AOF/spill/原子快照/WAL/热更新/分区归档） |
| `web3.rs` | 6 个 Web3 场景（event 日志/ABI 缓存/快照/manifest/多链配置/reorg） |
| `pitfalls.rs` | 8 个常见文件 I/O 陷阱 |
| `strategies.rs` | 8 条泛化应对策略 + 反例 |

## 运行

```bash
cargo run -p file-io
cargo test -p file-io
```

## 相关 crate

- `parsing` — 文件字节之上的 framing / 协议解析
- `networking` — 网络 transport；文件常作 WAL/快照层
- `async-runtime` — 异步运行时中的阻塞 I/O 隔离
