//! 数据面慢文件系统调用探针。
//!
//! 背景：用户设备实测的宿主全局停摆形态是全部 worker 卡在对某条挂载层的
//! `newfstatat`（dentry 重验证无限等待），但内核栈不含路径，无法定位是哪个
//! 后端/哪条挂载。本模块以 RAII 守卫包装数据面的文件系统调用：调用开始计时，
//! 结束（含错误路径）时超过阈值即把调用点、完整路径与耗时写进运行日志——
//! 下一次停摆的日志将直接给出 worker 卡住的确切目标。
//!
//! 阈值取 1 秒：f2fs 直连的常规 stat 是微秒级，1 秒已属异常；探针本身在
//! 正常路径上只多一次 `Instant::now()`（纳秒级），不改变任何调用语义。

use std::path::Path;
use std::time::{Duration, Instant};

/// 慢调用判定阈值。
const SLOW_FS_LOG_THRESHOLD: Duration = Duration::from_secs(1);

/// 慢调用守卫：创建即计时，Drop 时超阈值输出现场。
pub(crate) struct SlowFsGuard {
    label: &'static str,
    path: String,
    start: Instant,
}

impl SlowFsGuard {
    pub(crate) fn start(label: &'static str, path: impl AsRef<Path>) -> Self {
        Self {
            label,
            path: path.as_ref().to_string_lossy().into_owned(),
            start: Instant::now(),
        }
    }
}

impl Drop for SlowFsGuard {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed >= SLOW_FS_LOG_THRESHOLD {
            // error 级确保进入运行日志（宿主仅转发 warn 以上）。
            log::error!(
                "fuse slow fs op label={} path={} elapsed_ms={}",
                self.label,
                self.path,
                elapsed.as_millis()
            );
        }
    }
}
