//! 共享 FUSE daemon 的服务监督与死连接恢复策略。
//!
//! 现有结构里每个应用各自 fork 一个 FUSE 服务进程，daemon 只能靠 `/proc` + starttime
//! 判活，服务异常退出后由周期 reconcile 兜底。本模块把"健康检查"与"恢复决策"从
//! reconcile 主循环里拆出来，给出三条明确契约：
//!
//! 1. **端点健康**：只有真正访问目标命名空间内的挂载点，才能区分"挂载记录还在"与
//!    "连接已断"。`/proc/<pid>/mountinfo` 里存在记录不代表可用——FUSE 服务退出后记录会
//!    保留，而访问返回 ENOTCONN；
//! 2. **恢复顺序**：先按身份摘除，再重新注入。摘除没有被确认清空之前不进入注入阶段，
//!    否则就是在死连接上叠加新的挂载层；
//! 3. **失败收敛**：同一命名空间连续摘除失败达到预算后停止注入并保持告警，把"一直挂不上"
//!    暴露成持续可观测状态，而不是每轮都再叠一层。
//!
//! 判定所需的归属信息来自 [`crate::mount_identity`]，本模块只负责"据此该做什么"。

use crate::mount_identity::{MountLedger, MountVerdict};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};

/// 断连类 errno：挂载记录仍在，但文件系统已经无法响应访问。
///
/// FUSE 服务退出、后端设备摘除或挂载句柄失效时内核返回的错误码不完全一致，这些都属于
/// "必须摘除后重建"的同一类情况。
fn is_dead_connection_errno(error_no: i32) -> bool {
    matches!(
        error_no,
        libc::ENOTCONN | libc::EIO | libc::ENODEV | libc::ESTALE
    )
}

/// 挂载点已经不存在：没有需要摘除的残留。
fn is_missing_errno(error_no: i32) -> bool {
    matches!(error_no, libc::ENOENT | libc::ENOTDIR)
}

/// 目标命名空间内挂载端点的健康判定结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointHealth {
    /// 挂载点可以正常打开。
    Healthy,
    /// 挂载记录还在但访问断连：必须先摘除再恢复。
    DeadConnection(i32),
    /// 挂载点不存在：没有残留需要处理。
    Missing,
    /// 探测本身未能得出结论（权限、路径非法等），不能据此判定挂载不可用。
    Unprobed(i32),
}

impl EndpointHealth {
    /// 是否属于"连接已断"。
    pub fn is_dead_connection(&self) -> bool {
        matches!(self, Self::DeadConnection(_))
    }

    /// 是否已经可以确认没有残留挂载。
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Missing)
    }

    /// 判定所依据的 errno；健康与缺失状态返回 0。
    pub fn error_no(&self) -> i32 {
        match self {
            Self::DeadConnection(error_no) | Self::Unprobed(error_no) => *error_no,
            Self::Healthy | Self::Missing => 0,
        }
    }

    /// 诊断用短名。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::DeadConnection(_) => "dead_connection",
            Self::Missing => "missing",
            Self::Unprobed(_) => "unprobed",
        }
    }
}

/// 探测目标命名空间内的挂载端点是否可用。
///
/// 通过 `/proc/<pid>/root/<target>` 解析路径会沿用目标进程的 mount namespace，不需要切换
/// daemon 自身的命名空间，也不会修改目录元数据。`pid <= 0` 表示探测 daemon 自己所在的
/// 命名空间，用于检查共享 FUSE 宿主挂载是否仍然存活。
pub fn probe_endpoint(pid: i32, target: &str) -> EndpointHealth {
    if target.is_empty() {
        return EndpointHealth::Unprobed(libc::EINVAL);
    }
    let probe_path = if pid > 0 {
        format!("/proc/{pid}/root{target}")
    } else {
        format!("/proc/self/root{target}")
    };
    let Ok(c_path) = CString::new(probe_path) else {
        return EndpointHealth::Unprobed(libc::EINVAL);
    };
    // SAFETY: c_path 由当前作用域持有，指针在 open 调用期间有效；标志只读打开目录。
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd >= 0 {
        // SAFETY: fd 来自上面的成功 open，且此处是唯一的关闭路径。
        unsafe { libc::close(fd) };
        return EndpointHealth::Healthy;
    }
    let error_no = crate::platform::errno::last();
    if is_missing_errno(error_no) {
        return EndpointHealth::Missing;
    }
    if is_dead_connection_errno(error_no) {
        return EndpointHealth::DeadConnection(error_no);
    }
    EndpointHealth::Unprobed(error_no)
}

