use libc::{
    EEXIST, EINTR, POLLIN, S_IFDIR, c_int, c_void, chown, mkdir, poll, pollfd, read, stat, write,
};
use std::ffi::CString;
use std::time::{Duration, Instant};

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
pub fn create_directory(path: &str, uid: i32) -> bool {
    if path.is_empty() || !path.starts_with('/') {
        return false;
    }
    if is_directory(path) {
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
