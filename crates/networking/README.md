# Networking 深度实践

> TCP/UDP、会话状态机、背压与重连 —— 从 HFT/Web3 生产场景泛化到通用网络策略

## 模块

| 文件 | 内容 |
|------|------|
| `basics.rs` | 端点、流/报文语义、读循环、分层架构 |
| `hft.rs` | 7 个 HFT 生产场景（会话/组播/重组/心跳/背压/序列号/调度） |
| `web3.rs` | 6 个 Web3 场景（JSON-RPC/WS 订阅/P2P 帧/gossip/多节点/批量） |
| `pitfalls.rs` | 8 个常见网络陷阱 |
| `strategies.rs` | 8 条泛化应对策略 + 反例 |

## 运行

```bash
cargo run -p networking
cargo test -p networking
```

## 相关 crate

- `parsing` — 字节流之上的 framing / 协议解析
- `async-runtime` — Tokio 执行器与 Future 取消
- `channels` — 背压与 mpsc/oneshot 模式
