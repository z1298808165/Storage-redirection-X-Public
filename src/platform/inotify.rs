#[allow(dead_code)] // quality-allow(lint-suppression): cdylib 目标不编译 daemon 调用方，但测试和 daemon 目标需要 wd 字段。
pub struct Event<'a> {
    pub wd: i32,
    pub mask: u32,
    pub name: &'a [u8],
}

/// 文件变化等待器：监听父目录以覆盖原子替换，仅接受指定文件的事件。
/// 每次业务等待独占一个实例，不跨 fork 共享事件队列。
pub struct FileChangeWaiter {
    fd: super::unique_fd::UniqueFd,
    targets: Vec<(i32, Vec<u8>)>,
}

impl FileChangeWaiter {
    pub fn new(paths: &[&str]) -> Option<Self> {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: 标志只请求非阻塞和执行时关闭，成功后的 fd 由本实例独占。
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let mut waiter = Self {
            fd: super::unique_fd::UniqueFd::new(fd),
            targets: Vec::new(),
        };
        for path in paths {
            let path = std::path::Path::new(path);
            let parent = std::ffi::CString::new(path.parent()?.as_os_str().as_bytes()).ok()?;
            // SAFETY: 父目录路径有效，fd 为本实例的 inotify 描述符。
            let wd = unsafe {
                libc::inotify_add_watch(
                    fd,
                    parent.as_ptr(),
                    libc::IN_CLOSE_WRITE
                        | libc::IN_MOVED_TO
                        | libc::IN_DELETE
                        | libc::IN_CREATE
                        | libc::IN_DELETE_SELF
                        | libc::IN_MOVE_SELF,
                )
            };
            if wd < 0 {
                return None;
            }
            waiter
                .targets
                .push((wd, path.file_name()?.as_bytes().to_vec()));
        }
        Some(waiter)
    }

    pub fn fd(&self) -> i32 {
        self.fd.get()
    }

    /// 排空事件；监听失效或队列溢出时也返回变化，由调用方重读事实或重建监听。
    pub fn changed(&self) -> bool {
        let mut buffer = [0u8; 4096];
        let mut changed = false;
        loop {
            // SAFETY: 缓冲有效且 fd 非阻塞，每轮只解析实际读入的长度。
            let n = unsafe { libc::read(self.fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
            if n < 0 {
                let errno = std::io::Error::last_os_error().raw_os_error();
                if errno == Some(libc::EINTR) {
                    continue;
                }
                return changed || errno != Some(libc::EAGAIN);
            }
            if n == 0 {
                return true;
            }
            for_each_event(&buffer[..n as usize], |event| {
                let name = event
                    .name
                    .split(|byte| *byte == 0)
                    .next()
                    .unwrap_or_default();
                changed |= event.mask
                    & (libc::IN_Q_OVERFLOW
                        | libc::IN_IGNORED
                        | libc::IN_DELETE_SELF
                        | libc::IN_MOVE_SELF)
                    != 0
                    || self
                        .targets
                        .iter()
                        .any(|(wd, target)| *wd == event.wd && target == name);
            });
            if changed {
                return true;
            }
        }
    }

    /// 等待真实变化或绝对截止时间；信号打断与无关文件写入不延长总预算。
    pub fn wait_until(&self, deadline_ms: i64) -> bool {
        loop {
            let remaining = deadline_ms.saturating_sub(super::paths::monotonic_ms());
            if remaining <= 0 {
                return false;
            }
            let mut descriptor = libc::pollfd {
                fd: self.fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: 描述符为有效单元素数组，超时受绝对截止时间约束。
            let result =
                unsafe { libc::poll(&mut descriptor, 1, remaining.min(i32::MAX as i64) as i32) };
            if result < 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return true;
            }
            if result == 0 {
                return false;
            }
            if descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
                || self.changed()
            {
                return true;
            }
        }
    }
}

/// 遍历一次 inotify read 缓冲区，统一处理事件头完整性、长度溢出和尾部截断。
pub fn for_each_event(mut buffer: &[u8], mut callback: impl FnMut(&Event<'_>)) {
    while let Some((header, payload)) = buffer.split_first_chunk::<16>() {
        let wd = i32::from_ne_bytes([header[0], header[1], header[2], header[3]]);
        let mask = u32::from_ne_bytes([header[4], header[5], header[6], header[7]]);
        let length = u32::from_ne_bytes([header[12], header[13], header[14], header[15]]) as usize;
        let Some(name) = payload.get(..length) else {
            break;
        };
        callback(&Event { wd, mask, name });
        buffer = &payload[length..];
    }
}
