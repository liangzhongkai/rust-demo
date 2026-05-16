#!/usr/bin/env bash
# Build a shared library from vendor_ticks.c for Python ctypes (Rust static link is unchanged).
set -euo pipefail
cd "$(dirname "$0")"

case "$(uname -s)" in
Darwin)
  OUT="libvendor_ticks.dylib"
  ;;
*)
  OUT="libvendor_ticks.so"
  ;;
esac

cc -shared -fPIC -std=c11 -Wall -Wextra -o "${OUT}" vendor_ticks.c
echo "Built $(pwd)/${OUT}"
