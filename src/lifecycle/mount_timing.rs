// 应用主线程上**没有任何**挂载等待预算：请求发出后立刻关闭连接，就位判定是一次观测
// （不睡眠、不轮询）。历史上这里有过 220 × 50ms = 11 秒的轮询（超过 AMS 的启动超时线，
// 真机表现为 `failed to attach` + `start timeout`）与后来的 600ms 阻塞读回执（那条通道
// 受 SELinux 域迁移阻断，见 `mount_request` 模块头注释）。两版都已删除——最省电、最不
// 可能把应用拖死的等待，是不等待。

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

pub(crate) fn companion_parent_recv_budget_sec(scoped_fuse_root_count: usize) -> i64 {
    companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count)
        .saturating_add(COMPANION_PARENT_RECV_GRACE_TIMEOUT_SEC)
}

pub(crate) fn companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count: usize) -> i64 {
    COMPANION_PARENT_RECV_PRIMARY_TIMEOUT_SEC
        .saturating_add(FUSE_READY_TIMEOUT_SEC.saturating_mul(scoped_fuse_root_count as i64))
}
