# 03 — Ethereum Mempool Crawler

> 基于 [eth-p2p-mempool-crawler](https://github.com/7suyash7/eth-p2p-mempool-crawler) 和 [Medium 文章](https://medium.com/@suyashnyn1/observing-ethereums-mempool-directly-with-reth-d404919cae79) 的 Rust 实战案例。

## 项目背景

以太坊 Mempool 是交易进入区块前的「候场区」。常见做法是跑全节点或依赖 RPC 提供商，但前者成本高，后者有延迟和限流。

本案例展示如何**直接连接以太坊 P2P 网络**，监听 Mempool 交易广播，无需完整同步链状态。核心思路是复用 [Reth](https://github.com/paradigmxyz/reth) 的模块化 crate，而不是运行完整节点。

## 架构概览

```text
┌─────────────────┐     ┌──────────────────┐     ┌─────────────┐     ┌─────────┐
│ NetworkManager  │────►│ Tx Event Handler │────►│  Processor  │────►│   UI    │
│  (reth-network) │     │  (EthP2PHandler) │     │ (analysis)  │     │(ratatui)│
└─────────────────┘     └──────────────────┘     └─────────────┘     └─────────┘
        │                        │                      │
        └──── Peer Events ───────┴──── MPSC channels ───┘
```

### 关键任务

| 任务 | 职责 | 对应 crate |
|------|------|-----------|
| NetworkManager | TCP/RLPx 连接、Discv4 发现 | `reth-network`, `reth-discv4` |
| Tx Event Handler | 处理 `NetworkTransactionEvent` | 自定义 `EthP2PHandler` |
| Processor | `TransactionSigned` → `TxAnalysisResult` | `analysis` 模块 |
| UI | ratatui 实时仪表盘 | `ui` 模块 |

### Mempool 两条数据路径

1. **Hash → Request → Response**：peer 广播 `NewPooledTransactionHashes`，crawler 发送 `GetPooledTransactions`，peer 返回 `PooledTransactions`
2. **Direct Broadcast**：peer 直接发送完整 `Transactions` 消息

## 本 crate 结构

```
03-eth-mempool-crawler/
├── src/
│   ├── main.rs          # CLI 入口
│   ├── lib.rs
│   ├── analysis.rs      # Mock 交易分析（默认模式）
│   ├── pipeline.rs      # Mock 异步管道演示
│   ├── types.rs
│   └── (reference/ 上游完整源码，供阅读对照)
├── examples/
│   ├── mempool_flow.rs
│   └── task_architecture.rs
└── config.toml.example  # live 模式配置
```

## 快速开始

### Mock 模式（默认，无需网络）

```bash
# 运行 mock 管道，模拟 P2P 事件和交易分析
cargo run -p eth-mempool-crawler

# 指定模拟时长
cargo run -p eth-mempool-crawler -- mock --duration 15

# 运行示例
cargo run -p eth-mempool-crawler --example mempool_flow
cargo run -p eth-mempool-crawler --example task_architecture
```

### Live 模式（连接以太坊主网）

完整 live crawler 见 `reference/` 目录（上游源码）或直接使用上游仓库：

```bash
git clone https://github.com/7suyash7/eth-p2p-mempool-crawler
cd eth-p2p-mempool-crawler
# 准备 PostgreSQL + config.toml
cargo run --release
```

## 核心技术栈

| 层次 | 技术 |
|------|------|
| P2P 网络 | Reth (`reth-network`, `reth-discv4`, `reth-eth-wire`) |
| 异步运行时 | Tokio + MPSC channels |
| 交易类型 | `reth-primitives`, `alloy-consensus` |
| TUI | ratatui + crossterm |
| 持久化 | PostgreSQL + sqlx |
| API | axum WebSocket + REST |

## 学习要点

1. **Reth 模块化设计**：无需全节点，可单独使用 `reth-network` 做 P2P 客户端
2. **Async 任务解耦**：Network / Handler / Processor / UI 通过 channel 通信，互不阻塞
3. **P2P 协议栈**：Discv4 发现 → RLPx 握手 → ETH Status → 交易消息
4. **类型转换陷阱**：`PooledTransaction` → `TransactionSigned`（Path 1）；`NoopProvider` 下 Path 2 可能已预转换

## 参考资源

- [Observing Ethereum's Mempool Directly with Reth (Medium)](https://medium.com/@suyashnyn1/observing-ethereums-mempool-directly-with-reth-d404919cae79)
- [eth-p2p-mempool-crawler (GitHub)](https://github.com/7suyash7/eth-p2p-mempool-crawler)
- [Reth Documentation](https://reth.rs/)

## 许可

Live 模式代码基于上游 [Apache-2.0](https://github.com/7suyash7/eth-p2p-mempool-crawler/blob/main/LICENSE) 项目移植。