/// 恢复动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    /// 先按身份摘除本模块的挂载层，确认清空后再注入。
    DetachThenReinject,
    /// 路径上没有残留，直接注入。
    ReinjectOnly,
    /// 同路径已被其它身份接管：不摘除，交由接管者负责。
    SkipSuperseded,
    /// 目标进程或命名空间已失效：只清理账本，不做任何挂载操作。
    DropStale,
    /// 已收敛：停止注入并保持告警，等待摘除被确认完成。
    RefusePoisoned,
}

impl RecoveryAction {
    /// 诊断用短名。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DetachThenReinject => "detach_then_reinject",
            Self::ReinjectOnly => "reinject_only",
            Self::SkipSuperseded => "skip_superseded",
            Self::DropStale => "drop_stale",
            Self::RefusePoisoned => "refuse_poisoned",
        }
    }

    /// 该动作是否允许继续执行挂载注入。
    ///
    /// [`RecoveryAction::DropStale`] 允许注入：账本记录的命名空间已经随进程或命名空间替换
    /// 而销毁，其中的挂载也一并消失，此时"没有残留"是确定的事实，重新注入不会叠加。
    /// [`RecoveryAction::SkipSuperseded`] 不允许：目标路径已被接管，再挂一层会遮蔽接管者。
    pub fn allows_inject(&self) -> bool {
        matches!(
            self,
            Self::DetachThenReinject | Self::ReinjectOnly | Self::DropStale
        )
    }
}

/// 按身份判定与端点健康规划恢复动作。
///
/// `poisoned` 由账本的摘除失败轮次决定。收敛优先级高于其它判定：即使端点此刻看起来健康，
/// 只要上一次摘除没有被确认清空，就说明同路径上可能还压着一层死挂载，此时注入只会叠得
/// 更高，因此必须先拒绝注入。
pub fn plan_recovery(
    verdict: &MountVerdict,
    health: EndpointHealth,
    poisoned: bool,
) -> RecoveryAction {
    if poisoned {
        return RecoveryAction::RefusePoisoned;
    }
    match verdict {
        MountVerdict::StaleNamespace => RecoveryAction::DropStale,
        MountVerdict::Superseded(_) => RecoveryAction::SkipSuperseded,
        MountVerdict::Detached => RecoveryAction::ReinjectOnly,
        // 归属确认是本模块的挂载层：无论端点此刻是否断连都先摘除。
        // 端点断连时摘除是修复；端点健康时摘除是避免同一路径叠加多层挂载。
        MountVerdict::Owned(_) => RecoveryAction::DetachThenReinject,
    }
    .normalize_for_health(health)
}

impl RecoveryAction {
    /// 端点已经不存在时把"先摘除"降级为"直接注入"，避免对空路径做无意义的卸载尝试。
    fn normalize_for_health(self, health: EndpointHealth) -> Self {
        if health.is_absent() && matches!(self, Self::DetachThenReinject) {
            return Self::ReinjectOnly;
        }
        self
    }
}

/// 汇总多个挂载点的恢复动作，取最保守的一个。
///
/// 一个命名空间里通常有多个挂载点（共享存储根、应用私有目录、路径映射目标），它们的判定
/// 可能不一致。只要有一个挂载点不允许继续注入，整个命名空间就不能注入：否则同一轮里
/// 一部分路径被清理、一部分继续叠加，挂载栈会变得无法解释。
pub fn aggregate_actions(actions: impl IntoIterator<Item = RecoveryAction>) -> RecoveryAction {
    let mut worst = RecoveryAction::ReinjectOnly;
    let mut seen = false;
    for action in actions {
        if !seen {
            worst = action;
            seen = true;
            continue;
        }
        if severity(action) > severity(worst) {
            worst = action;
        }
    }
    worst
}

/// 动作的保守程度排序，数值越大越优先。
fn severity(action: RecoveryAction) -> u8 {
    match action {
        RecoveryAction::ReinjectOnly => 0,
        RecoveryAction::DetachThenReinject => 1,
        RecoveryAction::SkipSuperseded => 2,
        RecoveryAction::DropStale => 3,
        RecoveryAction::RefusePoisoned => 4,
    }
}

