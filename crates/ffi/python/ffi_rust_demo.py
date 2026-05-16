#!/usr/bin/env python3
"""
Call Rust `unpack_ticks_via_vendor` (C under the hood) through PyO3.

From `crates/ffi`:

    pip install maturin
    maturin develop --features pyo3
    python3 python/ffi_rust_demo.py

This imports the native module `ffi_rust` (see `src/lib.rs` + `py_bind.rs`).
"""

from __future__ import annotations

import importlib.util
import struct
import sys


def main() -> None:
    if importlib.util.find_spec("ffi_rust") is None:
        print(
            "Missing Python module `ffi_rust`. From crates/ffi run:\n"
            "  pip install maturin\n"
            "  maturin develop --features pyo3\n",
            file=sys.stderr,
        )
        sys.exit(1)

    ffi_rust = importlib.import_module("ffi_rust")
    wire = b"".join(struct.pack("<Q", x) for x in (1, 2, 3))
    ticks = ffi_rust.unpack_ticks(wire)
    print(f"unpack_ticks (PyO3 → Rust) = {ticks}")
    assert list(ticks) == [1, 2, 3], ticks


if __name__ == "__main__":
    main()
