// 应用侧等待「挂载执行方回执」的预算。
//
// 这个等待跑在应用主线程上（zygisk specialize 之后），而 AMS 的进程启动超时约 10 秒。
// 旧实现用 220 × 50ms = 11 秒的轮询，单独就超过启动超时线；现在改为阻塞等待伴生进程的
// 4 字节回执（`poll` + 一次读满），预算 600ms：实测挂载在请求发出后 ~200ms 内落定，
// 600ms 覆盖正常波动，又远低于启动超时线，即使回执永远不来也不会把应用拖死。
pub(crate) const POST_MOUNT_ACK_WAIT_MS: i64 = 600;

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

pub(crate) fn post_mount_ack_wait_budget_ms() -> i64 {
    POST_MOUNT_ACK_WAIT_MS
}

pub(crate) fn companion_parent_recv_budget_sec(scoped_fuse_root_count: usize) -> i64 {
    companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count)
        .saturating_add(COMPANION_PARENT_RECV_GRACE_TIMEOUT_SEC)
}

pub(crate) fn companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count: usize) -> i64 {
    COMPANION_PARENT_RECV_PRIMARY_TIMEOUT_SEC
        .saturating_add(FUSE_READY_TIMEOUT_SEC.saturating_mul(scoped_fuse_root_count as i64))
}
