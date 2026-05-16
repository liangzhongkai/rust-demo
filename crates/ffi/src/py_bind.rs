//! Python 绑定（功能开关：`--features pyo3`）。

use crate::hft::unpack_ticks_via_vendor;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// 将 LE `u64` 流解码为 tick 列表（经 Rust `unpack_ticks_via_vendor` → 同一 C 符号）。
#[pyfunction]
fn unpack_ticks(wire: &[u8]) -> PyResult<Vec<u64>> {
    let n = wire.len() / 8;
    let mut scratch = vec![0u64; n.saturating_add(8).max(8)];
    unpack_ticks_via_vendor(wire, &mut scratch)
        .map(|s| s.to_vec())
        .map_err(|e| PyValueError::new_err(format!("{e:?}")))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(unpack_ticks, m)?)?;
    Ok(())
}
