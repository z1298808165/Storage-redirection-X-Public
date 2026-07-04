pub(crate) const POST_MOUNT_STATUS_POLL_COUNT: i32 = 140;
pub(crate) const POST_MOUNT_STATUS_POLL_DELAY_US: u32 = 50 * 1000;
pub(crate) const POST_SPECIALIZE_SLOW_MS: i64 = 20;

// 父进程等待挂载结果的超时。之前的 2s 在高负载/FUSE 异常场景下
// 容易直接进入 SIGKILL 流程，把 mount writer 持锁的子进程强杀，
// 进而损坏 FUSE 命名空间状态拖死 MediaProvider。
pub(crate) const COMPANION_PARENT_RECV_PRIMARY_TIMEOUT_SEC: i64 = 5;
// SIGTERM 后再给的 grace，让子进程在用户态完成清理或回报结果后退出。
pub(crate) const COMPANION_PARENT_RECV_GRACE_TIMEOUT_SEC: i64 = 1;
pub(crate) const COMPANION_PROCESS_READY_TIMEOUT_MS: i32 = 5000;
pub(crate) const COMPANION_MOUNT_SLOW_MS: i64 = 20;

pub(crate) fn post_mount_status_wait_budget_ms() -> i64 {
    (POST_MOUNT_STATUS_POLL_COUNT as i64).saturating_mul(POST_MOUNT_STATUS_POLL_DELAY_US as i64)
        / 1000
}

pub(crate) fn companion_parent_recv_budget_sec() -> i64 {
    COMPANION_PARENT_RECV_PRIMARY_TIMEOUT_SEC
        .saturating_add(COMPANION_PARENT_RECV_GRACE_TIMEOUT_SEC)
}
