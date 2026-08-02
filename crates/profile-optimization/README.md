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

# 性能验证（推荐，需在原生 Linux 或已配置 perf 的环境）
cargo build --release -p profile-optimization
perf record -g --call-graph dwarf ./target/release/profile-optimization
perf report
```

### WSL2 上 perf 不可用？

WSL2 内核是 `6.6.x-microsoft-standard-WSL2`，Ubuntu apt **不提供** 匹配的 `linux-tools-*-microsoft-*` 包，因此 `/usr/bin/perf` 只会打印 WARNING 并退出。

**方案 A：尝试安装通用 tools 并做 symlink（部分 WSL 可用，硬件计数器仍可能受限）**

```bash
sudo apt update
sudo apt install -y linux-tools-generic linux-tools-common

# 查看实际安装的 perf 路径（版本号以本机为准）
ls /usr/lib/linux-tools-*-generic/perf

# 为当前 WSL 内核建目录并链接（把下面路径换成 ls 输出的那个）
K=$(uname -r)
sudo mkdir -p "/usr/lib/linux-tools/$K"
sudo ln -sf /usr/lib/linux-tools-6.8.0-*-generic/perf "/usr/lib/linux-tools/$K/perf"

# 验证
perf --version
perf stat -e cycles,instructions ./target/release/profile-optimization
```

若 `perf stat` 报 `Permission denied` 或 `not supported`，可试：

```bash
sudo sysctl -w kernel.perf_event_paranoid=1
```

**方案 B：在 WSL2 内用本 crate 自带工具（无需 perf）**

```bash
cargo run --release -p profile-optimization   # bench_ns / P99 histogram / StageTimer
cargo test -p profile-optimization
```

**方案 C：在原生 Linux 上 profile（最可靠）**

云主机 / 物理机 / GitHub Actions `ubuntu-latest` 上跑 `perf record`，或用 `samply record`、`cargo flamegraph`（底层仍依赖 perf）。

**方案 D：分配分析（不依赖 perf 采样）**

```bash
# heaptrack（WSL2 通常可用）
sudo apt install -y heaptrack
heaptrack ./target/release/profile-optimization
```

## 设计说明

- **纯 std**：无 criterion/perf 依赖，教学可运行；生产对照命令写在注释
- **可测量**：每个场景含 slow/fast 或 before/after 对比
- **生产对照**：每个场景标注「生产问题 → profiling 套路 → 关键约束」

## 相关 crate

- `zero-cost` — 优化后用 profile 验证零成本假设
- `simd` — SIMD 收益必须用 criterion/perf 证明
- `parsing` / `networking` — 热路径解析与 I/O 的 profiling 对象
