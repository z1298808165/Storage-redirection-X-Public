use libc::{
    EEXIST, EINTR, POLLIN, S_IFDIR, c_int, c_void, chown, mkdir, mode_t, poll, pollfd, read, stat,
    write,
};
use std::ffi::CString;
use std::time::{Duration, Instant};

/// 模块自己创建、并 chown 给调用应用的映射/放行目标目录模式。
///
/// 与原生 MediaProvider 的公共目录口径（`02770`，`media_rw:media_rw`）只差一个
/// **other-execute**：模块把目录属主换成应用后，直接 bind 形态下应用并不在
/// `media_rw` 组，需要 other 穿越位才能进去；组位留给系统代写进程（代写进程在
/// `media_rw`/`ext_data_rw` 组里）读写。
///
/// 这里原来写的是 `0o2773`（other 可读写）：既不是原生口径，也没有任何链路需要
/// 「任意第三方 uid 往属主是某个应用的目录里直接写」——那等于把所有托管目录开给
/// 全系统。收敛到 `0o2771`，与下面真实公共目录的修正目标保持一致。
pub const MAPPED_DIR_MODE: mode_t = 0o2771;

/// 已存在的真实公共目录（如 `Download`）的修正目标模式。
///
/// 原生是 `02770 media_rw:media_rw`；模块在 namespace bind 形态下把真实目录直接
/// 交给应用，应用不在 `media_rw` 组，因此只补 other-execute 供穿越，其它位保持
/// 原生语义（不补 other 读写）。
pub const REAL_PUBLIC_DIR_MODE: mode_t = 0o2771;

/// 共享媒体后端目录的**原生**口径：`02770 media_rw:media_rw`。
///
/// 由 `media_rw` 自己管理的后端目录不额外放权，与 AOSP 完全一致。
pub const SHARED_PUBLIC_DIR_MODE: mode_t = 0o2770;

// 带总超时的阻塞读：每次 read 前先 poll 等待可读，超出截止时间立即失败返回。
// 用于伴生进程等只信任本地对端但仍不能被对端拖死的场景：
// 对端一直不发数据或只发一半时，这里会在超时后返回 false，由调用方记录日志并回收 fd。
pub fn read_all_timeout(fd: c_int, buffer: &mut [u8], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut total = 0usize;
    while total < buffer.len() {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline - now;
        // poll 的超时单位是毫秒，向上取整保证不会因为截断变成非阻塞轮询
        let remaining_ms = remaining.as_millis().min(c_int::MAX as u128) as c_int;
        let wait_ms = if remaining_ms == 0 { 1 } else { remaining_ms };
        let mut poll_fd = pollfd {
            fd,
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: poll_fd 是栈上有效的单元素 pollfd，数量参数与之匹配
        let ready = unsafe { poll(&mut poll_fd, 1, wait_ms) };
        if ready < 0 {
            if errno_is(EINTR) {
                continue;
            }
            return false;
        }
        if ready == 0 {
            return false;
        }
        // SAFETY: 读入缓冲区剩余区间，长度不超过缓冲区容量
        let n = unsafe {
            read(
                fd,
                buffer[total..].as_mut_ptr() as *mut c_void,
                buffer.len() - total,
            )
        };
        if n < 0 {
            if errno_is(EINTR) {
                continue;
            }
            return false;
        }
        if n == 0 {
            return false;
        }
        total += n as usize;
    }
    true
}

// EINTR 自动重试，直到写完或出错
pub fn write_all(fd: c_int, buffer: &[u8]) -> bool {
    let mut total = 0usize;
    while total < buffer.len() {
        let n = unsafe {
            write(
                fd,
                buffer[total..].as_ptr() as *const c_void,
                buffer.len() - total,
            )
        };
        if n < 0 {
            if errno_is(EINTR) {
                continue;
            }
            return false;
        }
        if n == 0 {
            return false;
        }
        total += n as usize;
    }
    true
}

pub fn is_directory(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    let mut st = std::mem::MaybeUninit::<stat>::uninit();
    let ret = unsafe { libc::stat(c_path.as_ptr(), st.as_mut_ptr()) };
    if ret != 0 {
        return false;
    }
    let st = unsafe { st.assume_init() };
    (st.st_mode & S_IFDIR) != 0
}

// uid >= 0 时同步设置 owner，否则沿用默认
//
// 失败原因通过 [`last_create_directory_errno`] 暴露：调用方（如沙盒目标目录
// 创建）需要把 errno 写进诊断日志，区分两类完全不同的失败：
// - 可自愈的开机竞态：SELinux 拒绝（EACCES/EPERM）与 FBE CE 密钥未就绪
//   （ENOKEY，真机实测开机 +15s 批量出现 errno=126，密钥解锁后重试即愈）；
// - 配置或环境错误：路径被普通文件占用（ENOTDIR/EEXIST 语义冲突）等，需要用户侧介入。
static LAST_CREATE_DIRECTORY_ERRNO: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);

/// 目录创建失败中属于"开机竞态、稍后重试可自愈"的 errno 集合。
///
/// 与 [`last_create_directory_errno`] 配套使用：调用方据此把这类失败降级为
/// 竞态告警而不是配置错误，避免把开机窗口的批量失败当成永久性故障处理。
pub fn errno_is_transient_boot_race(errno: c_int) -> bool {
    matches!(
        errno,
        libc::EACCES | libc::EPERM | libc::ENOKEY | libc::ENOTCONN
    )
}

/// 最近一次 [`create_directory`] 失败的 errno；成功或尚未调用时为 0。
pub fn last_create_directory_errno() -> c_int {
    LAST_CREATE_DIRECTORY_ERRNO.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn create_directory(path: &str, uid: i32) -> bool {
    if path.is_empty() || !path.starts_with('/') {
        LAST_CREATE_DIRECTORY_ERRNO.store(libc::EINVAL, std::sync::atomic::Ordering::Relaxed);
        return false;
    }
    if is_directory(path) {
        LAST_CREATE_DIRECTORY_ERRNO.store(0, std::sync::atomic::Ordering::Relaxed);
        return true;
    }

    let mut current = String::new();
    let bytes = path.as_bytes();
    let mut pos = 0usize;

    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos] == b'/' {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }

        let next = match path[pos..].find('/') {
            Some(idx) => pos + idx,
            None => path.len(),
        };
        let part = &path[pos..next];
        current.push('/');
        current.push_str(part);

        let Ok(c_path) = CString::new(current.as_str()) else {
            return false;
        };

        let ret = unsafe { mkdir(c_path.as_ptr(), 0o755) };
        if ret != 0 {
            if !errno_is(EEXIST) {
                // SAFETY: __errno 返回线程局部 errno 存储的有效指针
                let current = unsafe { *libc::__errno() };
                LAST_CREATE_DIRECTORY_ERRNO.store(current, std::sync::atomic::Ordering::Relaxed);
                return false;
            }
        } else if uid >= 0 {
            unsafe {
                chown(c_path.as_ptr(), uid as u32, uid as u32);
            }
        }

        pos = if next == path.len() {
            path.len()
        } else {
            next + 1
        };
    }

    is_directory(path)
}

fn errno_is(target: c_int) -> bool {
    unsafe { *libc::__errno() == target }
}
