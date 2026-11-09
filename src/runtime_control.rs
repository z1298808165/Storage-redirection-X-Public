use crate::platform::module_paths;
use std::sync::atomic::{AtomicBool, Ordering};

/// 运行开关文件所在目录是否已被 inotify 监听。
///
/// 为真时主循环不需要任何固定节拍的探询：开关改写会作为目录事件送进来。为假（inotify
/// 不可用）才退回固定节拍。
#[allow(dead_code)] // quality-allow(lint-suppression): 仅 daemon 二进制目标读写，cdylib 进程不做主循环等待。
static DISABLE_WATCH_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 登记「开关目录已监听」。
#[allow(dead_code)] // quality-allow(lint-suppression): 仅 daemon 二进制目标读写。
pub fn set_disable_watch_active(active: bool) {
    DISABLE_WATCH_ACTIVE.store(active, Ordering::Release);
}

/// 开关目录是否已有事件源。
#[allow(dead_code)] // quality-allow(lint-suppression): 仅 daemon 二进制目标读写。
pub fn disable_watch_active() -> bool {
    DISABLE_WATCH_ACTIVE.load(Ordering::Acquire)
}

pub fn is_module_runtime_enabled() -> bool {
    std::fs::metadata(module_paths::RUNTIME_DISABLE_FILE).is_err()
}
