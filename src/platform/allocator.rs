//! 分配器空闲页归还。
//!
//! scudo 的 primary 分配器释放小对象后不会立即把页交还内核，突发负载
//! （套件尾段的配置/挂载风暴、日常的媒体库扫描）产生的分配峰值会长期
//! 滞留在 RSS 里：真机实测 daemon 静息态从 10MB 涨至 62MB 不回落，宿主
//! fork 时又以 COW 形式在子进程 RSS 中放大观感。静息期显式 `M_PURGE`
//! 可把 primary 空闲页归还内核；`M_PURGE` 自 API 30 起由 bionic scudo
//! 实现（模块最低支持 API 31），其它分配器对该选项按 mallopt 规范安全
//! 忽略，因此可以无条件调用。

use std::sync::atomic::{AtomicI64, Ordering};

/// bionic `mallopt` 的 `M_PURGE` 选项值（见 NDK sysroot malloc.h）。
const M_PURGE: libc::c_int = -101;

/// 静息期归还的节流间隔。突发期间的频繁归还只会换来反复缺页；把归还
/// 摊到 5 分钟级，让峰值滞留最多一个节流窗口即被回收。
pub const QUIESCENT_PURGE_INTERVAL_MS: i64 = 300_000;

// SAFETY: mallopt 是 bionic libc 导出的稳定入口，只调整分配器内部行为，
// 不接触调用方内存。
unsafe extern "C" {
    fn mallopt(cmd: libc::c_int, value: libc::c_int) -> libc::c_int;
}

/// 把分配器已释放但仍滞留的页归还内核。
///
/// 只应在静息期调用（批次排空、周期兜底轮），避免突发负载期间的反复
/// 缺页；返回值忽略——失败或选项不支持时等效无操作。
pub fn release_free_pages() {
    // SAFETY: M_PURGE 仅触发分配器内部空闲页回收，无内存安全影响。
    let _ = unsafe { mallopt(M_PURGE, 0) };
}

/// 按 [`QUIESCENT_PURGE_INTERVAL_MS`] 节流执行 [`release_free_pages`]。
///
/// 各常驻进程（daemon 主循环、共享宿主服务循环）在自己的空闲心跳里调用，
/// 单一进程内节流由本函数保证。
// quality-allow(lint-suppression): 调用方是 daemon.rs 的周期兜底入口，该文件仅由
// bin 目标编译，lib 目标不可见。
#[allow(dead_code)]
pub fn quiescent_purge_throttled() {
    static LAST_PURGE_MS: AtomicI64 = AtomicI64::new(0);
    let now_ms = crate::platform::paths::monotonic_ms();
    if now_ms.saturating_sub(LAST_PURGE_MS.load(Ordering::Relaxed)) < QUIESCENT_PURGE_INTERVAL_MS {
        return;
    }
    LAST_PURGE_MS.store(now_ms, Ordering::Relaxed);
    release_free_pages();
}
