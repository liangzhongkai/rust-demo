# ffi — 外部函数接口

## 学习目标

- **HFT**：**Rust** 经 `extern "C"` 调用 [`c/vendor_ticks.c`](c/vendor_ticks.c)（`build.rs` 静态链接）。
- **Python → C**：[`python/vendor_ticks_demo.py`](python/vendor_ticks_demo.py) + `c/build_shared.sh` 用 `ctypes` 加载 **C 共享库**（不经过 Rust 进程）。
- **Python → Rust**：[`python/ffi_rust_demo.py`](python/ffi_rust_demo.py) 通过 **PyO3** 调用 [`src/py_bind.rs`](src/py_bind.rs) ↔ **同一套** `unpack_ticks_via_vendor`（内部仍链到 C）。
- 区分 **Rust 调用 native** 与 **native 持有 opaque**；Web3 示例中的 `demo_crypto_dual_out` / session 仍为 **同 crate Rust**。

## 调用链与「延迟」怎么理解

| 路径 | 实际跨过谁的边界 | 典型用途 |
|------|------------------|----------|
| `cargo run` / `hft` | Rust ↔ C（静态链） | 生产热路径、与 vendor 同进程 |
| `ctypes` + `libvendor_ticks.so` | Python ↔ **C**（Rust 可执行文件不参与） | 验证 C ABI、脚本、**不是**「调 Rust」 |
| PyO3 `unpack_ticks` | Python ↔ **Rust**（扩展 `.so` 内再链 C） | 在 Python 里复用 Rust 封装；比「ctypes 调裸 C」多一层扩展初始化，但热逻辑仍在 native |

**高频 / 微秒敏感路径**：不要把 **每条消息一次 Python 调用** 当主回路；应 **批量 buffer**、或把循环完全放在 **纯 Rust 进程** 里，Python 只做编排/低频 RPC。

构建 PyO3 需要本机 **Python 开发头文件**（如 Debian `python3-dev`；Conda 环境通常已含）。

## 前置条件

- **C 编译器**（`gcc`/`clang`）：`cargo build`、`build_shared.sh` 均需。
- **可选 / PyO3**：`pip install maturin`，且需能与 `pyo3` 链接的 Python。

## 目录结构

```
crates/ffi/
├── build.rs              # 编译 c/*.c，静态链到 `libffi`（rlib/cdylib）
├── pyproject.toml        # maturin：`maturin develop --features pyo3`
├── c/
│   ├── build_shared.sh   # 生成 libvendor_ticks.{so,dylib} 供 ctypes
│   └── vendor_ticks.c
├── python/
│   ├── vendor_ticks_demo.py   # ctypes → C
│   └── ffi_rust_demo.py       # PyO3 → Rust (`import ffi_rust`)
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs            # rlib +（开启 pyo3 时）cdylib 与 `#[pymodule] ffi_rust`
    ├── py_bind.rs        # `#[pyfunction] unpack_ticks`（feature pyo3）
    ├── hft.rs            # Rust → C
    ├── web3.rs
    ├── pitfalls.rs
    ├── strategies.rs
    └── main.rs            # `use ffi::hft` 等同名 crate 二进制
```

## Rust → C

```bash
cargo run -p ffi
cargo test -p ffi
```

## Python → C（ctypes，仅 C）

```bash
cd crates/ffi
bash c/build_shared.sh
python3 python/vendor_ticks_demo.py
```

- 生成 `c/libvendor_ticks.so`（或 macOS 上 `libvendor_ticks.dylib`），已 `gitignore`。

## Python → Rust（PyO3）

在 `crates/ffi` 目录：

```bash
pip install maturin
maturin develop --features pyo3
python3 python/ffi_rust_demo.py
```

发布构建可用：`maturin build --features pyo3`（产物 wheel 名见 [pyproject.toml](pyproject.toml) 中 `[project]`）。

## 模块

| 文件 | 内容 |
|------|------|
| `lib.rs` / `py_bind.rs` | 可选 PyO3 扩展 `ffi_rust` |
| `hft.rs` | Rust → C；PyO3 复用 `unpack_ticks_via_vendor` |
| `web3.rs` | 64 字节输出 + opaque session（Rust stub） |
| `pitfalls.rs` | 悬挂指针、unwind、`CString`、ABI |
| `strategies.rs` | 泛化矩阵与小工具 |

## 一键速查

```bash
cargo run -p ffi
cargo test -p ffi

cd crates/ffi && bash c/build_shared.sh && python3 python/vendor_ticks_demo.py

cd crates/ffi && maturin develop --features pyo3 && python3 python/ffi_rust_demo.py
```

## 与 `unsafe-rust` 的关系

- `unsafe-rust::hft` / `web3` 侧重 **内存模型与解码边界**；
- 本 crate 侧重 **跨语言边界的封装与运维可观测性**（错误码、session 生命周期）。
