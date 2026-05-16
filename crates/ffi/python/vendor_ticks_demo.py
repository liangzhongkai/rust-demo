#!/usr/bin/env python3
"""
Call `demo_vendor_unpack_ticks` in **C only** (`c/vendor_ticks.c`).

This does **not** load Rust; it loads `libvendor_ticks.so` from `c/build_shared.sh`.
For **Python → Rust**, use `ffi_rust_demo.py` + `maturin develop` (PyO3).

Prereq (from `crates/ffi`):

    bash c/build_shared.sh
    python3 python/vendor_ticks_demo.py
"""

from __future__ import annotations

import ctypes
import platform
import struct
import sys
from pathlib import Path

CRATE_ROOT = Path(__file__).resolve().parents[1]
_SUFFIX = ".dylib" if platform.system() == "Darwin" else ".so"
_LIB_PATH = CRATE_ROOT / "c" / f"libvendor_ticks{_SUFFIX}"


def _load() -> ctypes.CDLL:
    if not _LIB_PATH.is_file():
        print(
            f"Missing {_LIB_PATH}. Run:\n  cd {CRATE_ROOT}\n  bash c/build_shared.sh",
            file=sys.stderr,
        )
        sys.exit(1)
    return ctypes.CDLL(str(_LIB_PATH))


def main() -> None:
    lib = _load()
    fn = lib.demo_vendor_unpack_ticks
    fn.argtypes = [
        ctypes.POINTER(ctypes.c_uint64),
        ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_uint8),
        ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_size_t),
    ]
    fn.restype = ctypes.c_int

    # Same logical payload as `hft::demonstrate`: ticks 1, 2, 3 as little-endian u64.
    wire = b"".join(struct.pack("<Q", x) for x in (1, 2, 3))
    scratch = (ctypes.c_uint64 * 8)()
    written = ctypes.c_size_t(0)
    buf = ctypes.create_string_buffer(wire)

    rc = fn(
        ctypes.cast(scratch, ctypes.POINTER(ctypes.c_uint64)),
        ctypes.c_size_t(len(scratch)),
        ctypes.cast(buf, ctypes.POINTER(ctypes.c_uint8)),
        ctypes.c_size_t(len(wire)),
        ctypes.byref(written),
    )
    print(f"demo_vendor_unpack_ticks rc={rc}, written={written.value}")
    if rc != 0:
        sys.exit(1)
    ticks = [int(scratch[i]) for i in range(written.value)]
    print(f"decoded ticks = {ticks}")
    assert ticks == [1, 2, 3], ticks


if __name__ == "__main__":
    main()
