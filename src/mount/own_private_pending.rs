//! 自有私有目录恢复的**重试队列**（跨进程用标记文件承载）。
//!
//! 为什么需要：`Android/{data,media,obb}/<pkg>` 的系统视图恢复只能在应用命名空间里、
//! 且必须在 MediaProvider 的 FUSE 视图**可解析**之后执行。冷启动首轮挂载经常落在
//! MP 就绪窗口内（真机 2026-11-04 duchamp 实测可达数分钟：那一刻挂载表里视图都在位，
//! root 逐级可达，但 MP 对未注册 pid 的 lookup 一律拒绝），此时恢复只能失败；而
//! reconcile 的周期轮只处理 `missing` 挂载，已挂好的应用不会再来一次——于是失败变成
//! **永久状态**，自有私有目录长期留在直连层（两层页缓存分裂 → SQLite `-shm` 跨不过去
//! → WAL 回滚空主库）。
//!
//! 这里把失败落成标记文件，由 daemon 主循环按退避把该包重新排进增量 reconcile
//! （等同一次该包的配置变更 → 重新挂载 → 恢复幂等重试）。成功后由恢复路径自己删标记。
//!
//! **就绪度感知**是关键设计：恢复失败分两类，预算分开算——
//! - 视图未就绪（`root_dir=false`，root 连存储根都解析不到）：MP 窗口内的必然失败，
//!   不消耗重试预算，只按轮次退避重试；
//! - 视图就绪后仍失败：真问题（数据面拒绝、锚点失效等），计入预算，到顶放弃留 warn。
//!
//! 用文件而不是内存表，是因为恢复执行在 daemon **fork 出的挂载子进程**里，子进程的
//! 静态表不会回传父进程；磁盘是两侧唯一共享的载体。

use crate::platform::{module_paths, paths};
use std::sync::atomic::{AtomicI32, Ordering};

static DAEMON_WAKE_EVENTFD: AtomicI32 = AtomicI32::new(-1);

/// 由 daemon 主循环注入唤醒 fd；挂载子进程 fork 后继承同一个 eventfd。
pub fn set_daemon_wake_eventfd(fd: i32) {
    DAEMON_WAKE_EVENTFD.store(fd, Ordering::Release);
}

fn wake_daemon() {
    let fd = DAEMON_WAKE_EVENTFD.load(Ordering::Acquire);
    if fd >= 0 {
        let value = 1u64;
        // SAFETY: fd 已校验为非负的 eventfd 写端，value 是栈上 u64，长度与写入请求一致；
        // 写失败（含 EAGAIN）只是漏一次唤醒，daemon 仍会按截止点醒来，故忽略返回值。
        unsafe {
            libc::write(
                fd,
                &value as *const u64 as *const libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }
    }
}

/// 视图**就绪后**仍失败的重试上限：这种失败换时间没用，20 次足够区分「偶发竞态」
/// 与「稳定故障」，到顶放弃并留一条 warn。
const MAX_READY_FAILURES: u32 = 20;
/// 含窗口期轮次的总轮数上限：窗口期轮次不烧预算，但也不能无界轮询。按下面的退避
/// （3s×3 → 15s×17 → 30s×100）展开约 55 分钟，远超任何实测 MP 就绪窗口。
const MAX_ROUNDS: u32 = 120;
/// 最近一次 bind(MS_BIND) 失败的真实 errno（0 = 无记录）。
///
/// 恢复失败的告警此前取的是 `last_create_directory_errno()`——create 早成功、bind
/// 失败时日志会打出误导性的 errno=0。bind 侧把真实值存这里，失败日志直接引用。
static LAST_BIND_ERRNO: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// bind 失败时记录真实 errno（planner 的 mount 失败分支调用）。
pub fn note_bind_errno(errno: libc::c_int) {
    LAST_BIND_ERRNO.store(errno, std::sync::atomic::Ordering::Relaxed);
}

/// 取最近一次 bind 失败的真实 errno。
pub fn last_bind_errno() -> libc::c_int {
    LAST_BIND_ERRNO.load(std::sync::atomic::Ordering::Relaxed)
}

fn pending_dir() -> String {
    paths::join(module_paths::MODULE_DIR, "tmp/own_private_pending")
}

fn marker_path(package: &str) -> Option<String> {
    if package.is_empty() || package.contains('/') || package.contains("..") || package.len() > 160
    {
        return None;
    }
    Some(paths::join(&pending_dir(), package))
}

/// 返回距离最近待办到期的毫秒数；无 marker 时返回 None。
///
/// 只在 daemon 准备阻塞时扫描小型 marker 目录，不使用周期唤醒；marker 新建/刷新则由
/// `note_failure` 写 eventfd 立刻打断当前等待。此处使用 file mtime 的 elapsed 时长，
/// 调用方再换算为单调时钟 deadline，避免混用墙钟时间戳与 CLOCK_MONOTONIC。
pub fn next_due_delay_ms() -> Option<i64> {
    let entries = std::fs::read_dir(pending_dir()).ok()?;
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            let (rounds, ready_failures) = read_marker(&path.to_string_lossy());
            if ready_failures > MAX_READY_FAILURES || rounds >= MAX_ROUNDS {
                return 0;
            }
            let age_ms = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                .map(|elapsed| elapsed.as_millis().min(i64::MAX as u128) as i64)
                .unwrap_or(0);
            backoff_ms(rounds).saturating_sub(age_ms).max(0)
        })
        .min()
}

