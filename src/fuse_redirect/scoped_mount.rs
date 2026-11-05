//! scoped 挂载的结论收敛点。
//!
//! daemon 挂载路径（`daemon_mount`）与 companion 挂载路径（`lifecycle::companion_mount`）各有一份
//! 「启动 scoped 服务 → 记录能力 → 选择后端 → 打 `backend_effective` 日志」的流程。两份实现逐字
//! 重复，且已经开始漂移：真机排查时同一请求在两处给出不同的 `selection_reason`，把「能力未放行」
//! 与「规则本来就不需要 FUSE 根」混成同一句话，导致判定依据不可读。
//!
//! 这里把「结论」收敛成单一实现：调用方只负责启动服务（两侧请求类型不同，无法共用），把
//! [`ScopedMountAttempt`] 交给本模块，由本模块统一写能力快照、统一选择后端、统一拼
//! `selection_reason` 并输出日志。

use crate::config::StorageBackendMode;
use crate::fuse_redirect::config;

/// 本次 scoped 挂载走到了哪一步。
///
/// 由调用方给出，因为它才知道哪些分支被走过；本模块据此决定能力计数与后端结论，避免调用方再
/// 各自拼一套判定。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopedMountAttempt {
    /// 能力闸门未放行：设备被判不可用且退避窗口未到期，或该应用已用完自己的失败预算。
    GateBlocked,
    /// 闸门放行，但该请求的规则不需要任何 scoped 挂载根。
    NoRootsNeeded,
    /// 已尝试启动 scoped 服务且成功。
    Ready,
    /// 已尝试启动 scoped 服务但失败。
    Failed,
}

/// [`conclude_scoped_mount`] 的输入。
///
/// 打包成结构体而不是九参数函数：这些字段是一组同源事实（同一个请求的规划结果与尝试结果），
/// 拆成位置参数后调用点极易错位——尤其是两个语义相近的 `ready_reason` / `failed_reason`。
pub struct ScopedMountReport<'a> {
    pub package_name: &'a str,
    pub pid: i32,
    pub requested_mode: StorageBackendMode,
    /// 日志前缀，保留两条路径各自的既有前缀（`daemon hybrid fuse` / `hybrid fuse`）。
    pub log_prefix: &'a str,
    /// 规划出的 scoped 挂载根数量。
    pub roots_planned: usize,
    /// 实际启动的 scoped 会话数。
    pub sessions: usize,
    pub attempt: ScopedMountAttempt,
    /// 写能力快照时 `Ready` 分支使用的原因串。
    pub ready_reason: &'a str,
    /// 写能力快照时 `Failed` 分支使用的原因串。
    pub failed_reason: &'a str,
    /// FUSE 视图没建起来的具体原因（`None` 表示不适用：成功、闸门未放行或规则不需要）。
    ///
    /// 它出现在 `selection_reason` 与日志里，是现场判断「设备不支持 / 配置无规则 /
    /// 策略没登记 / 接入失败」的唯一依据。
    pub unavailable_reason: Option<&'a str>,
}

/// 汇总 scoped 挂载结果：写能力快照、选择后端、输出统一日志。
pub fn conclude_scoped_mount(report: ScopedMountReport<'_>) {
    let ScopedMountReport {
        package_name,
        pid,
        requested_mode,
        log_prefix,
        roots_planned,
        sessions,
        attempt,
        ready_reason,
        failed_reason,
        unavailable_reason,
    } = report;
    let capability = match attempt {
        ScopedMountAttempt::Ready => {
            config::record_fuse_capability_result(true, ready_reason, package_name)
        }
        ScopedMountAttempt::Failed => {
            config::record_fuse_capability_result(false, failed_reason, package_name)
        }
        // 未尝试就不产生新证据。这里刻意不写快照：把「没试」写成一次结果会让计数失去意义。
        ScopedMountAttempt::GateBlocked | ScopedMountAttempt::NoRootsNeeded => {
            config::fuse_capability()
        }
    };

    let scoped_fuse_effective = attempt == ScopedMountAttempt::Ready && sessions > 0;
    let effective_backend = if scoped_fuse_effective {
        "fuse"
    } else {
        "namespace"
    };
    let selection_reason: String = if scoped_fuse_effective {
        // 「scoped」是 per-app 会话时代沿用的稳定关键字；per-app 会话已退役（b21c613a），
        // Ready 实际表示应用视图已接入共享宿主会话。追加 view= 说明真实形态，避免现场
        // 误读成「走了独立 scoped 会话、没走宿主」。
        "scoped_mount_ready:view=srx_host".to_string()
    } else if let Some(reason) = unavailable_reason {
        // 保留既有前缀（诊断脚本按它检索），把具体原因追加在后面：日志里既有稳定
        // 关键字，也有一眼能读懂的原因。
        format!("scoped_mount_failed_namespace_fallback:{reason}")
    } else if attempt == ScopedMountAttempt::GateBlocked {
        // 保留既有字符串：CI 诊断脚本按它检索。
        "capability_unavailable_or_unknown".to_string()
    } else if attempt == ScopedMountAttempt::NoRootsNeeded {
        "rules_need_no_scoped_fuse_root".to_string()
    } else {
        "scoped_mount_failed_namespace_fallback".to_string()
    };

    if attempt == ScopedMountAttempt::Failed {
        // 这条已经不代表「scoped 服务失败」：per-app scoped 会话已退役，Failed 只说明
        // 宿主 FUSE 视图没能建立，本轮按 namespace bind 继续（具体原因在
        // `fuse view unavailable` 那行里）。
        log::warn!(
            "{} no fuse view, using namespace bind pid={} pkg={}",
            log_prefix,
            pid,
            package_name
        );
    }

    log::info!(
        "backend_effective pkg={} pid={} requested={} effective={} capability={} selection_reason={} fuse_roots={} fuse_sessions={}",
        package_name,
        pid,
        requested_mode.as_str(),
        effective_backend,
        config::fuse_capability_as_str(capability),
        selection_reason,
        roots_planned,
        sessions
    );
}

/// 打印规划出的 scoped 挂载根。
pub fn log_scoped_mount_roots(log_prefix: &str, package_name: &str, pid: i32, roots: &[String]) {
    if roots.is_empty() {
        return;
    }
    log::info!(
        "{} roots pkg={} pid={} count={}",
        log_prefix,
        package_name,
        pid,
        roots.len()
    );
    for root in roots {
        log::info!("{} root {}", log_prefix, root);
    }
}
