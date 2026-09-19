// 应用侧等待「重定向挂载是否落定」的轮询预算。
//
// 这个等待跑在应用主线程上（zygisk specialize 之后），而 AMS 的进程启动超时约 10 秒。
// 旧值 220 × 50ms = 11 秒，单独就超过了启动超时线：只要挂载判据没能立刻给出肯定答案
// （命名空间后端的 bind 挂载就属于这类），应用会在等待里被 AMS 判 start timeout 杀掉，
// 表现为所有依赖文件系统的用例超时。
//
// 实测挂载通常在请求发出后 ~200ms 内就已落定，这个等待实际只是「确认集合稳定」而非
// 「等待挂载建立」——companion 路径的父进程在此之前已经等过子进程的挂载结果。因此把预算
// 收到 600ms（20ms × 30 轮）：足够覆盖正常波动与落定确认，又远低于启动超时线，即使判据
// 完全不成立也不会把应用拖死。
pub(crate) const POST_MOUNT_STATUS_POLL_COUNT: i32 = 30;
pub(crate) const POST_MOUNT_STATUS_POLL_DELAY_US: u32 = 20 * 1000;
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

pub(crate) fn post_mount_status_wait_budget_ms() -> i64 {
    (POST_MOUNT_STATUS_POLL_COUNT as i64).saturating_mul(POST_MOUNT_STATUS_POLL_DELAY_US as i64)
        / 1000
}

pub(crate) fn companion_parent_recv_budget_sec(scoped_fuse_root_count: usize) -> i64 {
    companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count)
        .saturating_add(COMPANION_PARENT_RECV_GRACE_TIMEOUT_SEC)
}

pub(crate) fn companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count: usize) -> i64 {
    COMPANION_PARENT_RECV_PRIMARY_TIMEOUT_SEC
        .saturating_add(FUSE_READY_TIMEOUT_SEC.saturating_mul(scoped_fuse_root_count as i64))
}
