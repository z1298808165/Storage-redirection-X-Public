// Zygisk 应用主线程只观测挂载就绪，不阻塞等待；Hyos 同步请求使用 companion 给出的预算。

pub(crate) const POST_SPECIALIZE_SLOW_MS: i64 = 20;

// 父进程等待挂载结果的超时。之前的 2s 在高负载/FUSE 异常场景下
// 容易直接进入 SIGKILL 流程，把 mount writer 持锁的子进程强杀，
// 进而损坏 FUSE 命名空间状态拖死 MediaProvider。
pub(crate) const COMPANION_PARENT_RECV_PRIMARY_TIMEOUT_SEC: i64 = 5;
// SIGTERM 后再给的 grace，让子进程在用户态完成清理或回报结果后退出。
pub(crate) const COMPANION_PARENT_RECV_GRACE_TIMEOUT_SEC: i64 = 1;
pub(crate) const COMPANION_PROCESS_READY_TIMEOUT_MS: i32 = 5000;
pub(crate) const COMPANION_MOUNT_SLOW_MS: i64 = 20;
pub(crate) const FUSE_READY_TIMEOUT_SEC: i64 = 4;
pub(crate) const BARRIER_WAIT_TIMEOUT_MS: i64 = 2_000;

/// Hyos 的同步等待覆盖就绪、宿主登记、两轮屏障与挂载子进程预算，另留两秒收尾。
pub(crate) fn hyos_mount_timeout_ms(roots: usize, host_wait_ms: u64) -> Option<i32> {
    let roots = i64::try_from(roots).ok()?;
    let barriers = roots.checked_mul(BARRIER_WAIT_TIMEOUT_MS)?.checked_mul(2)?;
    let total = i64::from(COMPANION_PROCESS_READY_TIMEOUT_MS)
        .checked_add(i64::try_from(host_wait_ms).ok()?)?
        .checked_add(companion_parent_recv_budget_sec(roots as usize).checked_mul(1000)?)?
        .checked_add(barriers)?
        .checked_add(2_000)?;
    i32::try_from(total).ok()
}

pub(crate) fn companion_parent_recv_budget_sec(scoped_fuse_root_count: usize) -> i64 {
    companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count)
        .saturating_add(COMPANION_PARENT_RECV_GRACE_TIMEOUT_SEC)
}

pub(crate) fn companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count: usize) -> i64 {
    COMPANION_PARENT_RECV_PRIMARY_TIMEOUT_SEC
        .saturating_add(FUSE_READY_TIMEOUT_SEC.saturating_mul(scoped_fuse_root_count as i64))
}

#[cfg(test)]
#[path = "../../tests/native-unit/mount_timing.rs"]
mod tests;
