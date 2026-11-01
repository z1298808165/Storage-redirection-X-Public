//! MediaProvider 进程内配置热重载控制。
//!
//! root 侧通过请求文件通知已经注入的 MediaProvider，Provider 内的独立线程再调用
//! 与 native hook 热路径相同的配置刷新入口。整个过程不结束 Provider，也不触碰
//! 普通应用进程，避免重建系统 FUSE volume 时由 Android 连带重建应用。
//!
//! 唤醒链路是事件驱动的：唯一写入方 `srxctl restart_media_provider` 写完请求
//! 文件后总是随发 SIGUSR2，处理器把信号经自管道转入监视线程的 `poll`（信号
//! 处理器内只做 async-signal-safe 的 write），线程平时阻塞在 poll 上空闲零唤醒，
//! 收到信号立即刷新。文件本身只承载请求 id，不作为独立唤醒源——
//! 「写文件但不发信号」在唯一写入方 srxctl 中不存在。
//!
//! 完成回执走 `HotReload` 专用标签经私有日志 socket 直送 daemon，由 daemon
//! （root，不受 sepolicy 限制）读取请求文件中的权威 request id 并补写 ack 文件；
//! MP 域对模块目录（shell_data_file）的读写会被 sepolicy 拒绝，因此 MP 侧不做
//! 任何文件落盘。

use crate::platform::module_paths;
use libc::{O_CLOEXEC, O_NONBLOCK, POLLIN, c_int, pipe2, poll, pollfd};
use std::fs;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static WATCHER_STARTED: AtomicBool = AtomicBool::new(false);
/// 自管道写端；信号处理器只写此 fd，读端由监视线程 poll。
static SIGNAL_PIPE_WRITE_FD: AtomicI32 = AtomicI32::new(-1);
static SIGNAL_PIPE_READ_FD: OnceLock<c_int> = OnceLock::new();

extern "C" fn hot_reload_signal_handler(_: libc::c_int) {
    // 自管道是 async-signal-safe 唤醒通道：处理器内仅执行 write，EAGAIN（管道满）
    // 意味着监视线程必然会被已积压的字节唤醒，无需处理。
    let fd = SIGNAL_PIPE_WRITE_FD.load(Ordering::Acquire);
    if fd >= 0 {
        // SAFETY: 向有效 fd 写入一个字节的字面量，指针在调用期间有效。
        unsafe {
            libc::write(fd, b"x".as_ptr().cast(), 1);
        }
    }
}

pub(super) fn start() {
    if WATCHER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    let mut fds = [0 as c_int; 2];
    // SAFETY: fds 是栈上有效数组；pipe2 创建带 CLOEXEC/NONBLOCK 标志的管道。
    if unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC | O_NONBLOCK) } != 0 {
        WATCHER_STARTED.store(false, Ordering::Release);
        log::warn!("media provider hot reload pipe create failed");
        return;
    }
    if SIGNAL_PIPE_READ_FD.set(fds[0]).is_err() {
        // SAFETY: fd 由当前代码持有，注册失败后负责关闭。
        unsafe { libc::close(fds[0]) };
        // SAFETY: 同上，关闭写端。
        unsafe { libc::close(fds[1]) };
        WATCHER_STARTED.store(false, Ordering::Release);
        return;
    }
    SIGNAL_PIPE_WRITE_FD.store(fds[1], Ordering::Release);

    // SIGUSR2 只负责唤醒监视线程，实际刷新仍在线程上下文执行。
    // SAFETY: 处理器只执行原子读与 write，函数指针和信号编号符合 libc::signal 约定。
    unsafe {
        libc::signal(
            libc::SIGUSR2,
            hot_reload_signal_handler as *const () as libc::sighandler_t,
        );
    }

    let spawn = std::thread::Builder::new()
        .name("srx-media-hot-reload".to_string())
        .spawn(run);
    if spawn.is_err() {
        WATCHER_STARTED.store(false, Ordering::Release);
        log::warn!("media provider hot reload watcher start failed");
    }
}

fn run() {
    let Some(read_fd) = SIGNAL_PIPE_READ_FD.get().copied() else {
        return;
    };
    let mut fds = [pollfd {
        fd: read_fd,
        events: POLLIN,
        revents: 0,
    }];
    loop {
        // 阻塞等待信号经自管道送达：无请求时空闲零唤醒，不再周期轮询请求文件。
        // SAFETY: fds 是栈上有效 pollfd 数组，-1 表示无限等待。
        let ready = unsafe { poll(fds.as_mut_ptr(), 1, -1) };
        if ready <= 0 {
            // EINTR 之外的错误没有恢复路径，继续下一轮 poll 即可（fd 状态未变）。
            continue;
        }
        drain_signal_pipe(read_fd);

        let request_text = read_request().unwrap_or_default();
        let request_id = field(&request_text, "request").unwrap_or("signal");

        crate::hook::refresh_runtime_config_after_disk_change();
        let pid = std::process::id();
        // 完成回执经 HotReload 专用标签直送 daemon（不受 verbose 门禁）：
        // MP 对模块目录无写权限（sepolicy 域切换后 shell_data_file 不可写），
        // ack 文件由 daemon 收到本消息后代为落盘，request id 亦由 daemon 从
        // 请求文件读取（MP 侧读取同样可能被 sepolicy 拒绝，此处取不到即 "signal"）。
        crate::logging::write_log(
            crate::logging::Level::Info,
            crate::logging::HOT_RELOAD_LOG_TAG,
            &format!(
                "media provider hot reload completed request={} pid={}",
                request_id, pid
            ),
        );
    }
}

/// 排空自管道积压的唤醒字节：多次信号合并为一次刷新（刷新内容与请求文件取
/// 最新值一致），避免每个信号都走一遍完整的配置重建。
fn drain_signal_pipe(read_fd: c_int) {
    let mut buf = [0u8; 64];
    loop {
        // SAFETY: 读入栈上缓冲的完整区间，长度取自同一数组，不会越界。
        let n = unsafe { libc::read(read_fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            return;
        }
    }
}

fn read_request() -> Option<String> {
    fs::read_to_string(module_paths::MEDIA_PROVIDER_HOT_RELOAD_REQUEST_FILE)
        .ok()
        .map(|text| text.trim().to_string())
}

fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.split_whitespace().find_map(|item| {
        let (item_key, value) = item.split_once('=')?;
        (item_key == key && !value.is_empty()).then_some(value)
    })
}
