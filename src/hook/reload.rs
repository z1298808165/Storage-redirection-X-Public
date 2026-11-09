use crate::config::{SettingsHub, watcher};
use crate::platform::paths;
use std::sync::atomic::{AtomicI64, Ordering};

/// 指纹复查间隔仅用于 inotify watch 不可用的兼容降级。
///
/// 正常路径由每个 app-specialize 进程自行创建 inotify 实例并由事件线程处理；watch
/// 不可用时才每秒做廉价指纹（目录/根文件 stat），发现可能变化后才做完整扫描。
const RUNTIME_CONFIG_RELOAD_INTERVAL_MS: i64 = 1000;
/// 事件通道在线时无需指纹复查；事件读取失败会把 `events_live` 置为 false，转入上面的
/// 降级节拍。它也供其它热路径共享同一判据。
const NO_FINGERPRINT_FALLBACK_MS: i64 = i64::MAX / 4;
const RUNTIME_CONFIG_EVENT_RELOAD_INTERVAL_MS: i64 = 100;
const RUNTIME_CONFIG_EVENT_RELOAD_WINDOW_MS: i64 = 1500;

static LAST_RUNTIME_CONFIG_RELOAD_CHECK_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);
static FORCE_RUNTIME_CONFIG_RELOAD_UNTIL_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);

pub(super) enum RuntimeReloadCheck {
    Skipped,
    Checked { did_reload: bool },
}

impl RuntimeReloadCheck {
    pub(super) fn did_reload(&self) -> bool {
        matches!(self, Self::Checked { did_reload: true })
    }
}

pub(super) fn force_after_disk_change() -> bool {
    let now_ms = paths::monotonic_ms();
    FORCE_RUNTIME_CONFIG_RELOAD_UNTIL_MS.store(
        now_ms.saturating_add(RUNTIME_CONFIG_EVENT_RELOAD_WINDOW_MS),
        Ordering::Relaxed,
    );
    LAST_RUNTIME_CONFIG_RELOAD_CHECK_MS.store(now_ms, Ordering::Relaxed);
    SettingsHub::instance().reload_force()
}

pub(super) fn poll_or_check_throttled() -> RuntimeReloadCheck {
    if watcher::poll_changed() {
        return RuntimeReloadCheck::Checked {
            did_reload: force_after_disk_change(),
        };
    }

    let now_ms = paths::monotonic_ms();
    let force_until_ms = FORCE_RUNTIME_CONFIG_RELOAD_UNTIL_MS.load(Ordering::Relaxed);
    let is_event_window = now_ms <= force_until_ms;
    let reload_interval_ms = reload_interval_ms(is_event_window);
    let last_ms = LAST_RUNTIME_CONFIG_RELOAD_CHECK_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last_ms) < reload_interval_ms {
        return RuntimeReloadCheck::Skipped;
    }
    if LAST_RUNTIME_CONFIG_RELOAD_CHECK_MS
        .compare_exchange(last_ms, now_ms, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return RuntimeReloadCheck::Skipped;
    }

    let did_reload = if is_event_window {
        SettingsHub::instance().reload_force()
    } else {
        SettingsHub::instance().reload_if_changed()
    };
    RuntimeReloadCheck::Checked { did_reload }
}

fn reload_interval_ms(is_event_window: bool) -> i64 {
    if is_event_window {
        RUNTIME_CONFIG_EVENT_RELOAD_INTERVAL_MS
    } else {
        fallback_reload_interval_ms()
    }
}

/// 非事件窗口内的指纹复查间隔：事件通道在线时不复查，否则降级为每秒廉价复查。
///
/// 完整目录枚举仅在廉价配置指纹显示可能变化时发生。
pub(crate) fn fallback_reload_interval_ms() -> i64 {
    if watcher::events_live() {
        NO_FINGERPRINT_FALLBACK_MS
    } else {
        RUNTIME_CONFIG_RELOAD_INTERVAL_MS
    }
}
