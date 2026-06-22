# 上游 eth-p2p-mempool-crawler 源码参考

本目录包含 [7suyash7/eth-p2p-mempool-crawler](https://github.com/7suyash7/eth-p2p-mempool-crawler) 的完整源码，供对照学习，**不参与本 workspace 编译**。

## 模块说明

| 文件 | 职责 |
|------|------|
| `runner.rs` | 主入口：初始化 NetworkManager、spawn 各 async 任务 |
| `network.rs` | `EthP2PHandler`：处理 P2P 事件和 mempool 消息 |
| `analysis.rs` | `TransactionSigned` → `TxAnalysisResult` |
| `ui.rs` | ratatui 实时 TUI 仪表盘 |
| `config.rs` | config.toml + CLI 配置加载 |
| `db.rs` / `api.rs` / `oracle.rs` | PostgreSQL 持久化、REST/WebSocket API、Gas Oracle |

## 运行上游项目

```bash
git clone https://github.com/7suyash7/eth-p2p-mempool-crawler
cd eth-p2p-mempool-crawler
cp config.toml.example config.toml  # 配置 PostgreSQL 和端口
cargo run --release
```

## 与本 crate mock 模式的对应关系

| 上游 (reference/) | 本 crate (src/) |
|-------------------|-----------------|
| `NetworkManager` + Discv4 | `pipeline::network_simulator` |
| `EthP2PHandler` | `pipeline::tx_event_handler` |
| `analysis::analyze_transaction` | `analysis::analyze_mock_tx` |
| `ui::run_ui` (ratatui) | `pipeline::display_loop` (stdout) |
