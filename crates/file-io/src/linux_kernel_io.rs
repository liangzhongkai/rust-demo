//! # Linux 内核 I/O 机制
//!
//! 补齐标准 `std::fs` 未直接暴露的内核路径：
//!
//! 1. **数据路径**：DMA → page cache → CPU copy → 用户态
//! 2. **Buffer IO vs Direct IO**：page cache 缓冲读 vs `O_DIRECT` 绕过缓存
//! 3. **BIO vs NIO**：阻塞 `read` vs `O_NONBLOCK`（pipe）/ libaio 异步完成
//! 4. **Linux AIO (libaio)**：`io_setup` / `io_submit` / `io_getevents`
//! 5. **内核零拷贝**：`mmap`（memmap2）、`sendfile`、`splice`
//!
//! 需要 Linux；非 Linux 平台打印跳过说明。

#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::Instant;

const BLOCK: usize = 4096;
const DEMO_BYTES: usize = BLOCK * 4;

// ============================================================================
// 0. 数据路径：DMA / page cache / CPU copy / context switch
// ============================================================================
pub mod data_path {

    #[derive(Debug, Clone, Copy)]
    pub struct IoPathProfile {
        pub name: &'static str,
        pub user_copy: u8,
        pub bypass_page_cache: bool,
        pub syscall_per_chunk: u8,
        pub typical_ctx_switches: &'static str,
    }

    pub fn profiles() -> [IoPathProfile; 5] {
        [
            IoPathProfile {
                name: "read/write (buffered)",
                user_copy: 1,
                bypass_page_cache: false,
                syscall_per_chunk: 1,
                typical_ctx_switches: "user→kernel→user 各 1 次",
            },
            IoPathProfile {
                name: "O_DIRECT",
                user_copy: 1,
                bypass_page_cache: true,
                syscall_per_chunk: 1,
                typical_ctx_switches: "user→kernel→user；无 page cache 命中",
            },
            IoPathProfile {
                name: "mmap (memmap2)",
                user_copy: 0,
                bypass_page_cache: false,
                syscall_per_chunk: 0,
                typical_ctx_switches: "fault 时内核填 page；热读无额外 copy",
            },
            IoPathProfile {
                name: "sendfile",
                user_copy: 0,
                bypass_page_cache: false,
                syscall_per_chunk: 1,
                typical_ctx_switches: "数据留在内核；page cache→socket/file buffer",
            },
            IoPathProfile {
                name: "splice (pipe)",
                user_copy: 0,
                bypass_page_cache: false,
                syscall_per_chunk: 1,
                typical_ctx_switches: "内核 pipe buffer 中转；至少一端是 pipe",
            },
        ]
    }

    pub fn demonstrate() {
        println!("## 0. 数据路径：DMA / CPU copy / context switch");
        println!("  磁盘/SSD ──DMA──► 内核 buffer (page cache 或 bounce buffer)");
        println!("  page cache ──CPU copy──► 用户 buffer     ← 普通 read()");
        println!("  page cache ──映射──► 用户地址空间         ← mmap，无额外用户态 copy");
        println!("  page cache ──内核搬运──► socket/file fd   ← sendfile / splice");
        println!("  O_DIRECT：DMA 直达对齐的用户 buffer，绕过 page cache");
        println!();
        for p in profiles() {
            println!(
                "  {:22} user_copy={} bypass_cache={} syscall/chunk={} ctx_switch={}",
                p.name,
                p.user_copy,
                p.bypass_page_cache,
                p.syscall_per_chunk,
                p.typical_ctx_switches
            );
        }
        println!();
    }
}

// ============================================================================
// 1. Buffer IO vs Direct IO
// ============================================================================
#[cfg(target_os = "linux")]
pub mod buffer_vs_direct {
    use super::*;

    fn write_test_file(path: &Path) -> io::Result<()> {
        let mut f = File::create(path)?;
        let payload: Vec<u8> = (0..DEMO_BYTES).map(|i| (i % 251) as u8).collect();
        f.write_all(&payload)?;
        f.sync_all()?;
        Ok(())
    }

    fn read_buffered(path: &Path) -> io::Result<usize> {
        let mut f = File::open(path)?;
        let mut buf = vec![0u8; DEMO_BYTES];
        let n = std::io::Read::read(&mut f, &mut buf)?;
        Ok(n)
    }