/// 第 `rounds` 轮重试前的退避间隔：前三次 3 秒（覆盖热重载与短窗口），窗口期拉长到
/// 15 秒、30 秒，避免长窗口里高频重挂正在运行的应用。
fn backoff_ms(rounds: u32) -> i64 {
    match rounds {
        0..=3 => 3_000,
        4..=20 => 15_000,
        _ => 30_000,
    }
}

/// 读取标记内容，返回 `(rounds, ready_failures)`；缺失或损坏按 `(0, 0)` 处理。
fn read_marker(path: &str) -> (u32, u32) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut fields = text.split_whitespace();
    let rounds = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let ready = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    (rounds, ready)
}

/// 记一次失败：视图未就绪只推进轮次，视图就绪后的失败才计入放弃预算。
pub fn note_failure(package: &str, view_ready: bool) {
    let Some(path) = marker_path(package) else {
        return;
    };
    let (rounds, ready_failures) = read_marker(&path);
    let rounds = rounds + 1;
    let ready_failures = if view_ready {
        ready_failures + 1
    } else {
        ready_failures
    };
    if ready_failures > MAX_READY_FAILURES || rounds > MAX_ROUNDS {
        let _ = std::fs::remove_file(&path);
        log::warn!(
            "own private restore retry exhausted pkg={} rounds={} ready_failures={} max_ready={} max_rounds={}",
            package,
            rounds,
            ready_failures,
            MAX_READY_FAILURES,
            MAX_ROUNDS
        );
        return;
    }
    let _ = std::fs::create_dir_all(pending_dir());
    // 写入内容同时刷新 mtime：mtime 是「上次尝试时刻」，内容是两类计数的当前值。
    if std::fs::write(&path, format!("{rounds} {ready_failures}")).is_ok() {
        wake_daemon();
        log::debug!(
            "own private restore retry scheduled pkg={} round={} ready_failures={} view_ready={}",
            package,
            rounds,
            ready_failures,
            view_ready
        );
    }
}

/// 记一次成功：清掉标记，停止重试。
pub fn note_success(package: &str) {
    let Some(path) = marker_path(package) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
}

/// 取出本轮到期、需要重新挂载的包名。返回空表示无需额外 reconcile。
///
/// 每个到期项都会把轮数 +1 并刷新 mtime，因此 daemon 主循环即使用户配置没有任何
/// 变化，也会按 `backoff_ms` 的节奏把该包推进一轮挂载；两类计数任一到顶即放弃。
pub fn take_due_packages() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(pending_dir()) else {
        return Vec::new();
    };
    let mut due = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let (rounds, ready_failures) = read_marker(&path.to_string_lossy());
        // mtime 缺失/读取失败时按 0 计龄，下一次退避截止点再试。
        let age_ms = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .map(|elapsed| elapsed.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        if age_ms < backoff_ms(rounds) {
            continue;
        }
        if ready_failures > MAX_READY_FAILURES || rounds + 1 > MAX_ROUNDS {
            let _ = std::fs::remove_file(&path);
            log::warn!(
                "own private restore retry exhausted pkg={} rounds={} ready_failures={} max_ready={} max_rounds={}",
                name,
                rounds,
                ready_failures,
                MAX_READY_FAILURES,
                MAX_ROUNDS
            );
            continue;
        }
        if std::fs::write(&path, format!("{} {ready_failures}", rounds + 1)).is_err() {
            continue;
        }
        log::info!(
            "own private restore retry pkg={} round={} ready_failures={}",
            name,
            rounds + 1,
            ready_failures
        );
        due.push(name);
    }
    due
}
