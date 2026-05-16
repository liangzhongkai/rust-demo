//! `ffi` 库：与同名 `ffi` 二进制共用模块；启用 `pyo3` 特性时额外注册 Python 扩展 `ffi_rust`。

#![allow(dead_code)]

pub mod hft;
pub mod pitfalls;
pub mod strategies;
pub mod web3;

#[cfg(feature = "pyo3")]
mod py_bind;

#[cfg(feature = "pyo3")]
use pyo3::prelude::*;

/// PyO3 入口：构建扩展时使用 `maturin build --features pyo3`，`import ffi_rust`。
#[cfg(feature = "pyo3")]
#[pymodule]
fn ffi_rust(m: &Bound<'_, PyModule>) -> PyResult<()> {
    py_bind::register(m)?;
    Ok(())
}