    fn alloc_aligned(size: usize, align: usize) -> io::Result<*mut u8> {
        let mut ptr: *mut libc::c_void = std::ptr::null_mut();
        let rc = unsafe { libc::posix_memalign(&mut ptr, align, size) };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc));
        }
        Ok(ptr as *mut u8)
    }

    fn read_direct(path: &Path) -> io::Result<usize> {
        let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes())?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECT,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let buf = alloc_aligned(DEMO_BYTES, BLOCK)?;
        let n = unsafe {
            libc::read(
                fd,
                buf as *mut libc::c_void,
                DEMO_BYTES,
            )
        };
        unsafe {
            libc::free(buf as *mut libc::c_void);
            libc::close(fd);
        }
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    pub fn demonstrate() {
        println!("## 1. Buffer IO vs Direct IO (O_DIRECT)");
        let dir = std::env::temp_dir().join("file-io-kernel-buf-direct");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.bin");
        write_test_file(&path).unwrap();

        let t0 = Instant::now();
        let n_buf = read_buffered(&path).unwrap();
        let buf_us = t0.elapsed().as_micros();

        match read_direct(&path) {
            Ok(n_dir) => {
                let t1 = Instant::now();
                let _ = read_direct(&path).unwrap();
                let direct_us = t1.elapsed().as_micros();
                println!(
                    "  buffered read: {n_buf} bytes (~{buf_us} µs) — 走 page cache，可能命中缓存"
                );
                println!(
                    "  O_DIRECT read: {n_dir} bytes (~{direct_us} µs) — 绕过 page cache，buffer 需 {BLOCK} 对齐"
                );
            }
            Err(e) => {
                println!("  buffered read: {n_buf} bytes (~{buf_us} µs)");
                println!(
                    "  O_DIRECT 跳过: {e}（tmpfs/部分 FS 不支持 O_DIRECT，生产用 ext4/xfs 裸盘）"
                );
            }
        }
        println!("  规则：DB/时序库热写用 O_DIRECT 防 cache 污染；重复读 mmap/缓冲更快\n");
    }
}

#[cfg(not(target_os = "linux"))]
pub mod buffer_vs_direct {
    pub fn demonstrate() {
        println!("## 1. Buffer IO vs Direct IO — 需要 Linux\n");
    }
}

// ============================================================================
// 2. BIO vs NIO
// ============================================================================
#[cfg(target_os = "linux")]
pub mod bio_vs_nio {
    use super::*;

    fn blocking_file_read(path: &Path) -> io::Result<usize> {
        let mut f = File::open(path)?;
        let mut buf = [0u8; BLOCK];
        std::io::Read::read(&mut f, &mut buf)
    }

