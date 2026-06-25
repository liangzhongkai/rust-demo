//! 跨模块复用的栈/堆诊断工具。
//!
//! 生产环境配合 heaptrack、dhat、perf mem；本 crate 用纯 std 实现可运行的教学替身。

#![allow(dead_code)]

use std::time::Instant;

/// 分配计数：用 Vec 容量变化近似 heap churn（生产用 dhat / heaptrack）。
#[derive(Debug, Default, Clone, Copy)]
pub struct AllocCounter {
    pub allocs: u64,
    pub bytes: u64,
}

impl AllocCounter {
    pub fn track_vec_push<T>(&mut self, v: &mut Vec<T>, item: T) {
        let cap_before = v.capacity();
        v.push(item);
        if v.capacity() > cap_before {
            self.allocs += 1;
            self.bytes += (v.capacity() - cap_before) as u64 * std::mem::size_of::<T>() as u64;
        }
    }

    pub fn track_string_push(&mut self, s: &mut String, chunk: &str) {
        let cap_before = s.capacity();
        s.push_str(chunk);
        if s.capacity() > cap_before {
            self.allocs += 1;
            self.bytes += (s.capacity() - cap_before) as u64;
        }
    }
}

/// 简易微基准：warmup + N 次迭代，返回 (min_ns, mean_ns)。
pub fn bench_ns<F: FnMut()>(warmup: usize, iters: usize, mut f: F) -> (u64, u64) {
    for _ in 0..warmup {
        f();
    }
    let mut samples: Vec<u64> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        f();
        samples.push(start.elapsed().as_nanos() as u64);
    }
    let min = *samples.iter().min().unwrap_or(&0);
    let mean = if samples.is_empty() {
        0
    } else {
        samples.iter().sum::<u64>() / samples.len() as u64
    };
    (min, mean)
}

/// 打印类型在栈上的布局信息。
pub fn layout<T>(name: &str) {
    println!(
        "  {name}: size={} align={} (栈上直接存放)",
        std::mem::size_of::<T>(),
        std::mem::align_of::<T>()
    );
}

/// 栈上小缓冲 + 溢出到堆的通用模式（不依赖 smallvec crate）。
#[derive(Debug, Clone)]
pub struct InlineBuffer<T, const N: usize> {
    inline: [Option<T>; N],
    len: usize,
    spill: Vec<T>,
}

impl<T, const N: usize> Default for InlineBuffer<T, N> {
    fn default() -> Self {
        Self {
            inline: std::array::from_fn(|_| None),
            len: 0,
            spill: Vec::new(),
        }
    }
}

impl<T, const N: usize> InlineBuffer<T, N> {
    pub fn push(&mut self, item: T) {
        if self.len < N {
            self.inline[self.len] = Some(item);
            self.len += 1;
        } else {
            self.spill.push(item);
        }
    }

    pub fn len(&self) -> usize {
        self.len + self.spill.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn heap_spill_count(&self) -> usize {
        self.spill.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.inline[..self.len]
            .iter()
            .filter_map(|o| o.as_ref())
            .chain(self.spill.iter())
    }
}

/// 固定容量环形缓冲（栈上数组 backing，零堆分配）。
#[derive(Debug, Clone)]
pub struct RingBuffer<T, const CAP: usize> {
    buf: [Option<T>; CAP],
    head: usize,
    len: usize,
}

impl<T, const CAP: usize> Default for RingBuffer<T, CAP> {
    fn default() -> Self {
        Self {
            buf: std::array::from_fn(|_| None),
            head: 0,
            len: 0,
        }
    }
}

impl<T, const CAP: usize> RingBuffer<T, CAP> {
    pub fn push(&mut self, item: T) -> Option<T> {
        let idx = (self.head + self.len) % CAP;
        let evicted = if self.len == CAP {
            let old_idx = self.head;
            self.head = (self.head + 1) % CAP;
            self.buf[old_idx].take()
        } else {
            self.len += 1;
            None
        };
        self.buf[idx] = Some(item);
        evicted
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        (0..self.len).filter_map(move |i| {
            let idx = (self.head + i) % CAP;
            self.buf[idx].as_ref()
        })
    }
}

/// 简易 bump arena：一次请求生命周期内批量堆分配，结束时整体释放。
pub struct BumpArena {
    chunks: Vec<Vec<u8>>,
    current: Vec<u8>,
    chunk_size: usize,
}

impl BumpArena {
    pub fn new(chunk_size: usize) -> Self {
        Self {
            chunks: Vec::new(),
            current: Vec::with_capacity(chunk_size),
            chunk_size,
        }
    }

    pub fn alloc_bytes(&mut self, len: usize) -> &mut [u8] {
        if self.current.len() + len > self.chunk_size {
            let old = std::mem::replace(&mut self.current, Vec::with_capacity(self.chunk_size));
            if !old.is_empty() {
                self.chunks.push(old);
            }
        }
        let start = self.current.len();
        self.current.resize(start + len, 0);
        &mut self.current[start..start + len]
    }

    pub fn reset(&mut self) {
        self.chunks.clear();
        self.current.clear();
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len() + (!self.current.is_empty() as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_buffer_spills() {
        let mut buf: InlineBuffer<u32, 3> = InlineBuffer::default();
        for i in 0..5 {
            buf.push(i);
        }
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.heap_spill_count(), 2);
    }

    #[test]
    fn ring_buffer_evicts() {
        let mut ring: RingBuffer<u32, 3> = RingBuffer::default();
        assert!(ring.push(1).is_none());
        assert!(ring.push(2).is_none());
        assert_eq!(ring.push(3), None);
        assert_eq!(ring.push(4), Some(1));
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), vec![2, 3, 4]);
    }
}
