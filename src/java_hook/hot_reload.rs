//! MediaProvider 进程内配置热重载控制。
//!
//! root 侧通过请求文件通知已经注入的 MediaProvider，Provider 内的独立线程再调用
//! 与 native hook 热路径相同的配置刷新入口。整个过程不结束 Provider，也不触碰
//! 普通应用进程，避免重建系统 FUSE volume 时由 Android 连带重建应用。

use crate::platform::{self, module_paths};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(200);
static WATCHER_STARTED: AtomicBool = AtomicBool::new(false);
static SIGNAL_PENDING: AtomicBool = AtomicBool::new(false);

extern "C" fn hot_reload_signal_handler(_: libc::c_int) {
    SIGNAL_PENDING.store(true, Ordering::Release);
}

pub(super) fn start() {
    if WATCHER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    // SIGUSR2 只负责唤醒轮询线程，实际刷新仍在线程上下文执行。
    // SAFETY: 处理器只执行原子写入，函数指针和信号编号符合 libc::signal 约定。
    unsafe {
        libc::signal(
            libc::SIGUSR2,
            hot_reload_signal_handler as *const () as libc::sighandler_t,
        );
    }

    let spawn = thread::Builder::new()
        .name("srx-media-hot-reload".to_string())
        .spawn(run);
    if spawn.is_err() {
        WATCHER_STARTED.store(false, Ordering::Release);
        log::warn!("media provider hot reload watcher start failed");
    }
}

fn run() {
    let mut last_request = read_request().unwrap_or_default();
    loop {
        let signal_requested = SIGNAL_PENDING.swap(false, Ordering::AcqRel);
        let request = read_request();
        if !signal_requested && request.is_none() {
            thread::sleep(POLL_INTERVAL);
            continue;
        }

        let request_text = request.as_deref().unwrap_or_default();
        if !signal_requested && (request_text.is_empty() || request_text == last_request) {
            thread::sleep(POLL_INTERVAL);
            continue;
        }

        let request_id = field(request_text, "request").unwrap_or("signal");
        if !request_text.is_empty() {
            last_request = request_text.to_string();
        }

        crate::hook::refresh_runtime_config_after_disk_change();
        let pid = std::process::id();
        let boot_id = platform::read_boot_id();
        let ack = format!(
            "stage=hot_reload_ok request={} pid={} boot_id={}\n",
            request_id,
            pid,
            if boot_id.is_empty() {
                "unknown"
            } else {
                &boot_id
            }
        );
        write_ack(&ack);
        log::info!(
            "media provider hot reload completed request={} pid={}",
            request_id,
            pid
        );
        thread::sleep(POLL_INTERVAL);
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

fn write_ack(content: &str) {
    let path = std::path::Path::new(module_paths::MEDIA_PROVIDER_HOT_RELOAD_ACK_FILE);
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let temp = path.with_extension("ack.tmp");
    if fs::write(&temp, content).is_err() {
        return;
    }
    let _ = fs::rename(&temp, path);
    if let Ok(c_path) = std::ffi::CString::new(module_paths::MEDIA_PROVIDER_HOT_RELOAD_ACK_FILE) {
        // SAFETY: c_path 在调用期间保持有效，路径来自模块固定常量。
        unsafe { libc::chmod(c_path.as_ptr(), 0o644) };
    }
}