    fn nonblocking_pipe_read() -> io::Result<&'static str> {
        let mut pipefd = [0i32; 2];
        if unsafe { libc::pipe(pipefd.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let read_fd = pipefd[0];
        let flags = unsafe { libc::fcntl(read_fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(read_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut buf = [0u8; 64];
        let n = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        unsafe {
            libc::close(pipefd[0]);
            libc::close(pipefd[1]);
        }
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN) {
                return Ok("EAGAIN（无数据立即返回，不阻塞）");
            }
            return Err(err);
        }
        Ok("读到数据")
    }

    pub fn demonstrate() {
        println!("## 2. BIO vs NIO");
        let dir = std::env::temp_dir().join("file-io-kernel-bio-nio");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("small.bin");
        fs::write(&path, b"bio-demo").unwrap();

        let n = blocking_file_read(&path).unwrap();
        println!("  BIO 文件 read: {n} bytes — 常规文件在 Linux 上始终阻塞到数据就绪");

        match nonblocking_pipe_read() {
            Ok(msg) => println!("  NIO pipe read (O_NONBLOCK): {msg}"),
            Err(e) => println!("  NIO pipe demo err: {e}"),
        }

        println!("  要点：普通文件没有真正的 O_NONBLOCK；文件异步靠 libaio/io_uring 或线程池");
        println!("  tokio::fs = spawn_blocking + BIO；热路径用 io_uring（Linux 5.1+）\n");
    }
}

#[cfg(not(target_os = "linux"))]
pub mod bio_vs_nio {
    pub fn demonstrate() {
        println!("## 2. BIO vs NIO — 需要 Linux\n");
    }
}

// ============================================================================
// 3. Linux AIO (libaio)
// ============================================================================
#[cfg(target_os = "linux")]
pub mod linux_aio {
    use super::*;

    type AioContext = u64;

    const IOCB_CMD_PREAD: u16 = 0;

    #[repr(C)]
    struct IoCb {
        aio_data: u64,
        aio_key: u32,
        aio_lio_opcode: u16,
        aio_reqprio: i16,
        aio_fildes: u32,
        aio_buf: u64,
        aio_nbytes: u64,
        aio_offset: i64,
        aio_reserved2: u64,
        aio_flags: u32,
        aio_resfd: u32,
    }

    #[repr(C)]
    struct IoEvent {
        data: u64,
        obj: u64,
        res: i64,
        res2: i64,
    }

    fn io_setup(nr: u32, ctx: &mut AioContext) -> i64 {
        unsafe { libc::syscall(libc::SYS_io_setup, nr, ctx as *mut _) as i64 }
    }

    fn io_destroy(ctx: AioContext) -> i64 {
        unsafe { libc::syscall(libc::SYS_io_destroy, ctx) as i64 }
    }

    fn io_submit(ctx: AioContext, nr: u64, iocbpp: *const *const IoCb) -> i64 {
        unsafe { libc::syscall(libc::SYS_io_submit, ctx, nr, iocbpp) as i64 }
    }

    fn io_getevents(
        ctx: AioContext,
        min_nr: u64,
        max_nr: u64,
        events: *mut IoEvent,
        timeout: *const libc::timespec,
    ) -> i64 {
        unsafe {
            libc::syscall(
                libc::SYS_io_getevents,
                ctx,
                min_nr,
                max_nr,
                events,
                timeout,
            ) as i64
        }
    }

    fn prep_pread(iocb: &mut IoCb, fd: i32, buf: *mut u8, count: usize, offset: i64) {
        *iocb = IoCb {
            aio_data: 0,
            aio_key: 0,
            aio_lio_opcode: IOCB_CMD_PREAD,
            aio_reqprio: 0,
            aio_fildes: fd as u32,
            aio_buf: buf as u64,
            aio_nbytes: count as u64,
            aio_offset: offset,
            aio_reserved2: 0,
            aio_flags: 0,
            aio_resfd: 0,
        };
    }

    fn alloc_aligned(size: usize, align: usize) -> io::Result<*mut u8> {
        let mut ptr: *mut libc::c_void = std::ptr::null_mut();
        let rc = unsafe { libc::posix_memalign(&mut ptr, align, size) };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc));
        }
        Ok(ptr as *mut u8)
    }

    pub fn pread_async(path: &Path) -> io::Result<(usize, u128)> {
        let content = b"libaio-pread-demo!!";
        // libaio 磁盘 I/O 要求 O_DIRECT + 对齐 buffer/长度
        let aligned_len = ((content.len() + BLOCK - 1) / BLOCK) * BLOCK;
        let mut file_buf = vec![0u8; aligned_len];
        file_buf[..content.len()].copy_from_slice(content);
        fs::write(path, &file_buf)?;

        let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes())?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECT,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut ctx: AioContext = 0;
        if io_setup(1, &mut ctx) < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }

        let buf = alloc_aligned(aligned_len, BLOCK)?;
        let mut iocb = IoCb {
            aio_data: 0,
            aio_key: 0,
            aio_lio_opcode: 0,
            aio_reqprio: 0,
            aio_fildes: 0,
            aio_buf: 0,
            aio_nbytes: 0,
            aio_offset: 0,
            aio_reserved2: 0,
            aio_flags: 0,
            aio_resfd: 0,
        };
        prep_pread(&mut iocb, fd, buf, aligned_len, 0);
        let iocb_ptr = &iocb as *const IoCb;
        let iocbpp = &iocb_ptr;

        let t0 = Instant::now();
        if io_submit(ctx, 1, iocbpp) != 1 {
            let _ = io_destroy(ctx);
            unsafe {
                libc::free(buf as *mut libc::c_void);
                libc::close(fd);
            }
            return Err(io::Error::last_os_error());
        }

        let mut events = [IoEvent {
            data: 0,
            obj: 0,
            res: 0,
            res2: 0,
        }];
        let n = io_getevents(ctx, 1, 1, events.as_mut_ptr(), std::ptr::null());
        let elapsed = t0.elapsed().as_micros();

        let _ = io_destroy(ctx);
        unsafe {
            libc::free(buf as *mut libc::c_void);
            libc::close(fd);
        }

        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if events[0].res < 0 {
            return Err(io::Error::from_raw_os_error(-events[0].res as i32));
        }
        Ok((content.len(), elapsed))
    }

    pub fn demonstrate() {
        println!("## 3. Linux AIO (libaio)");
        let dir = std::env::temp_dir().join("file-io-kernel-aio");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aio.bin");

        match pread_async(&path) {
            Ok((n, us)) => {
                println!("  io_submit(PREAD+O_DIRECT) → io_getevents: {n} bytes (~{us} µs)");
                println!("  内容 = \"libaio-pread-demo!!\"");
            }
            Err(e) => {
                println!("  libaio 演示失败: {e}");
                println!("  WSL/容器可能未启用 libaio；生产更推荐 io_uring");
            }
        }
        println!("  路径：提交时不阻塞 → 完成事件在 io_getevents 取回（减少 BIO 等待）");
        println!("  注意：libaio 只支持 O_DIRECT 打开的 fd 做真正的异步磁盘 I/O");
        println!("  现代替代：io_uring（一次 enter 批量提交+收割，无 aio 的 O_DIRECT 限制）\n");
    }
}

