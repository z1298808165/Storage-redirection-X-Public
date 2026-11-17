//! procfs 直读通道：只用裸系统调用，绕开本进程内所有 PLT hook。
//!
//! 为什么需要一条独立通道：本模块在 zygisk 注入的进程里读取自身状态
//! （`/proc/self/mountinfo`、`/proc/<pid>/status` 等），而同一进程里必然挂着 PLT
//! hook——可以是本模块自己的路径改写 hook，也可以是第三方 zygisk 模块。任何挂在
//! `open`/`read`/`fopen` 上的 hook 都能改写这些内容，于是"读了什么"不再等于
//! "内核说了什么"。对就绪判定这类安全判定（少看见一条挂载就会空等满预算并被 AMS
//! 判启动超时）来说，这种不确定性不可接受。
//!
//! 因此这里只用 `SYS_openat` / `SYS_read` / `SYS_close`：不经过 libc 包装函数，也就
//! 不经过任何 PLT 改写。同理，本模块将来若要对外净化 procfs 视图，内部判定必须走这条
//! 通道，否则净化会把自身的就绪判定一起抹掉。
//!
//! 约束：只读、一次性读取、不重试等待（失败即返回 `None`），不做任何轮询。

use std::ffi::CString;

/// 单次读取允许的最大字节数。
///
/// `mountinfo` 在层数多的设备上可达数十 KB，进程表类文件更大；1 MiB 足够覆盖，
/// 同时给调用方一个明确上界，避免脏输入把内存拖爆。
const MAX_READ_BYTES: usize = 1024 * 1024;
/// 初始缓冲与增长步长。
///
/// 从 8 KiB 起倍增：常见 procfs 文件（status/cmdline）一次读满即结束，只有
/// `mountinfo` 这类大文件才递增到几十 KB，避免每次调用都预留大块内存。
const INITIAL_BYTES: usize = 8 * 1024;

/// 用裸系统调用读取一个 procfs 文本文件。
///
/// 返回 `None` 表示打开或读取失败（含读到 0 字节），调用方按"通道不可用"处理，
/// 不要把它当成"文件为空"——两者的后续行为不同。
///
/// `max_bytes` 会被夹到 `[1, MAX_READ_BYTES]`：调用方给的只是期望上限，不是信任值。
pub fn read_proc_file(path: &str, max_bytes: usize) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let limit = max_bytes.clamp(1, MAX_READ_BYTES);
    let c_path = CString::new(path).ok()?;

    // SAFETY: c_path 是以 NUL 结尾的合法路径且在本作用域内保持存活；raw syscall
    // 不经 libc 包装，因此不会被本进程内的 PLT hook 改写。失败返回负值。
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return None;
    }
    let fd = fd as libc::c_int;

    let mut buffer: Vec<u8> = vec![0; INITIAL_BYTES.min(limit)];
    let mut total = 0usize;
    loop {
        if total >= buffer.len() {
            if buffer.len() >= limit {
                break;
            }
            let next = (buffer.len() * 2).min(limit);
            buffer.resize(next, 0);
        }
        let want = buffer.len() - total;
        // SAFETY: fd 由上面的 openat 成功返回；写入区间是 buffer 内未填充的部分，
        // 长度按剩余容量给出，因此内核只会写到已分配内存范围内。
        let n = unsafe {
            libc::syscall(
                libc::SYS_read,
                fd as libc::c_long,
                buffer[total..].as_mut_ptr(),
                want as libc::c_long,
            )
        };
        if n < 0 {
            if crate::platform::errno::last() == libc::EINTR {
                // 被信号打断不算失败，重新读同一段剩余区间。
                continue;
            }
            // SAFETY: fd 为本函数打开的合法描述符，此处只关闭一次。
            unsafe { libc::syscall(libc::SYS_close, fd as libc::c_long) };
            return None;
        }
        if n == 0 {
            break;
        }
        total += n as usize;
    }
    // SAFETY: fd 为本函数打开的合法描述符，此处只关闭一次。
    unsafe { libc::syscall(libc::SYS_close, fd as libc::c_long) };

    if total == 0 {
        return None;
    }
    buffer.truncate(total);
    Some(String::from_utf8_lossy(&buffer).into_owned())
}