/// 单个命名空间的监督快照。
#[derive(Clone, Debug)]
pub struct NamespaceSupervision {
    pub package_name: String,
    pub target_pid: i32,
    pub generation: u64,
    pub detach_attempts: u32,
    pub poisoned: bool,
    pub last_health: EndpointHealth,
    pub last_action: RecoveryAction,
}

impl NamespaceSupervision {
    /// 从账本与本次判定结果生成监督快照。
    pub fn from_ledger(
        ledger: &MountLedger,
        health: EndpointHealth,
        action: RecoveryAction,
    ) -> Self {
        Self {
            package_name: ledger.package_name.clone(),
            target_pid: ledger.target_pid,
            generation: ledger.generation,
            detach_attempts: ledger.detach_attempts,
            poisoned: ledger.is_poisoned(),
            last_health: health,
            last_action: action,
        }
    }
}

/// 全局监督计数，用于把恢复行为汇总进周期日志与诊断输出。
#[derive(Default)]
struct SupervisorCounters {
    detach_then_reinject: AtomicU64,
    reinject_only: AtomicU64,
    skip_superseded: AtomicU64,
    drop_stale: AtomicU64,
    refuse_poisoned: AtomicU64,
    dead_connection_probes: AtomicU64,
}

static COUNTERS: SupervisorCounters = SupervisorCounters {
    detach_then_reinject: AtomicU64::new(0),
    reinject_only: AtomicU64::new(0),
    skip_superseded: AtomicU64::new(0),
    drop_stale: AtomicU64::new(0),
    refuse_poisoned: AtomicU64::new(0),
    dead_connection_probes: AtomicU64::new(0),
};

/// 记录一次恢复判定结果。
pub fn record_action(action: RecoveryAction, health: EndpointHealth) {
    let counter = match action {
        RecoveryAction::DetachThenReinject => &COUNTERS.detach_then_reinject,
        RecoveryAction::ReinjectOnly => &COUNTERS.reinject_only,
        RecoveryAction::SkipSuperseded => &COUNTERS.skip_superseded,
        RecoveryAction::DropStale => &COUNTERS.drop_stale,
        RecoveryAction::RefusePoisoned => &COUNTERS.refuse_poisoned,
    };
    counter.fetch_add(1, Ordering::Relaxed);
    if health.is_dead_connection() {
        COUNTERS
            .dead_connection_probes
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// 汇总计数，用于周期日志。
pub struct SupervisorSummary {
    pub detach_then_reinject: u64,
    pub reinject_only: u64,
    pub skip_superseded: u64,
    pub drop_stale: u64,
    pub refuse_poisoned: u64,
    pub dead_connection_probes: u64,
}

impl SupervisorSummary {
    pub fn snapshot() -> Self {
        Self {
            detach_then_reinject: COUNTERS.detach_then_reinject.load(Ordering::Relaxed),
            reinject_only: COUNTERS.reinject_only.load(Ordering::Relaxed),
            skip_superseded: COUNTERS.skip_superseded.load(Ordering::Relaxed),
            drop_stale: COUNTERS.drop_stale.load(Ordering::Relaxed),
            refuse_poisoned: COUNTERS.refuse_poisoned.load(Ordering::Relaxed),
            dead_connection_probes: COUNTERS.dead_connection_probes.load(Ordering::Relaxed),
        }
    }

    /// 是否有任何恢复行为发生过；没有发生时周期日志不必输出。
    pub fn has_activity(&self) -> bool {
        self.detach_then_reinject > 0
            || self.reinject_only > 0
            || self.skip_superseded > 0
            || self.drop_stale > 0
            || self.refuse_poisoned > 0
    }

    /// 一行诊断文本，供 daemon 日志与 `srx_daemon doctor` 复用。
    pub fn render(&self) -> String {
        format!(
            "detach_then_reinject={} reinject_only={} skip_superseded={} drop_stale={} refuse_poisoned={} dead_connection_probes={}",
            self.detach_then_reinject,
            self.reinject_only,
            self.skip_superseded,
            self.drop_stale,
            self.refuse_poisoned,
            self.dead_connection_probes
        )
    }
}

/// 渲染单个命名空间监督状态的诊断文本。
pub fn render_namespace(snapshot: &NamespaceSupervision) -> String {
    format!(
        "pkg={} pid={} generation={} health={} action={} detach_attempts={} poisoned={}",
        snapshot.package_name,
        snapshot.target_pid,
        snapshot.generation,
        snapshot.last_health.as_str(),
        snapshot.last_action.as_str(),
        snapshot.detach_attempts,
        snapshot.poisoned
    )
}
