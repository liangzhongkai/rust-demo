/* Demo vendor library: LE u64 tick stream -> dst (must match Rust façade in hft.rs). */

#include <stddef.h>
#include <stdint.h>

static uint64_t read_le_u64(const uint8_t *p) {
  uint64_t v = 0;
  for (int i = 0; i < 8; i++) {
    v |= (uint64_t)p[i] << (8 * i);
  }
  return v;
}

int demo_vendor_unpack_ticks(uint64_t *dst, size_t dst_cap, const uint8_t *src,
                             size_t src_len, size_t *written) {
  if (dst == NULL || src == NULL || written == NULL) {
    return -1;
  }
  if (src_len % sizeof(uint64_t) != 0) {
    return -2;
  }
  size_t n = src_len / sizeof(uint64_t);
  if (n > dst_cap) {
    return -3;
  }
  for (size_t i = 0; i < n; i++) {
    dst[i] = read_le_u64(src + i * sizeof(uint64_t));
  }
  *written = n;
  return 0;
}