#[cfg(not(target_os = "linux"))]
pub mod linux_aio {
    pub fn demonstrate() {
        println!("## 3. Linux AIO (libaio) — 需要 Linux\n");
    }
}

// ============================================================================
// 4. 内核零拷贝：mmap / sendfile / splice
// ============================================================================
#[cfg(target_os = "linux")]
pub mod zero_copy {
    use super::*;
    use memmap2::Mmap;
    use std::os::unix::io::{AsRawFd, FromRawFd};

    const PAYLOAD: &[u8] = b"zero-copy-kernel-paths-demo-content";

    fn demo_mmap(path: &Path) -> io::Result<&'static str> {
        let f = File::open(path)?;
        let map = unsafe { Mmap::map(&f)? };
        let ok = map.starts_with(PAYLOAD);
        Ok(if ok {
            "mmap 命中 page cache 映射，解析层 &[u8] 无用户态 copy"
        } else {
            "mmap 内容不匹配"
        })
    }

    fn demo_sendfile(src: &Path, dst: &Path) -> io::Result<usize> {
        let src_f = File::open(src)?;
        let dst_f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)?;
        let src_fd = src_f.as_raw_fd();
        let dst_fd = dst_f.as_raw_fd();
        let mut offset: libc::off_t = 0;
        let n = unsafe {
            libc::sendfile(
                dst_fd,
                src_fd,
                &mut offset,
                PAYLOAD.len(),
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        dst_f.sync_all()?;
        Ok(n as usize)
    }

    fn demo_splice(src: &Path, dst: &Path) -> io::Result<usize> {
        let src_f = File::open(src)?;
        let src_fd = src_f.as_raw_fd();

        let mut pipefd = [0i32; 2];
        if unsafe { libc::pipe2(pipefd.as_mut_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }

        // file → pipe（内核 page cache → pipe buffer，不经用户态）
        let mut off: libc::loff_t = 0;
        let to_pipe = unsafe {
            libc::splice(
                src_fd,
                &mut off,
                pipefd[1],
                std::ptr::null_mut(),
                PAYLOAD.len(),
                0,
            )
        };
        if to_pipe < 0 {
            unsafe {
                libc::close(pipefd[0]);
                libc::close(pipefd[1]);
            }
            return Err(io::Error::last_os_error());
        }

        let dst_f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)?;
        let dst_fd = dst_f.as_raw_fd();

        // pipe → file
        let mut moved = 0usize;
        while moved < PAYLOAD.len() {
            let n = unsafe {
                libc::splice(
                    pipefd[0],
                    std::ptr::null_mut(),
                    dst_fd,
                    std::ptr::null_mut(),
                    PAYLOAD.len() - moved,
                    0,
                )
            };
            if n < 0 {
                unsafe {
                    libc::close(pipefd[0]);
                    libc::close(pipefd[1]);
                }
                return Err(io::Error::last_os_error());
            }
            if n == 0 {
                break;
            }
            moved += n as usize;
        }

        unsafe {
            libc::close(pipefd[0]);
            libc::close(pipefd[1]);
        }
        dst_f.sync_all()?;
        Ok(moved)
    }

    /// sendfile 典型场景：文件 → socket（nginx、静态文件服务）
    fn demo_sendfile_socket(src: &Path) -> io::Result<usize> {
        let src_f = File::open(src)?;
        let src_fd = src_f.as_raw_fd();

        let mut sv = [0i32; 2];
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sv.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }

        let sock_wr = unsafe { File::from_raw_fd(sv[1]) };
        let sock_rd = unsafe { File::from_raw_fd(sv[0]) };
        let wr_fd = sock_wr.as_raw_fd();

        let mut offset: libc::off_t = 0;
        let n = unsafe {
            libc::sendfile(
                wr_fd,
                src_fd,
                &mut offset,
                PAYLOAD.len(),
            )
        };
        drop(sock_wr);

        if n < 0 {
            drop(sock_rd);
            return Err(io::Error::last_os_error());
        }

        let mut buf = vec![0u8; PAYLOAD.len()];
        let mut rd = sock_rd;
        std::io::Read::read_exact(&mut rd, &mut buf)?;
        drop(rd);
        Ok(buf.len())
    }

    pub fn demonstrate() {
        println!("## 4. 内核零拷贝：mmap / sendfile / splice");
        let dir = std::env::temp_dir().join("file-io-kernel-zerocopy");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let src = dir.join("source.bin");
        let dst_sendfile = dir.join("dest.sendfile");
        let dst_splice = dir.join("dest.splice");
        fs::write(&src, PAYLOAD).unwrap();

        match demo_mmap(&src) {
            Ok(msg) => println!("  [mmap/memmap2] {msg}"),
            Err(e) => println!("  [mmap] {e}"),
        }

        match demo_sendfile(&src, &dst_sendfile) {
            Ok(n) => {
                let got = fs::read(&dst_sendfile).unwrap();
                println!(
                    "  [sendfile] file→file {n} bytes，内核搬运 page cache，verified={}",
                    got == PAYLOAD
                );
            }
            Err(e) => println!("  [sendfile] file→file: {e}"),
        }

        match demo_sendfile_socket(&src) {
            Ok(n) => println!("  [sendfile] file→socket {n} bytes（nginx 静态文件典型路径）"),
            Err(e) => println!("  [sendfile] file→socket: {e}"),
        }

        match demo_splice(&src, &dst_splice) {
            Ok(n) => {
                let got = fs::read(&dst_splice).unwrap();
                println!(
                    "  [splice] file→pipe→file {n} bytes，pipe 作内核中转，verified={}",
                    got == PAYLOAD
                );
            }
            Err(e) => println!("  [splice] {e}"),
        }

        println!("  对比：read+write 多一次用户态 CPU copy；sendfile/splice 数据不经过用户 buffer");
        println!("  splice 优势：可对接任意支持 splice 的 fd（pipe、socket、tun 等）\n");
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sendfile_roundtrip() {
            let dir = std::env::temp_dir().join("file-io-kernel-test");
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let src = dir.join("t.bin");
            let dst = dir.join("t.out");
            fs::write(&src, PAYLOAD).unwrap();
            let n = demo_sendfile(&src, &dst).unwrap();
            assert_eq!(n, PAYLOAD.len());
            assert_eq!(fs::read(&dst).unwrap(), PAYLOAD);
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub mod zero_copy {
    pub fn demonstrate() {
        println!("## 4. 内核零拷贝 (mmap/sendfile/splice) — 需要 Linux\n");
    }
}

pub fn demonstrate() {
    println!("--- Linux 内核 I/O 机制 ---\n");
    data_path::demonstrate();
    buffer_vs_direct::demonstrate();
    bio_vs_nio::demonstrate();
    linux_aio::demonstrate();
    zero_copy::demonstrate();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_path_profiles_non_empty() {
        assert_eq!(data_path::profiles().len(), 5);
    }
}
