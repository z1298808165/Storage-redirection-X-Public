//! 挂载身份账本的恢复决策层（仅 daemon 二进制）。
//!
//! 账本的类型、落盘格式与身份采集在 [`crate::mount_ledger`]，由 lib 与 bin 共用；这里只保留
//! **只与恢复决策有关**的部分：把账本与实时挂载表对照得出处置结论（[`MountVerdict`]）、
//! 清理与枚举。这样拆分的理由是 companion 路径（应用进程内）也要写账本，而它只编译在 lib 里，
//! 拉不进来本文件的判定逻辑；反过来本文件的条目在 lib 里没有任何调用方，留在共享模块只会
//! 触发 dead_code（CI `-D warnings` 会失败）。
//!
//! 本模块通过 glob 再导出共享内核，因此 `mount_identity::load` 这类既有调用路径保持不变。

use crate::platform::paths;
use std::fs;

pub use crate::mount_ledger::*;

/// 同一命名空间连续摘除失败的收敛预算。
///
/// 达到预算后账本进入 poisoned 状态：恢复流程停止重新注入，只保留告警，直到摘除被确认
/// 完成。这样做的目的是把"一直挂不上"暴露成一个可观测的持续状态，而不是每轮都叠加一层
/// 新的挂载，让应用最终面对一个多层 ENOTCONN 的死挂载栈。
///
/// 放在这一层而不是共享内核里：它是**恢复决策**的阈值，lib 侧只负责写账本，不需要它。
pub const MAX_DETACH_ATTEMPTS: u32 = 3;

impl MountLedger {
    /// 累计一次摘除失败；返回是否已经达到收敛预算。
    pub fn record_detach_failure(&mut self) -> bool {
        self.detach_attempts = self.detach_attempts.saturating_add(1);
        self.is_poisoned()
    }

    /// 是否已收敛到 poisoned：继续注入只会在死连接上叠加新的挂载层。
    pub fn is_poisoned(&self) -> bool {
        self.detach_attempts >= MAX_DETACH_ATTEMPTS
    }

    /// 目标进程是否仍是账本记录的那一个实例。
    pub fn target_is_current(&self) -> bool {
        crate::platform::is_process_instance_alive(self.target_pid, self.target_start_time)
    }
}

/// 挂载归属判定结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountVerdict {
    /// 记录与实时状态一致：必须先摘除这一层，再重新注入。
    Owned(LiveMount),
    /// 目标路径上已没有挂载：无需摘除，可以直接重新注入。
    Detached,
    /// 同路径被其它身份接管（新会话或其它组件）：不得摘除，交由接管者负责。
    Superseded(LiveMount),
    /// 目标进程或命名空间已经失效：挂载随命名空间销毁，记录应清理。
    StaleNamespace,
}

/// 按挂载身份判定当前应该执行哪种恢复动作。
///
/// 判定顺序体现"先证明归属，再动手"的原则：
///
/// 1. 目标进程已不是记录的那个实例（退出或被 PID 复用）→ [`MountVerdict::StaleNamespace`]，
///    挂载随旧命名空间销毁，账本应清理，不能对复用后的 PID 摘挂载；
/// 2. 命名空间 inode 变化 → 同样视为旧命名空间已销毁；
/// 3. 账本里没有该挂载点的记录，或目标路径上没有挂载 → [`MountVerdict::Detached`]，
///    直接重新注入；
/// 4. 最顶层挂载的 ID 与挂载源都匹配记录 → [`MountVerdict::Owned`]，摘除它再重新注入；
/// 5. 其余情况（同路径上有别的挂载，或本模块的新会话已经接管）→
///    [`MountVerdict::Superseded`]，不摘除。
pub fn classify_mount(
    ledger: &MountLedger,
    mount_point: &str,
    live: Option<&LiveMount>,
    current_namespace: Option<NamespaceIdentity>,
    target_is_current: bool,
) -> MountVerdict {
    if !target_is_current {
        return MountVerdict::StaleNamespace;
    }
    let Some(current_namespace) = current_namespace else {
        return MountVerdict::StaleNamespace;
    };
    if current_namespace != ledger.namespace {
        return MountVerdict::StaleNamespace;
    }
    let normalized = paths::normalize(mount_point);
    let recorded = ledger
        .mounts
        .iter()
        .find(|mount| paths::eq_ignore_case(&mount.mount_point, &normalized));
    let (Some(recorded), Some(live)) = (recorded, live) else {
        return MountVerdict::Detached;
    };
    if live.mount_id == recorded.mount_id && live.source == recorded.source {
        return MountVerdict::Owned(live.clone());
    }
    MountVerdict::Superseded(live.clone())
}

/// 删除账本；文件已不存在也视为成功。
pub fn remove(package_name: &str, pid: i32) -> bool {
    let path = ledger_path(package_name, pid);
    fs::remove_file(&path).is_ok() || fs::metadata(&path).is_err()
}

/// 清理目标进程已经退出的账本。
///
/// 判断以 `/proc/<pid>` 的 starttime 为准：PID 被复用后 starttime 一定不同，
/// 据此可以区分"进程真的退出了"与"PID 换了个新进程"。
pub fn prune_stale() -> usize {
    let Ok(entries) = fs::read_dir(ledger_dir()) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("identity") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(ledger) = decode(&content) else {
            // 解析不了的文件既不能用于恢复也不能用于清理，直接移除避免长期累积。
            if fs::remove_file(&path).is_ok() {
                removed = removed.saturating_add(1);
            }
            continue;
        };
        if !ledger.target_is_current() && fs::remove_file(&path).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    if removed > 0 {
        log::info!("mount identity pruned stale count={}", removed);
    }
    removed
}

/// 枚举磁盘上现有的全部账本，供诊断输出使用。
///
/// 解析失败的文件不计入结果：诊断不能因为一个损坏文件而整体失败。
pub fn list_ledgers() -> Vec<MountLedger> {
    let Ok(entries) = fs::read_dir(ledger_dir()) else {
        return Vec::new();
    };
    let mut ledgers = entries
        .flatten()
        .filter(|entry| {
            entry.path().extension().and_then(|value| value.to_str()) == Some("identity")
        })
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .filter_map(|content| decode(&content))
        .collect::<Vec<_>>();
    ledgers.sort_by(|left, right| {
        left.package_name
            .cmp(&right.package_name)
            .then_with(|| left.target_pid.cmp(&right.target_pid))
    });
    ledgers
}
