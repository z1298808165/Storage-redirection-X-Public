use crate::hook::stats::InterceptHub;
use std::sync::atomic::{AtomicBool, Ordering};

/// 是否已完成 fork 前预初始化。Hyos 的 spawner 会在 fork 前调用
/// `prepare_pre_fork`，子进程随后复用已初始化的 once_cell/RwLock。
static HYOS_PREPARED: AtomicBool = AtomicBool::new(false);

/// fork 前在 hyos_spawner 里初始化 once_cell/RwLock，避免子进程首次访问时
/// 撞上 fork 瞬间被其他线程持有的锁。
pub fn prepare_pre_fork() {
    if HYOS_PREPARED.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = InterceptHub::instance();
    log::info!("hyos pre-fork prepare done");
}

/// Hyos runtime 在 post-fork 子进程回调里调用；这里复用现有 native hook 安装链。
///
/// 约束：
/// - 回调里不能启动线程；
/// - 当前 `install` 仍会分配内存，后续需要拆成“fork 前预解析 + fork 后无锁提交”；
/// - 重复回调是幂等的，已初始化时直接返回。
pub fn install_for_hyos(package_name: &str, process_name: &str) {
    if package_name.is_empty() {
        log::warn!("hyos hook install skipped: empty package");
        return;
    }
    if !HYOS_PREPARED.load(Ordering::Acquire) {
        log::warn!("hyos pre-fork prepare missing pkg={package_name}");
    }
    let hub = InterceptHub::instance();
    if hub.is_runtime_hook_initialized() {
        log::info!(
            "hyos hook already initialized pkg={} existing={}",
            package_name,
            hub.get_package_name()
        );
        return;
    }
    if !hub.init(package_name, false, true) {
        log::warn!("hyos hook init failed pkg={package_name}");
        return;
    }
    hub.set_redirect_via_hook(true);
    let installed = hub.install();
    log::info!(
        "hyos hook install pkg={} proc={} installed={}",
        package_name,
        process_name,
        installed
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_pre_fork_is_idempotent() {
        prepare_pre_fork();
        assert!(HYOS_PREPARED.load(Ordering::Acquire));
        prepare_pre_fork();
        assert!(HYOS_PREPARED.load(Ordering::Acquire));
    }

    #[test]
    fn empty_package_is_rejected_before_hub_init() {
        // 空包名在接触 InterceptHub 之前就返回，避免把测试进程的 hook 状态
        // 初始化成无效包名。
        install_for_hyos("", "");
        assert!(!InterceptHub::instance().is_runtime_hook_initialized());
    }
}
