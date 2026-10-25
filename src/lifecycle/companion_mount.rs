mod diagnostics;
mod mount_state;
mod stats;
mod sys;

use super::companion_request::CompanionMountRequest;
use super::mount_timing;
use crate::config::SettingsHub;
use crate::fuse_redirect::{
    FuseRedirectConfig, ScopedMountAttempt, ScopedMountReport, conclude_scoped_mount,
    log_scoped_mount_roots, mount_blocking_with_ready,
};
use crate::fuse_session::{
    FuseMountState, decode_wait_status, recv_result, rollback_scoped_fuse_services,
    send_mount_result, set_recv_timeout,
};
use crate::mount::MountPlanner;
use crate::platform::unique_fd::UniqueFd;
use crate::platform::{self, paths::monotonic_ms};
use diagnostics::log_child_diagnostics;
use libc::{
    AF_UNIX, CLONE_NEWNS, MS_NODEV, MS_NOSUID, MS_PRIVATE, MS_RDONLY, MS_REC, O_CLOEXEC, O_RDONLY,
    SIGKILL, SIGTERM, SOCK_DGRAM, WNOHANG, c_int, c_ulong, c_void, close, kill, mount, open, read,
    readlink, setns, socketpair, waitpid,
};
use stats::update_redirect_stats;
use std::ffi::{CStr, CString};
use sys::{c_str, errno_text, last_errno};

// 等待目标进程就绪后在子进程中执行挂载
pub fn execute_companion_mount_request(request: &CompanionMountRequest) -> bool {
    let started_ms = monotonic_ms();
    if !is_redirect_enabled_for_request(request) {
        crate::mount_intent::mark_state(
            &request.package_name,
            request.pid,
            request.uid,
            request.storage_backend_mode,
            request.config_version,
            "disabled",
        );
        log::warn!(
            "companion mount denied redirect disabled pkg={} uid={} pid={}",
            request.package_name,
            request.uid,
            request.pid
        );
        log_companion_mount_perf(request, false, 0, 0, started_ms);
        return false;
    }
    let wait_started_ms = monotonic_ms();
    let user_id = platform::user_id_from_uid(request.uid);
    let is_ready = wait_for_process_ready(
        request.pid,
        user_id,
        mount_timing::COMPANION_PROCESS_READY_TIMEOUT_MS,
    );
    let wait_ms = monotonic_ms().saturating_sub(wait_started_ms);
    if !is_ready {
        log::warn!("wait proc not ready pid={}", request.pid);
    }
    // Auto 应用的宿主策略登记只能由 daemon 执行；发一条 register-policy 控制命令
    // 让 daemon 下一轮主循环（≤1s）定点预登记，替代最坏 3s+ 的周期 reconcile 等待。
    if matches!(
        request.storage_backend_mode,
        crate::config::StorageBackendMode::Auto
    ) && crate::logging::send_control_command(&format!(
        "register-policy:{}:{}",
        request.package_name, request.pid
    )) {
        log::debug!(
            "companion register-policy sent pkg={} pid={}",
            request.package_name,
            request.pid
        );
    }
    // 先落 fail-closed 屏障再进入宿主握手/挂载：应用主线程的等待预算只有 600ms
    // （AMS 启动超时约束），宿主握手最坏需要数秒，期间应用会看到未套规则的
    // 原生全景（沙箱/隐藏规则的可见窗口）。屏障把这段窗口内的视图变成只读空
    // 目录，FUSE 落地后压在屏障之上；挂载链路彻底失败时屏障常驻（fail-closed）。
    let barrier_roots = crate::fuse_redirect::scoped_fuse_mount_roots_for_request(request);
    if !barrier_roots.is_empty() {
        mount_storage_view_barriers(request, &barrier_roots);
    }
    let mount_started_ms = monotonic_ms();
    crate::mount_intent::mark_state(
        &request.package_name,
        request.pid,
        request.uid,
        request.storage_backend_mode,
        request.config_version,
        "applying",
    );
    let is_success = run_mount_in_forked_child(request);
    let mount_ms = monotonic_ms().saturating_sub(mount_started_ms);
    crate::mount_intent::mark_state(
        &request.package_name,
        request.pid,
        request.uid,
        request.storage_backend_mode,
        request.config_version,
        if is_success { "mounted" } else { "failed" },
    );
    log_companion_mount_perf(request, is_success, wait_ms, mount_ms, started_ms);
    is_success
}

fn is_redirect_enabled_for_request(request: &CompanionMountRequest) -> bool {
    let config = SettingsHub::instance();
    if !config.init(None) {
        log::warn!(
            "companion mount config init failed pkg={} uid={} pid={}",
            request.package_name,
            request.uid,
            request.pid
        );
        return false;
    }
    config.reload_if_changed();
    config.should_redirect(&request.package_name, request.uid)
        || (request.is_monitor_only
            && crate::config::should_capture_unconfigured_app(
                &request.package_name,
                request.uid,
                config.is_file_monitor_enabled(),
            ))
}

// 应用侧裸视图窗口的 fail-closed 屏障（详见 execute_companion_mount_request 调用点注释）。
// tmpfs 会拒绝未知挂载选项（EINVAL），标记通过可识别的 size 值体现：屏障固定 4m。
const STORAGE_VIEW_BARRIER_DATA: &[u8] = b"size=4m\0";

// 为每个 FUSE 挂载根落一个只读空 tmpfs 屏障。子进程 setns 进入应用命名空间后
// 先整树 MS_PRIVATE，再在根上压一层只读 tmpfs：应用在此期间只能看到空目录，
// 写入得到 EROFS 明确错误。随后的 FUSE 挂载压在屏障之上；既有卸载路径
// （clear_mount_target_stack）逐层摘栈时会把屏障一并清理。
//
// 注意共享宿主坍缩形态（scoped 根收敛成单一视图根、FUSE 只压在视图根上）：此时
// 屏障与 FUSE 是不同挂载点的兄弟挂载，屏障不会被自然压住，成功路径必须在 FUSE
// 就位后显式回收——见 detach_barriers_shadowed_by_view_fuse。
fn mount_storage_view_barriers(request: &CompanionMountRequest, roots: &[String]) {
    for root in roots {
        if mount_one_storage_view_barrier(request, root) {
            log::info!(
                "storage view barrier mounted root={} pid={} pkg={}",
                root,
                request.pid,
                request.package_name
            );
        }
    }
}

fn mount_one_storage_view_barrier(request: &CompanionMountRequest, root: &str) -> bool {
    let Ok(c_ns) = CString::new(format!("/proc/{}/ns/mnt", request.pid)) else {
        return false;
    };
    let Ok(c_root) = CString::new(root) else {
        return false;
    };
    // 先在父进程走完私有日志通道初始化，避免子进程继承初始化中的 OnceLock。
    crate::logging::prepare_for_fork();
    // SAFETY: fork 系统调用；子进程只做 setns/mount/_exit，日志通道已完成初始化。
    let child = unsafe { libc::fork() };
    if child < 0 {
        log::warn!(
            "storage view barrier fork failed root={} pkg={} errno={}",
            root,
            request.package_name,
            last_errno()
        );
        return false;
    }
    if child == 0 {
        // SAFETY: c_ns/c_root 均为 NUL 结尾的合法路径，生命周期覆盖本次调用；
        // 子进程随后仅执行 setns/mount/_exit，不再触碰需要安全保证的数据结构。
        unsafe {
            let entered = set_mount_namespace(request.pid, Some(&c_ns));
            if !entered {
                libc::_exit(1);
            }
            let private_ok = mount(
                std::ptr::null(),
                c"/".as_ptr(),
                std::ptr::null(),
                (MS_REC | MS_PRIVATE) as c_ulong,
                std::ptr::null(),
            ) == 0;
            if !private_ok {
                log::warn!(
                    "storage view barrier ms-private failed root={} pkg={} errno={}",
                    root,
                    request.package_name,
                    last_errno()
                );
                libc::_exit(1);
            }
            // 死宿主层清理：daemon 重启/宿主换代后，应用命名空间可能残留指向已消亡
            // 宿主进程的 srx FUSE 层（宿主 pid 已不存在）。死层盖在 scoped roots 的
            // 祖先路径上，路径解析穿过死连接会让屏障挂载报 ENOTCONN（真机 2026-10-13
            // 实测 errno=107），也让后续 FUSE 接入失效。挂屏障前把宿主已消亡的 srx 层
            // 逐个 MNT_DETACH；宿主仍存活的层不动，交由既有摘栈路径处理。
            if let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") {
                for line in mounts.lines().rev() {
                    let mut fields = line.split_whitespace();
                    let source = fields.next().unwrap_or("");
                    let mount_point = fields.next().unwrap_or("").replace("\\040", " ");
                    let Some(host_pid) = source
                        .strip_prefix("srx_fuse_host[")
                        .and_then(|rest| rest.strip_suffix(']'))
                        .and_then(|pid| pid.parse::<i32>().ok())
                    else {
                        continue;
                    };
                    if std::path::Path::new(&format!("/proc/{host_pid}")).exists() {
                        // 宿主仍存活：可能是本代健康会话，交给既有摘栈路径。
                        continue;
                    }
                    let Ok(c_mount_point) = CString::new(mount_point) else {
                        continue;
                    };
                    // SAFETY: c_mount_point 是 NUL 结尾的合法路径；MNT_DETACH 延迟卸载，
                    // 即使仍有引用也不阻塞后续流程。
                    let detached = libc::umount2(c_mount_point.as_ptr(), libc::MNT_DETACH) == 0;
                    log::warn!(
                        "storage view dead layer detached mount_point={} host_pid={} ok={} pkg={}",
                        c_mount_point.to_string_lossy(),
                        host_pid,
                        detached,
                        request.package_name
                    );
                }
            }
            // 残留屏障清理：挂载链路失败时屏障按设计常驻（fail-closed），但下一次挂载
            // 若读到的规则不同（如快照竞态下全零规则走 namespace/bind），残留屏障会把
            // 视图根盖死（空只读 tmpfs → 写入 ENOENT/EROFS，应用重启也无效——同一个
            // namespace）。挂新屏障前把同根上的旧屏障（tmpfs + size=4m 标记）逐层卸掉；
            // 栈顶若已是 srx FUSE 说明有健康会话在服务，跳过清理交由既有摘栈路径。
            if let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") {
                let root_norm = root.trim_end_matches('/');
                for line in mounts.lines().rev() {
                    let mut fields = line.split_whitespace();
                    let source = fields.next().unwrap_or("");
                    let mount_point = fields
                        .next()
                        .unwrap_or("")
                        .replace("\\040", " ")
                        .trim_end_matches('/')
                        .to_string();
                    if mount_point != root_norm {
                        continue;
                    }
                    let fstype = fields.next().unwrap_or("");
                    if source == "none" && fstype == "tmpfs" {
                        let Ok(c_root) = CString::new(root_norm) else {
                            break;
                        };
                        // SAFETY: c_root 是 NUL 结尾的合法路径；MNT_DETACH 延迟卸载，
                        // 即使屏障仍被引用也不阻塞挂载流程。
                        let detached = libc::umount2(c_root.as_ptr(), libc::MNT_DETACH) == 0;
                        log::warn!("stale view barrier detached root={} ok={}", root, detached);
                    } else {
                        break;
                    }
                }
            }
            let mounted = mount(
                std::ptr::null(),
                c_root.as_ptr(),
                c"tmpfs".as_ptr(),
                (MS_RDONLY | MS_NOSUID | MS_NODEV) as c_ulong,
                STORAGE_VIEW_BARRIER_DATA.as_ptr() as *const c_void,
            ) == 0;
            if !mounted {
                log::warn!(
                    "storage view barrier mount failed root={} pkg={} errno={}",
                    root,
                    request.package_name,
                    last_errno()
                );
                libc::_exit(1);
            }
            libc::_exit(0);
        }
    }
    let mut status: c_int = 0;
    // SAFETY: status 是有效的栈上整型指针，waitpid 阻塞等待本次 fork 的子进程。
    let waited = unsafe { waitpid(child, &mut status, 0) };
    waited == child && libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
}

// 该应用命名空间的存储视图根是否已被任意 srx FUSE 层覆盖（宿主或 scoped）。
// companion 的宿主接入失败回退 scoped 之前先查一次：daemon reconcile 可能已经
// 抢先完成 attach，此时再叠一层 scoped 会造成双重挂载与视图基准混乱。
fn app_storage_view_already_served(mount_root: &str) -> bool {
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else {
        return false;
    };
    let root = mount_root.trim_end_matches('/');
    // 栈顶判定：同一挂载点在 /proc/self/mounts 里的**最后一条**条目决定当前可见层。
    // 旧实现用 rev().any()——被残留只读 tmpfs 屏障盖住的 FUSE 也会命中，把「屏障
    // 裸奔」误判成会话存活，应用写入 ENOENT/EROFS 而链路自认为健康。
    let last = mounts.lines().rev().find(|line| {
        let mut fields = line.split_whitespace();
        fields.next().unwrap_or("");
        let mount_point = fields
            .next()
            .unwrap_or("")
            .replace("\\040", " ")
            .trim_end_matches('/')
            .to_string();
        mount_point == root
    });
    match last {
        Some(line) => {
            let source = line.split_whitespace().next().unwrap_or("");
            source.starts_with("srx_fuse")
        }
        None => false,
    }
}

// ===== 遍历感知的存储路径覆盖判定 =====
//
// /proc/self/mounts 是扁平列表，表达不了挂载树的父子可达性：共享宿主坍缩后，execute
// 层落在坍缩前目录根上的屏障（父挂载 = 存储视图上一层）与视图根上的 srx FUSE 是
// **兄弟挂载**，对子路径的查找会先进入视图根的 FUSE、不再回到父挂载——屏障整只被
// 遮蔽且按路径不可达（umount2 返回 EINVAL/ENOENT），对应用完全无害。扁平匹配会把
// 这种「被遮蔽的屏障」误判成栈顶裸奔屏障（真机 2026-10-15 误报
// `barrier exposed without fuse cover`，把健康挂载报成失败并污染全局失败预算）。
// 正确判定必须按 /proc/self/mountinfo（含父挂载 ID）从命名空间根模拟内核路径遍历：
//   1) 沿路径前缀逐段前进；若某前缀上存在「父挂载 = 当前所在挂载」的条目，则进入
//      其中文件序最后的一条（同挂载点栈顶）；
//   2) 目标路径的最终服务挂载 = 全路径自身有遍历可达条目则取栈顶，否则为遍历停止
//      时所在的挂载。
// 服务挂载是 srx FUSE 才算覆盖；被遮蔽/被埋住的屏障不构成「未覆盖」。

struct MountInfoEntry {
    id: u64,
    parent: u64,
    mount_point: String,
    fstype: String,
    source: String,
}

// mountinfo 行格式：`id parent major:minor root mountpoint options... - fstype source super...`
fn parse_mountinfo_line(line: &str) -> Option<MountInfoEntry> {
    let (head, tail) = line.split_once(" - ")?;
    let mut head_fields = head.split_whitespace();
    let id = head_fields.next()?.parse::<u64>().ok()?;
    let parent = head_fields.next()?.parse::<u64>().ok()?;
    head_fields.next()?; // major:minor
    head_fields.next()?; // root within filesystem
    let mount_point = head_fields
        .next()?
        .replace("\\040", " ")
        .trim_end_matches('/')
        .to_string();
    let mut tail_fields = tail.split_whitespace();
    let fstype = tail_fields.next().unwrap_or("").to_string();
    let source = tail_fields.next().unwrap_or("").to_string();
    Some(MountInfoEntry {
        id,
        parent,
        mount_point,
        fstype,
        source,
    })
}

fn is_srx_fuse_source(source: &str) -> bool {
    source.starts_with("srx_fuse")
}

// 屏障形态识别：fail-closed 只读空 tmpfs（source=none + fstype=tmpfs，同既有清扫逻辑）。
fn is_barrier_entry(entry: &MountInfoEntry) -> bool {
    entry.fstype == "tmpfs" && (entry.source == "none" || entry.source.is_empty())
}

// 从 current 出发取 exact_path 上的可达栈顶，含 over-mount 链吸收。
//
// mount(2) 在既有挂载点上再挂一层时，新层的 parent 就是下面那层的 ID（companion
// 先挂屏障、随后 scoped FUSE 压在屏障之上即此形态）。只看 parent == current 的
// 扁平层会把链顶（真正服务的层）漏掉，导致健康会话被误判 not stable / 屏障裸奔。
// 链吸收：从直接压在 current 上的层出发，反复把 parent 指向链内成员的条目并入；
// 栈顶取文件序最后一条（与 /proc/mounts 的既有约定一致）。
fn topmost_reachable_at<'a>(
    entries: &'a [MountInfoEntry],
    current_id: u64,
    exact_path: &str,
) -> Option<&'a MountInfoEntry> {
    let mut stack: Vec<&MountInfoEntry> = entries
        .iter()
        .filter(|entry| entry.parent == current_id && entry.mount_point == exact_path)
        .collect();
    loop {
        let stack_ids: std::collections::HashSet<u64> =
            stack.iter().map(|entry| entry.id).collect();
        let mut grew = false;
        for entry in entries {
            if entry.mount_point != exact_path || stack_ids.contains(&entry.id) {
                continue;
            }
            if stack_ids.contains(&entry.parent) {
                stack.push(entry);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    stack.last().copied()
}

// 遍历模拟的结果。
struct TraversalOutcome<'a> {
    // 目标路径最终落在哪个挂载里（None = 无法定位到根挂载，判未覆盖）。
    serving: Option<&'a MountInfoEntry>,
    // 目标路径自身栈顶（存在即说明路径自身有遍历可达的挂载条目）及其下面一层。
    top_at_path: Option<&'a MountInfoEntry>,
    beneath_top_at_path: Option<&'a MountInfoEntry>,
}

fn traverse_mountinfo<'a>(entries: &'a [MountInfoEntry], path: &str) -> TraversalOutcome<'a> {
    let normalized = path.trim_end_matches('/');
    // 根挂载：mountinfo 里 "/" 归一后为空串。
    let mut current = entries
        .iter()
        .find(|entry| entry.mount_point.is_empty())
        .map(|entry| entry.id);
    // 逐段前缀前进（不含全路径自身）。
    for (index, byte) in normalized.bytes().enumerate() {
        if index == 0 || byte != b'/' {
            continue;
        }
        let prefix = &normalized[..index];
        if let Some(current_id) = current
            && let Some(next) = topmost_reachable_at(entries, current_id, prefix)
        {
            current = Some(next.id);
        }
    }
    let (top_at_path, beneath_top_at_path, serving) = match current {
        Some(current_id) => {
            // 同挂载点条目可达集合：直接压在 current 上的扁平层，加 over-mount 链吸收。
            let mut stack: Vec<&MountInfoEntry> = entries
                .iter()
                .filter(|entry| entry.parent == current_id && entry.mount_point == normalized)
                .collect();
            loop {
                let stack_ids: std::collections::HashSet<u64> =
                    stack.iter().map(|entry| entry.id).collect();
                let mut grew = false;
                for entry in entries {
                    if entry.mount_point != normalized || stack_ids.contains(&entry.id) {
                        continue;
                    }
                    if stack_ids.contains(&entry.parent) {
                        stack.push(entry);
                        grew = true;
                    }
                }
                if !grew {
                    break;
                }
            }
            if stack.is_empty() {
                let serving = entries.iter().find(|entry| Some(entry.id) == current);
                (None, None, serving)
            } else {
                let top = stack[stack.len() - 1];
                // 链形态下「下面一层」= 链内父成员；扁平叠放退化为文件序前一条。
                let beneath = stack
                    .iter()
                    .rev()
                    .find(|entry| entry.id == top.parent && entry.id != top.id)
                    .copied()
                    .or_else(|| {
                        if stack.len() >= 2 {
                            Some(stack[stack.len() - 2])
                        } else {
                            None
                        }
                    });
                (Some(top), beneath, Some(top))
            }
        }
        None => (None, None, None),
    };
    TraversalOutcome {
        serving,
        top_at_path,
        beneath_top_at_path,
    }
}

// [`is_storage_path_served_by_srx_fuse_traversal`] 的纯逻辑核心，mountinfo 为
// /proc/self/mountinfo 全文，便于无挂载环境单测。
fn is_storage_path_served_by_srx_fuse_mountinfo_in(mountinfo: &str, path: &str) -> bool {
    let entries: Vec<MountInfoEntry> = mountinfo.lines().filter_map(parse_mountinfo_line).collect();
    let outcome = traverse_mountinfo(&entries, path);
    outcome
        .serving
        .map(|entry| is_srx_fuse_source(&entry.source))
        .unwrap_or(false)
}

fn is_storage_path_served_by_srx_fuse_traversal(path: &str) -> bool {
    match std::fs::read_to_string("/proc/self/mountinfo") {
        Ok(mountinfo) => is_storage_path_served_by_srx_fuse_mountinfo_in(&mountinfo, path),
        Err(_) => false,
    }
}

// 屏障处置决策（纯逻辑，便于单测）。
#[derive(Debug, PartialEq, Eq)]
enum BarrierDispose {
    // 服务挂载已是 srx FUSE：屏障被遮蔽或被埋住，不可达且无害，原样保留。
    Inert,
    // 屏障在遍历栈顶、其下同挂载点直接压着 srx FUSE：拆掉屏障恢复服务。
    Detach,
    // 其余形态保持 fail-closed 原样不动，交给裸奔检测判败。
    KeepFailClosed,
}

fn barrier_dispose_decision(outcome: &TraversalOutcome<'_>) -> BarrierDispose {
    if outcome
        .serving
        .map(|entry| is_srx_fuse_source(&entry.source))
        .unwrap_or(false)
    {
        return BarrierDispose::Inert;
    }
    if let (Some(top), Some(beneath)) = (outcome.top_at_path, outcome.beneath_top_at_path)
        && is_barrier_entry(top)
        && is_srx_fuse_source(&beneath.source)
    {
        return BarrierDispose::Detach;
    }
    BarrierDispose::KeepFailClosed
}

// 共享宿主坍缩后的屏障处置。
//
// 处置按遍历感知判定（见 is_storage_path_served_by_srx_fuse_traversal 模块注释）：
// 服务挂载已是 srx FUSE 的屏障（坍缩兄弟形态/被埋形态）不可达且无害，原样保留；
// 只有「屏障压在同挂载点 srx FUSE 之上」的残留形态才 MNT_DETACH 恢复服务；其余
// 保持 fail-closed 交给裸奔检测判败。
fn dispose_barriers_after_view_mount(barrier_roots: &[String], package_name: &str, pid: i32) {
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        log::warn!(
            "mountinfo read failed for barrier dispose pkg={}",
            package_name
        );
        return;
    };
    let entries: Vec<MountInfoEntry> = mountinfo.lines().filter_map(parse_mountinfo_line).collect();
    for root in barrier_roots {
        let normalized = root.trim_end_matches('/');
        if normalized.is_empty() {
            continue;
        }
        let outcome = traverse_mountinfo(&entries, normalized);
        match barrier_dispose_decision(&outcome) {
            BarrierDispose::Inert => {
                log::info!(
                    "collapsed view barrier inert root={} pkg={} pid={}",
                    normalized,
                    package_name,
                    pid
                );
            }
            BarrierDispose::Detach => {
                let Ok(c_root) = CString::new(normalized) else {
                    continue;
                };
                // SAFETY: c_root 是 NUL 结尾的合法路径；umount2 只解除挂载，不涉及借用指针。
                let detached = unsafe { libc::umount2(c_root.as_ptr(), libc::MNT_DETACH) } == 0;
                if detached {
                    log::info!(
                        "stale view barrier detached root={} pkg={} pid={}",
                        normalized,
                        package_name,
                        pid
                    );
                } else {
                    let errno = last_errno();
                    log::warn!(
                        "view barrier detach failed root={} pkg={} pid={} errno={} {}",
                        normalized,
                        package_name,
                        pid,
                        errno,
                        errno_text(errno)
                    );
                }
            }
            BarrierDispose::KeepFailClosed => {
                log::info!(
                    "collapsed view barrier kept fail-closed root={} pkg={} pid={}",
                    normalized,
                    package_name,
                    pid
                );
            }
        }
    }
}

// 卸载应用 namespace 里所有不在本次 roots 白名单内的残留屏障。
//
// 屏障（tmpfs size=4m 标记）在挂载链路失败时按 fail-closed 常驻；后续挂载若读到的
// 规则不同（全零快照走 namespace/bind），不会经过「挂新屏障前清理」的路径，残留屏障
// 就把对应子树盖成空只读目录（写入 ENOENT/EROFS），应用重启无效——同一个 namespace。
// 在 setns 之后、挂载动作之前统一清扫：本次 roots 之内的屏障留给挂载流程处理
// （压上 FUSE 或被新屏障替换），之外的按路径深度倒序逐个 MNT_DETACH（先卸深层，
// 避免父路径卸载后子路径不可达）。
fn detach_stale_barriers_not_in(scoped_fuse_roots: &[String]) {
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else {
        return;
    };
    let whitelisted = |mount_point: &str| -> bool {
        scoped_fuse_roots
            .iter()
            .any(|root| root.trim_end_matches('/') == mount_point)
    };
    let stale: Vec<String> = mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let source = fields.next().unwrap_or("");
            let mount_point = fields
                .next()
                .unwrap_or("")
                .replace("\\040", " ")
                .trim_end_matches('/')
                .to_string();
            let fstype = fields.next().unwrap_or("");
            (source == "none" && fstype == "tmpfs").then_some(mount_point)
        })
        .filter(|mount_point| !whitelisted(mount_point))
        .collect();
    if stale.is_empty() {
        return;
    }
    // 深度倒序：先卸 /a/b 再卸 /a，避免父路径先卸载后子路径解析不可达。
    let mut ordered = stale;
    ordered.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));
    for mount_point in ordered {
        let Ok(c_root) = CString::new(mount_point.as_str()) else {
            continue;
        };
        // SAFETY: c_root 是 NUL 结尾的合法路径；MNT_DETACH 延迟卸载不阻塞。
        let detached = unsafe { libc::umount2(c_root.as_ptr(), libc::MNT_DETACH) } == 0;
        log::warn!(
            "stale view barrier detached root={} ok={}",
            mount_point,
            detached
        );
    }
}

fn log_companion_mount_perf(
    request: &CompanionMountRequest,
    is_success: bool,
    wait_ms: i64,
    mount_ms: i64,
    started_ms: i64,
) {
    let total_ms = monotonic_ms().saturating_sub(started_ms);
    if total_ms < mount_timing::COMPANION_MOUNT_SLOW_MS && is_success {
        return;
    }
    log::info!(
        "perf companion mount pkg={} pid={} uid={} ok={} allow={} ro={} map={} map_only={} wait_ms={} mount_ms={} total_ms={}",
        request.package_name,
        request.pid,
        request.uid,
        is_success,
        request.allowed_real_paths.len(),
        request.read_only_paths.len(),
        request.path_mappings.len(),
        request.is_mapping_mode_only,
        wait_ms,
        mount_ms,
        total_ms
    );
}

// 切换到目标进程的挂载命名空间
fn set_mount_namespace(pid: i32, ns_path: Option<&CStr>) -> bool {
    // 路径在 fork 之前就已经转换好，这里只做 open/setns，避免子进程再次堆分配。
    let Some(c_path) = ns_path else {
        log::error!("ns path invalid pid={}", pid);
        return false;
    };
    let fd = unsafe { open(c_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 {
        let errno = last_errno();
        log::error!(
            "ns open failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    }
    let file = UniqueFd::new(fd);

    if unsafe { setns(file.get(), CLONE_NEWNS) } != 0 {
        let errno = last_errno();
        log::error!(
            "setns failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    }

    log::info!("entered ns pid={}", pid);
    let mut buf = [0u8; 256];
    let Some(self_ns_path) = c_str("/proc/self/ns/mnt") else {
        log::warn!("ns readlink path failed");
        return true;
    };
    let len = unsafe {
        readlink(
            self_ns_path.as_ptr(),
            buf.as_mut_ptr() as *mut _,
            buf.len() - 1,
        )
    };
    if len > 0 {
        buf[len as usize] = 0;
        let text = String::from_utf8_lossy(&buf[..len as usize]);
        log::info!("ns now={}", text);
    } else {
        let errno = last_errno();
        log::warn!(
            "ns read failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
    }
    true
}

fn wait_for_process_ready(pid: i32, user_id: i32, timeout_ms: i32) -> bool {
    let poll_interval_us = 5 * 1000;
    let timeout_us = timeout_ms * 1000;
    let mut elapsed_us = 0;

    while elapsed_us < timeout_us {
        if is_process_context_ready(pid) {
            // storage_mount 只用于调试日志，未开启调试日志时不做整份 mountinfo 的读取与扫描。
            if crate::logging::is_debug_logging_enabled() {
                let is_storage_mount_ready = is_process_storage_mount_ready(pid, user_id);
                log::debug!(
                    "proc ready pid={} wait_us={} storage_mount={}",
                    pid,
                    elapsed_us,
                    is_storage_mount_ready
                );
            }
            return true;
        }

        unsafe { libc::usleep(poll_interval_us as u32) };
        elapsed_us += poll_interval_us;
    }

    log::warn!(
        "proc ready timeout pid={} ms={} ctx=false storage_mount=false",
        pid,
        timeout_ms
    );
    false
}

// 轮询目标进程 SELinux 上下文，等待脱离 zygote 状态
fn is_process_context_ready(pid: i32) -> bool {
    let attr_path = format!("/proc/{}/attr/current", pid);

    let Ok(c_path) = std::ffi::CString::new(attr_path.clone()) else {
        log::warn!("attr path invalid pid={}", pid);
        return false;
    };

    let fd = unsafe { open(c_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 {
        let errno = last_errno();
        log::warn!(
            "attr open failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    }
    let file = UniqueFd::new(fd);
    let mut buf = [0u8; 256];
    let n = unsafe { read(file.get(), buf.as_mut_ptr() as *mut c_void, buf.len() - 1) };
    if n < 0 {
        let errno = last_errno();
        log::warn!(
            "attr read failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    }
    if n == 0 {
        return false;
    }
    let Ok(text) = std::str::from_utf8(&buf[..n as usize]) else {
        log::warn!("attr not utf8 pid={} bytes={}", pid, n);
        return false;
    };
    let context = text.trim();
    if context.contains("zygote") {
        return false;
    }
    log::debug!("proc ctx ready pid={} ctx={}", pid, context);
    true
}

fn is_process_storage_mount_ready(pid: i32, user_id: i32) -> bool {
    let path = format!("/proc/{}/mountinfo", pid);
    let Ok(content) = std::fs::read_to_string(&path) else {
        let errno = last_errno();
        log::warn!(
            "mountinfo read failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    };
    let storage_user = format!(" /storage/emulated/{user_id} ");
    let mnt_user = format!(" /mnt/user/{user_id}/emulated/{user_id} ");
    let has_storage = content
        .lines()
        .any(|line| line.contains(" /storage/emulated ") || line.contains(&storage_user));
    let has_mnt_user = content.lines().any(|line| line.contains(&mnt_user));
    if has_storage && has_mnt_user {
        log::debug!("proc storage mount ready pid={} user={}", pid, user_id);
        return true;
    }
    false
}

// 父进程等待子进程挂载结果并回收子进程
fn handle_parent_process(child: i32, sock: c_int, primary_timeout_sec: i64) -> bool {
    set_recv_timeout(sock, child, primary_timeout_sec);

    let mut result: i32 = -1;
    let expected_size = std::mem::size_of::<i32>() as isize;
    let mut n = recv_result(sock, &mut result);
    let mut should_reap_nonblocking = false;

    // 主超时未拿到结果时按 SIGTERM -> grace -> SIGKILL 渐进推进，
    // 避免在子进程仍持有 mount writer 时立刻 SIGKILL 损伤 FUSE 状态。
    if n != expected_size {
        log_recv_failure(child, n, expected_size, "primary");
        log_child_diagnostics(child, "primary_timeout");

        if unsafe { kill(child, SIGTERM) } != 0 {
            let errno = last_errno();
            log::warn!(
                "term child failed child={} errno={} {}",
                child,
                errno,
                errno_text(errno)
            );
        }

        set_recv_timeout(
            sock,
            child,
            mount_timing::COMPANION_PARENT_RECV_GRACE_TIMEOUT_SEC,
        );
        n = recv_result(sock, &mut result);
        if n == expected_size {
            log::warn!("child late result child={} ret={}", child, result);
        } else {
            log_recv_failure(child, n, expected_size, "grace");
            log_child_diagnostics(child, "grace_timeout");
            log::warn!("child stuck after term child={} forcing kill", child);
            should_reap_nonblocking = true;
            if unsafe { kill(child, SIGKILL) } != 0 {
                let errno = last_errno();
                log::warn!(
                    "kill child failed child={} errno={} {}",
                    child,
                    errno,
                    errno_text(errno)
                );
            }
        }
    }
    unsafe { close(sock) };

    reap_child(child, should_reap_nonblocking);

    let is_success = result == 0;
    if is_success {
        update_redirect_stats();
    } else {
        log::warn!("mount failed child={} recv={} ret={}", child, n, result);
    }
    is_success
}

fn reap_child(child: i32, nonblocking: bool) {
    let mut status: c_int = 0;
    let options = if nonblocking { WNOHANG } else { 0 };
    let attempts = if nonblocking { 20 } else { 1 };
    for attempt in 0..attempts {
        let wait_ret = unsafe { waitpid(child, &mut status as *mut _, options) };
        if wait_ret < 0 {
            let errno = last_errno();
            log::warn!(
                "waitpid failed child={} errno={} {}",
                child,
                errno,
                errno_text(errno)
            );
            return;
        }
        if wait_ret > 0 {
            log::info!(
                "child reaped child={} status={} raw={}",
                child,
                decode_wait_status(status),
                status
            );
            return;
        }
        if !nonblocking {
            break;
        }
        if attempt + 1 < attempts {
            unsafe { libc::usleep(10 * 1000) };
        }
    }

    log::warn!(
        "child not reaped child={} status=still_running reason=nonblocking_timeout",
        child
    );
}

fn log_recv_failure(child: i32, n: isize, expected: isize, phase: &str) {
    if n < 0 {
        let errno = last_errno();
        log::warn!(
            "recv result failed child={} phase={} errno={} {}",
            child,
            phase,
            errno,
            errno_text(errno)
        );
    } else {
        log::warn!(
            "recv result short child={} phase={} recv={} want={}",
            child,
            phase,
            n,
            expected
        );
    }
}

fn cleanup_previous_companion_mounts(state_path: &str, request: &CompanionMountRequest) {
    let Ok(content) = std::fs::read_to_string(state_path) else {
        return;
    };
    let mut targets = content
        .lines()
        .filter_map(|line| line.strip_prefix("target="))
        .filter(|target| !target.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    // path_mappings 形态切换时旧 state 可能只记录子映射目标；把全部存储别名根
    // 纳入清理，避免旧视图根继续压在新映射层上。
    if !request.path_mappings.is_empty() && request.uid >= 0 {
        targets.extend(crate::platform::paths::storage_alias_roots_for_user(
            crate::platform::user_id_from_uid(request.uid),
        ));
    }
    targets.sort_by_key(|target| std::cmp::Reverse(target.len()));
    targets.dedup();
    for target in targets {
        let Ok(c_target) = CString::new(target.as_str()) else {
            continue;
        };
        // SAFETY: c_target 是合法的 NUL 结尾路径，当前子进程已进入目标应用挂载命名空间。
        if unsafe { libc::umount2(c_target.as_ptr(), libc::MNT_DETACH) } == 0 {
            log::info!(
                "companion reload detached previous target={} pid={} pkg={}",
                target,
                request.pid,
                request.package_name
            );
        }
    }
    for line in content.lines() {
        let Some(value) = line.strip_prefix("fuse_child=") else {
            continue;
        };
        let Some((pid_text, start_text)) = value.split_once(':') else {
            continue;
        };
        let (Ok(pid), Ok(start_time_ticks)) = (pid_text.parse::<i32>(), start_text.parse::<u64>())
        else {
            continue;
        };
        if pid > 0 && crate::platform::is_process_instance_alive(pid, start_time_ticks) {
            crate::fuse_terminate::terminate_fuse_process(pid, Some(start_time_ticks));
        }
    }
    let _ = std::fs::remove_file(state_path);
}

// 子进程切换命名空间并执行实际挂载
fn handle_child_process(
    request: &CompanionMountRequest,
    plan: &CompanionMountForkPlan,
    sock: c_int,
) -> bool {
    if !set_mount_namespace(request.pid, plan.mount_namespace_path.as_deref()) {
        log::error!(
            "child setns failed pid={} pkg={}",
            request.pid,
            request.package_name
        );
        let _ = send_mount_result(sock, -1);
        unsafe { close(sock) };
        return false;
    }

    // Companion 也会处理同一 PID 的热重载请求，必须先回收上一轮状态文件中的
    // target/fuse_child。此前只有 daemon 路径执行 clear_previous_mounts，companion
    // 会在旧视图根和新映射层之上继续叠挂，完整场景的前置 MediaStore 场景因此会
    // 让 scenario 29 的旧进程同时命中请求路径与映射目标。
    cleanup_previous_companion_mounts(&plan.state_path, request);

    let mut mount_mgr = MountPlanner::new(
        &request.package_name,
        request.uid,
        &request.app_data_dir,
        &request.redirect_target,
        false,
    );
    mount_mgr.set_file_monitor_enabled(request.is_file_monitor_enabled);

    let scoped_fuse_roots = plan.scoped_fuse_roots.as_slice();
    // 残留屏障清扫（必须在挂载动作之前、setns 之后）：白名单用 barrier 集合（与
    // execute 层挂载同源），不能用收敛后的 scoped_fuse_roots——宿主在位时后者收敛成
    // 存储根，会把 execute 层刚挂好的屏障误判成残留拆掉（应用视图裸奔 → ENOENT）。
    detach_stale_barriers_not_in(&plan.barrier_roots);
    let cleared_system_view_before_preattach =
        if shared_host_preattach_candidate(request, scoped_fuse_roots)
            && crate::system_fuse_view::should_clear_system_fuse_view_for_platform()
        {
            crate::system_fuse_view::log_view_stack_for_package(
                request.uid,
                &request.package_name,
                "before_shared_host_preattach",
            );
            crate::system_fuse_view::clear_system_fuse_view_for_package(
                request.uid,
                &request.package_name,
            );
            true
        } else {
            false
        };
    // 共享宿主必须先于 namespace 重定向接管存储根，避免 SQLite 在接管前打开
    // 重定向沙盒中的旧副本。
    let preattached_host = preattach_shared_host(request, scoped_fuse_roots);
    let is_success = if preattached_host.is_some() {
        true
    } else if request.is_monitor_only {
        mount_mgr.ensure_mount_namespace_prepared()
    } else if request.is_mapping_mode_only {
        log::info!("map-only mount count={}", request.path_mappings.len());
        mount_mgr.apply_path_mappings_only(
            &request.path_mappings,
            &request.sandboxed_paths,
            &request.read_only_paths,
            scoped_fuse_roots,
        )
    } else {
        mount_mgr.apply_sdcard_redirect(
            &request.allowed_real_paths,
            &request.excluded_real_paths,
            &request.read_only_paths,
            &request.path_mappings,
            scoped_fuse_roots,
        )
    };

    // 注意：屏障裸奔检测不能放在这里——非 preattach 路径的 FUSE 会话在
    // start_scoped_fuse_services（成功分支内）才启动，此处屏障必然裸奔，
    // 在此判定会把每次正常挂载误判为失败并跳过 FUSE 启动。检测位于成功
    // 分支尾部（conclude_scoped_mount 之后、send_mount_result 之前）。
    let result = if is_success { 0 } else { -1 };
    if !is_success {
        log::warn!(
            "child mount failed pid={} pkg={} map_only={}",
            request.pid,
            request.package_name,
            request.is_mapping_mode_only
        );
    } else {
        let fuse_roots = scoped_fuse_roots;
        log_scoped_mount_roots(
            "hybrid fuse",
            &request.package_name,
            request.pid,
            fuse_roots,
        );
        // 与 daemon 侧同源：闸门判定用同一个函数，只是在这里取出来让 selection_reason 能区分
        // 「能力未放行」与「规则本来不需要 FUSE 根」。
        let gate_allowed = crate::fuse_redirect::config::scoped_mount_allowed_for_scope(
            &request.package_name,
            request.storage_backend_mode,
        );
        let (fuse_children, attempt) = if let Some(state) = preattached_host {
            (vec![state], ScopedMountAttempt::Ready)
        } else if !fuse_roots.is_empty() {
            match start_scoped_fuse_services(request, fuse_roots, mount_mgr.real_storage_anchor()) {
                Some(children) => (children, ScopedMountAttempt::Ready),
                None => (Vec::new(), ScopedMountAttempt::Failed),
            }
        } else if gate_allowed {
            (Vec::new(), ScopedMountAttempt::NoRootsNeeded)
        } else {
            (Vec::new(), ScopedMountAttempt::GateBlocked)
        };
        let outcome = conclude_scoped_mount(ScopedMountReport {
            package_name: &request.package_name,
            pid: request.pid,
            requested_mode: request.storage_backend_mode,
            log_prefix: "hybrid fuse",
            roots_planned: fuse_roots.len(),
            sessions: fuse_children.len(),
            attempt,
            ready_reason: "companion_scoped_mount_ready",
            failed_reason: "companion_scoped_mount_failed",
        });
        let mut namespace_fallback_ok = false;
        if outcome.needs_namespace_fallback {
            log::warn!(
                "hybrid fuse no scoped service mounted, fallback to mount namespace pid={} pkg={}",
                request.pid,
                request.package_name
            );
            if crate::fuse_session::apply_mount_namespace_fallback(&mut mount_mgr, request) {
                // fallback 用 bind 覆盖根（无 FUSE 会话）：屏障裸奔检测只对
                // FUSE 路径有意义，fallback 成功时跳过，否则必误报。
                namespace_fallback_ok = true;
            } else {
                log::warn!(
                    "hybrid fuse namespace fallback failed pid={} pkg={}",
                    request.pid,
                    request.package_name
                );
            }
        }
        // 与 daemon 侧同源、同顺序：挂载完成后按平台需要摘掉系统 MediaProvider 的 app data
        // isolation FUSE 视图，再补回被 MNT_DETACH 级联摘掉的映射子路径 bind。摘除逻辑此前只
        // 写在 daemon 路径里，走 companion 的应用（普通应用正是这条）在 x86_64 Android 13/14
        // 上继续失败。顺序约束见 crate::system_fuse_view 模块文档。
        if !cleared_system_view_before_preattach
            && crate::system_fuse_view::should_clear_system_fuse_view_for_platform()
        {
            crate::system_fuse_view::log_view_stack_for_package(
                request.uid,
                &request.package_name,
                "before_clear",
            );
            crate::system_fuse_view::clear_system_fuse_view_for_package(
                request.uid,
                &request.package_name,
            );
            if !request.path_mappings.is_empty() {
                mount_mgr.reapply_path_mappings_only(&request.path_mappings);
            }
            crate::system_fuse_view::log_view_stack_for_package(
                request.uid,
                &request.package_name,
                "after_clear",
            );
        }
        // 自有媒体目录恢复必须放在**所有存储根挂载动作之后**。此前它写在
        // `apply_sdcard_redirect` 内部（早于 `start_scoped_fuse_services`），随后 srx FUSE
        // 覆盖存储根会把子 bind 整体遮蔽——同一构建时而生效时而丢失（真机 2026-10-25：
        // 支付宝连续几次挂载里只有 preattach 那次留下 bind）。放在这里对
        // preattach / scoped / namespace fallback 三种形态统一生效。
        //
        // 语义：`Android/media/<pkg>` 若留在 srx 宿主 FUSE 内，后端是 f2fs 直连；未接管的
        // 消费方（MediaProvider 自身、跨包管理器、Shizuku/shell）经系统 MediaProvider FUSE
        // 访问同一物理文件时属于另一层内核缓存，SQLite 的 `-shm` mmap 与页内锁跨不过去，
        // WAL 被判无效后回滚到上一次 checkpoint 的主库（真机 XRadiant 主库落到 4096B 空页）。
        let restored_own_private = mount_mgr.restore_own_private_system_view();
        log::info!(
            "own private system view restored count={} pkg={} pid={}",
            restored_own_private,
            request.package_name,
            request.pid
        );
        let mounted_targets = mount_mgr.take_mounted_targets();
        // 新进程首次接入存储视图时的属主交接修复（挂载应答前同步执行）：原生
        // FUSE（未映射路径、namespace 回退、未挂载窗口）要求应用私有子树属主是
        // 应用 uid，而 MediaProvider 代写会留下 media_rw 属主条目（真机 2026-10-17
        // 反馈：A17 上仅映射应用探测自有 Telegram 目录、Unity 游戏读存档全部
        // EACCES）。companion 路径绕过 daemon 子进程，必须与 daemon fork 路径
        // 一样在应答前修复——真机 2026-10-18 实测 mapping_mode_only 应用的挂载
        // 普遍走本路径，只挂 daemon fork 会完全漏修。只对「无 mount identity
        if !mount_state::write_mount_state(request, plan, &mounted_targets, &fuse_children) {
            log::warn!(
                "mount state save failed pid={} pkg={}",
                request.pid,
                request.package_name
            );
        }
        // 账本与状态文件职责不同：状态文件回答"要摘哪些路径"，账本回答"那些挂载归谁"。
        // 这条路径此前只写状态文件、不写账本，于是走 companion 的应用在恢复流程与 doctor 里
        // 没有任何归属判据，只能退化成"按挂载源判定"这个较弱的判据。两条挂载路径必须都登记，
        // 否则同一件事在两条路径上的结果不一致——这正是本项目反复踩的坑。
        let mut identity_targets = mounted_targets.clone();
        identity_targets.extend(fuse_children.iter().map(|state| state.target.clone()));
        if !crate::mount_ledger::record_mount_identity(
            "companion",
            &request.package_name,
            request.pid,
            &identity_targets,
        ) {
            log::warn!(
                "mount identity save failed pid={} pkg={}",
                request.pid,
                request.package_name
            );
        }
        // 坍缩形态的屏障处置：按遍历感知判定——被视图根 FUSE 遮蔽的兄弟屏障不可达
        // 且无害（Inert），只有压在同挂载点 srx FUSE 之上的残留屏障才真正回收。
        if !namespace_fallback_ok && !fuse_children.is_empty() {
            dispose_barriers_after_view_mount(
                &plan.barrier_roots,
                &request.package_name,
                request.pid,
            );
        }
        // 屏障裸奔检测：scoped_fuse_roots 非空时，成功判定必须包含「每个屏障根的
        // 栈顶已被 srx FUSE 覆盖」。存在过的失败形态：会话建立失败但 bind 链判
        // ok=true，屏障（只读空 tmpfs）裸奔在栈顶——应用写入 ENOENT/EROFS，且
        // 应用重启无效（同 namespace），直到下一次完整挂载清掉为止。这里判失败：
        // fail-closed 屏障保留（不给应用裸视图），重试时规则已正确即可压上 FUSE。
        if !namespace_fallback_ok {
            // 覆盖判定按屏障根逐个做祖先回退（坍缩后屏障根自身在 mounts 里没有
            // 条目，只按挂载点精确匹配会漏掉子路径上的暴露屏障）。
            let exposed_check_roots: &[String] = if plan.barrier_roots.is_empty() {
                scoped_fuse_roots
            } else {
                &plan.barrier_roots
            };
            for root in exposed_check_roots {
                // 覆盖判定按 mountinfo 遍历模拟（扁平 /proc/mounts 表达不了挂载树
                // 父子可达性，坍缩形态下被视图根遮蔽的兄弟屏障会被误判成裸奔）。
                if !is_storage_path_served_by_srx_fuse_traversal(root) {
                    let mounts_at_check = std::fs::read_to_string("/proc/self/mounts")
                        .map(|content| {
                            content
                                .lines()
                                .filter(|line| {
                                    line.contains("/storage/emulated") || line.contains("srx_fuse")
                                })
                                .collect::<Vec<_>>()
                                .join(" | ")
                        })
                        .unwrap_or_else(|error| format!("mounts read failed: {}", error));
                    log::error!(
                        "barrier exposed without fuse cover root={} pkg={} pid={} mounts_at_check={}",
                        root,
                        request.package_name,
                        request.pid,
                        mounts_at_check
                    );
                    let _ = send_mount_result(sock, -1);
                    // SAFETY: sock 是本 fork 子进程持有的已连接控制端 fd，失败回包
                    // 已发送完毕，此处关闭后立即返回，无其它线程并发使用该 fd。
                    unsafe { close(sock) };
                    return false;
                }
            }
        }
        if !send_mount_result(sock, 0) {
            log::warn!(
                "child send result failed pid={} pkg={}",
                request.pid,
                request.package_name
            );
        }
        unsafe { close(sock) };
        return true;
    }
    log::info!(
        "companion mount {} pid={}",
        if is_success { "ok" } else { "fail" },
        request.pid
    );

    if !send_mount_result(sock, result) {
        log::warn!(
            "child send result failed pid={} pkg={}",
            request.pid,
            request.package_name
        );
    }
    unsafe { close(sock) };
    is_success
}

/// 在任何 namespace 重定向前接入共享宿主，避免应用先打开重定向沙盒中的文件。
/// 只有 Auto 模式且规划根已经收敛为整棵用户存储视图时才提前接管；预接管仅尝试
/// 共享宿主，失败后由后续挂载流程启动 scoped 会话。
fn preattach_shared_host(
    request: &CompanionMountRequest,
    scoped_fuse_roots: &[String],
) -> Option<FuseMountState> {
    if !shared_host_preattach_candidate(request, scoped_fuse_roots) {
        return None;
    }
    let user_id = platform::user_id_from_uid(request.uid);
    let storage_root = platform::paths::storage_user_root_for_user(user_id);

    let view = crate::fuse_host::read_host_session_view()?;
    if view.policy_fingerprints.get(&(request.uid as u32)) != Some(&request.policy_fingerprint)
        || !crate::fuse_host::can_attach_app(request.uid, &storage_root)
    {
        return None;
    }
    let state = try_bind_to_fuse_host(&view, request, &storage_root)?;
    log::info!(
        "companion shared host preattached pid={} pkg={} target={}",
        request.pid,
        request.package_name,
        storage_root
    );
    Some(state)
}

fn shared_host_preattach_candidate(
    request: &CompanionMountRequest,
    scoped_fuse_roots: &[String],
) -> bool {
    let is_auto = matches!(
        request.storage_backend_mode,
        crate::config::StorageBackendMode::Auto
    );
    let is_monitor_only = request.is_monitor_only && request.is_file_monitor_enabled;
    if !is_auto && !is_monitor_only {
        return false;
    }
    // 与 daemon 侧同源（crate::daemon_mount::shared_host_preattach_candidate）：存在
    // 私有 fs 侧映射时 preattach 成功会短路 planner，私有映射的命名空间 bind 挂载
    // 会被整体丢弃（真机实证：scenario 35 映射矩阵全失效）。命中时必须回落
    // planner 流程，由 planner 一边宿主接入一边物化私有映射。
    if crate::domain::mappings_need_namespace_mount(
        request.uid,
        &request.app_data_dir,
        &request.redirect_target,
        &request.path_mappings,
    ) {
        return false;
    }
    let user_id = platform::user_id_from_uid(request.uid);
    let storage_root = platform::paths::storage_user_root_for_user(user_id);
    scoped_fuse_roots.len() == 1
        && platform::paths::normalize_syntax(&scoped_fuse_roots[0])
            == platform::paths::normalize_syntax(&storage_root)
}

/// 按根启动 scoped FUSE 服务；单根失败只丢弃该根。
///
/// daemon 侧 [`crate::daemon_mount`] 有一份平行实现，两处必须保持一致：它们写入的是
/// 同一份 FUSE 能力快照，任一侧整组回滚都会把失败计入全局预算，累计到上限后把所有
/// 应用的 scoped 会话一起打回 mount namespace。
fn start_scoped_fuse_services(
    request: &CompanionMountRequest,
    roots: &[String],
    real_root_override: Option<String>,
) -> Option<Vec<FuseMountState>> {
    crate::fuse_session::start_scoped_fuse_services(
        request,
        roots,
        real_root_override,
        |root, real_root_override| start_fuse_service_for_root(request, root, real_root_override),
    )
}

fn scoped_fuse_mount_roots(request: &CompanionMountRequest) -> Vec<String> {
    let roots = crate::fuse_redirect::scoped_fuse_mount_roots_for_request(request);
    // Auto 模式的目标形态是共享宿主会话，与 daemon 侧规划同一语义：把按目录 scoped 根
    // 收敛成存储视图根。区别只在发现方式——companion 与 daemon 无共享内存，靠快照文件
    // 等待"宿主在位 + 本 uid 已被 daemon 预登记"；等待发生在挂载应答返回之前，应用进程
    // 尚未恢复运行，一次挂载到位，没有"先 scoped 后迁移"的 fail-open 中间态。快照缺失、
    // 等待超时或处于失败冷却期时保持旧规划，scoped 数据面照常服务。
    //
    // 存在私有 fs 侧映射时保持按目录规划（与 daemon 侧同源判定）：宿主策略层只覆盖
    // 存储视图，target 在视图外的映射会被宿主忽略，写入穿透到默认重定向根（真机实证：
    // scenario 35 的 Download/X → /data/user/0/<pkg>/no_backup 在收敛路径下落到
    // sdcard 沙盒）。按目录 scoped 会话与命名空间 bind 才能完整表达这类映射。
    if crate::domain::mappings_need_namespace_mount(
        request.uid,
        &request.app_data_dir,
        &request.redirect_target,
        &request.path_mappings,
    ) {
        return roots;
    }
    if roots.is_empty()
        || !matches!(
            request.storage_backend_mode,
            crate::config::StorageBackendMode::Auto
        )
    {
        return roots;
    }
    let Some(_view) =
        crate::fuse_host::wait_for_host_session_view(request.uid, request.policy_fingerprint)
    else {
        log::info!(
            "fuse host wait not ready roots={} mode={} pid={} pkg={}",
            roots.len(),
            request.storage_backend_mode.as_str(),
            request.pid,
            request.package_name
        );
        return roots;
    };
    let user_id = crate::platform::user_id_from_uid(request.uid);
    let view_root = platform::paths::storage_user_root_for_user(user_id);
    if roots.len() == 1
        && platform::paths::normalize_syntax(&roots[0])
            == platform::paths::normalize_syntax(&view_root)
    {
        return roots;
    }
    log::info!(
        "fuse host preferred collapse roots={} -> view root pid={} pkg={}",
        roots.len(),
        request.pid,
        request.package_name
    );
    vec![view_root]
}

fn start_fuse_service_for_root(
    request: &CompanionMountRequest,
    mount_root: &str,
    real_root_override: Option<String>,
) -> Option<FuseMountState> {
    // B2-b：优先尝试共享宿主会话接入（跨进程发现）。
    //
    // companion 是 Zygisk companion 进程，与 daemon 没有共享内存：`get_fuse_host()` 在本
    // 进程恒为 `None`，宿主会话只能通过 daemon 发布的快照文件发现。策略登记同样由 daemon
    // 预先完成（companion 没有控制通道），接入前用快照里的 uid 集合确认，未登记 uid 的
    // 请求会被宿主 fail-closed 拒绝成 ENOENT。
    //
    // `real_root_override` 必须丢弃（传 None）：它是应用命名空间专属的锚点别名，只存在于
    // 应用 namespace；宿主子进程在自己的私有命名空间里看到的是空目录，照搬覆盖会把真实根
    // 读成空，仅映射模式的应用会整个丢失公共存储视图。companion 不做登记（daemon 预登记），
    // 因此这里不构造策略配置。
    let view = crate::fuse_host::read_host_session_view();
    let registered = view.as_ref().is_some_and(|view| {
        view.policy_fingerprints.get(&(request.uid as u32)) == Some(&request.policy_fingerprint)
    });
    if !crate::fuse_host::can_attach_app(request.uid, mount_root) {
        // 宿主会话的虚拟根只能是整个存储视图根，且默认不接管应用挂载；两种情况都保持
        // 既有 scoped 路径，不能因为"宿主会话可用"就顺手接上。
        log::debug!(
            "fuse host attach skipped pid={} pkg={} target={} registered={} reason=attach_gate_closed",
            request.pid,
            request.package_name,
            mount_root,
            registered
        );
    } else if !registered {
        // 策略没进宿主会话时接入会让应用拿到"未登记即拒绝"的空视图；宁可继续 scoped。
        log::warn!(
            "fuse host attach skipped pid={} pkg={} target={} request_fingerprint={:016x} snapshot_fingerprint={:?} reason=policy_registration_failed",
            request.pid,
            request.package_name,
            mount_root,
            request.policy_fingerprint,
            view.as_ref()
                .and_then(|view| view.policy_fingerprints.get(&(request.uid as u32)))
                .copied()
        );
    } else if let Some(host_view) = view.as_ref() {
        if let Some(state) = try_bind_to_fuse_host(host_view, request, mount_root) {
            return Some(state);
        }
        log::warn!(
            "bind to fuse host failed, falling back to scoped fork pid={} pkg={}",
            request.pid,
            request.package_name
        );
    } else {
        log::warn!(
            "fuse host view disappeared before attach, falling back to scoped fork pid={} pkg={}",
            request.pid,
            request.package_name
        );
    }
    // 宿主接入失败时，daemon reconcile 可能已经为该应用挂上了共享宿主层；此时
    // 再叠 scoped 会话只会造成双重挂载。检测到已有 srx 层就直接放弃本根。
    if app_storage_view_already_served(mount_root) {
        log::info!(
            "scoped fork skipped, srx mount already present pid={} pkg={} target={}",
            request.pid,
            request.package_name,
            mount_root
        );
        return None;
    }
    // 回退：fork 独立 scoped 会话（B2-a 前的既有路径）。
    let mut ready_sockets = [0; 2];
    // SAFETY: socketpair 系统调用，传入有效的栈数组指针。
    if unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, ready_sockets.as_mut_ptr()) } != 0 {
        let errno = last_errno();
        log::warn!(
            "fuse ready socketpair failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
        return None;
    }

    // 先在父进程走完私有日志通道初始化，避免子进程继承处于初始化中的 OnceLock 而永久阻塞。
    crate::logging::prepare_for_fork();
    // SAFETY: fork 系统调用，已经通过 prepare_for_fork 避免日志通道竞态。
    let service_child = unsafe { libc::fork() };
    if service_child < 0 {
        let errno = last_errno();
        log::warn!(
            "fuse fork failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
        // SAFETY: ready_sockets 是有效 fd，fork 失败后父进程负责清理。
        unsafe {
            close(ready_sockets[0]);
            close(ready_sockets[1]);
        }
        return None;
    }

    if service_child == 0 {
        // SAFETY: 子进程关闭继承的 fd。
        unsafe {
            close(ready_sockets[0]);
        }
        let ok = mount_blocking_with_ready(
            fuse_config_from_request(request, Some(mount_root.to_string()), real_root_override),
            Some(ready_sockets[1]),
        );
        // SAFETY: 子进程直接退出，不执行析构函数（避免 fork 后的资源清理问题）。
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // SAFETY: 父进程关闭写端 fd。
    unsafe { close(ready_sockets[1]) };
    set_recv_timeout(
        ready_sockets[0],
        service_child,
        mount_timing::FUSE_READY_TIMEOUT_SEC,
    );
    let mut ready_result: i32 = -1;
    let expected = std::mem::size_of::<i32>() as isize;
    let n = recv_result(ready_sockets[0], &mut ready_result);
    // SAFETY: 父进程关闭读端 fd。
    unsafe { close(ready_sockets[0]) };
    if n != expected || ready_result != 0 {
        log::warn!(
            "fuse service not ready child={} recv={} ret={} pid={} pkg={}",
            service_child,
            n,
            ready_result,
            request.pid,
            request.package_name
        );
        crate::fuse_terminate::terminate_fuse_process(service_child, None);
        // 与 daemon 侧同口径：子进程可能在 mount(2) 已完成后才被判失败，终止后若目标
        // 顶层残留本模块 scoped 死层，就地摘除，避免常驻应用命名空间留下 ENOTCONN 死挂载。
        crate::fuse_session::detach_dead_scoped_layer_after_terminate(mount_root);
        return None;
    }

    let Some(child_start_time_ticks) = crate::platform::process_start_time_ticks(service_child)
    else {
        rollback_scoped_fuse_services(&[FuseMountState {
            target: mount_root.to_string(),
            child: service_child,
            child_start_time_ticks: 0,
            host_session: None,
        }]);
        return None;
    };
    Some(FuseMountState {
        target: mount_root.to_string(),
        child: service_child,
        child_start_time_ticks,
        host_session: None,
    })
}

/// B2-b：把应用接入共享宿主 FUSE 会话（companion 路径）。
///
/// 跨命名空间的取句柄与 bind 都在 [`crate::fuse_host::attach_app_to_host`] 里，daemon 与
/// companion 共用同一份实现：这条路径上"bind 落在哪个命名空间"是唯一的关键点，两处各写
/// 一份最大的风险就是只有一处改对，而症状（应用静默失去重定向）不会报错。
///
/// 记账上必须与 scoped 会话区分：宿主会话跨应用共享，`child` 不能填宿主 pid，
/// 否则回滚会把它当本应用的子进程终止掉。
fn try_bind_to_fuse_host(
    view: &crate::fuse_host::HostSessionView,
    request: &CompanionMountRequest,
    mount_root: &str,
) -> Option<FuseMountState> {
    // companion 挂载子进程同样受父进程固定预算约束（primary 5s + 4s×根数，另有 1s 宽限）。
    // 接入平时亚秒完成；开机风暴期 FUSE 路径遍历可能远慢于平时，等待上限必须为 scoped
    // 回退留出完整的 FUSE_READY 周期，否则接入超时会顶穿父进程预算，子进程被 SIGKILL，
    // 这一轮挂载作废。按单根最坏预算取值（根数增加时预算同步增长，单根值始终安全）。
    let attach_timeout_sec = mount_timing::companion_parent_recv_budget_sec(1)
        .saturating_sub(mount_timing::FUSE_READY_TIMEOUT_SEC)
        .saturating_sub(1)
        .clamp(1, crate::fuse_host::HOST_ATTACH_TIMEOUT_SEC);
    let attached = crate::fuse_host::attach_app_to_host(view, mount_root, attach_timeout_sec)?;
    log::info!(
        "fuse host attach ok pid={} pkg={} target={} host={}",
        request.pid,
        request.package_name,
        attached.target,
        attached.host_pid
    );
    Some(FuseMountState {
        target: attached.target,
        child: 0,
        child_start_time_ticks: 0,
        host_session: Some((attached.host_pid, attached.host_start_time_ticks)),
    })
}

fn fuse_config_from_request(
    request: &CompanionMountRequest,
    mount_root: Option<String>,
    real_root_override: Option<String>,
) -> FuseRedirectConfig {
    crate::fuse_redirect::fuse_config_from_request(request, mount_root, real_root_override)
}

/// fork 之前在父进程算好的挂载计划。
///
/// 子进程只保留调用线程，不能依赖其它父线程在 fork 瞬间持有的 malloc arena 或全局锁，
/// 所以所有可以提前得到的路径字符串都放在这里，子进程直接复用已分配好的内容。
struct CompanionMountForkPlan {
    /// 需要交给 FUSE 服务接管的挂载根，父子进程共用同一份结果。
    scoped_fuse_roots: Vec<String>,
    /// execute 层 barrier 的挂载集合（scoped_fuse_mount_roots_for_request 原始结果）。
    /// 残留清扫的白名单以此为准：只清不在其中的屏障。
    barrier_roots: Vec<String>,
    /// 挂载状态文件路径。
    state_path: String,
    /// 挂载状态文件的临时写入路径。
    temp_state_path: String,
    /// 目标进程的 mount namespace 路径，已提前转换为 C 字符串。
    mount_namespace_path: Option<CString>,
}

impl CompanionMountForkPlan {
    fn build(request: &CompanionMountRequest) -> Self {
        let state_path = mount_state::state_file_path(request);
        let temp_state_path = format!("{}.tmp", state_path);
        // barrier 集合必须与 execute 层 mount_storage_view_barriers 的挂载集合同源
        // （scoped_fuse_mount_roots_for_request 的原始按目录结果），而不是本结构体的
        // scoped_fuse_roots——后者在 Auto+宿主在位时会收敛成存储视图根，白名单若用它，
        // 残留清扫会把 execute 层刚挂好的屏障拆掉（应用视图裸奔 → ENOENT）。
        let barrier_roots = crate::fuse_redirect::scoped_fuse_mount_roots_for_request(request);
        Self {
            scoped_fuse_roots: scoped_fuse_mount_roots(request),
            barrier_roots,
            state_path,
            temp_state_path,
            mount_namespace_path: CString::new(format!("/proc/{}/ns/mnt", request.pid)).ok(),
        }
    }
}

// 通过 socketpair 创建子进程执行挂载操作
fn run_mount_in_forked_child(request: &CompanionMountRequest) -> bool {
    // fork 之后的子进程只保留调用线程。此时若再做堆分配、首次初始化或获取全局锁，
    // 可能因为其它父线程在 fork 瞬间持有 malloc arena 或全局锁而永久阻塞。
    // 因此把可以提前算出的字符串与路径列表全部在父进程算好，子进程只做 setns/mount/write。
    let plan = CompanionMountForkPlan::build(request);
    let scoped_fuse_root_count = plan.scoped_fuse_roots.len();
    let parent_timeout_sec =
        mount_timing::companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count);
    log::info!(
        "mount prep pid={} uid={} pkg={} allow={} ro={} map={} map_only={} parent_recv_budget_sec={}",
        request.pid,
        request.uid,
        request.package_name,
        request.allowed_real_paths.len(),
        request.read_only_paths.len(),
        request.path_mappings.len(),
        request.is_mapping_mode_only,
        mount_timing::companion_parent_recv_budget_sec(scoped_fuse_root_count)
    );

    let mut sockets = [0; 2];
    let ret = unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, sockets.as_mut_ptr()) };
    if ret != 0 {
        let errno = last_errno();
        log::error!(
            "socketpair failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
        return false;
    }

    // 先在父进程走完私有日志通道初始化，避免子进程继承处于初始化中的 OnceLock 而永久阻塞。
    crate::logging::prepare_for_fork();
    let child = unsafe { libc::fork() };
    if child < 0 {
        let errno = last_errno();
        log::error!(
            "fork failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
        unsafe {
            close(sockets[0]);
            close(sockets[1]);
        }
        return false;
    }

    if child > 0 {
        log::debug!("parent wait child={}", child);
        unsafe { close(sockets[1]) };
        return handle_parent_process(child, sockets[0], parent_timeout_sec);
    }

    log::debug!(
        "child start pid={} pkg={}",
        request.pid,
        request.package_name
    );
    unsafe { close(sockets[0]) };
    let sock = sockets[1];
    let is_success = handle_child_process(request, &plan, sock);
    unsafe { libc::_exit(if is_success { 0 } else { 1 }) };
}

#[cfg(test)]
mod tests {
    use super::{
        BarrierDispose, barrier_dispose_decision, is_storage_path_served_by_srx_fuse_mountinfo_in,
        traverse_mountinfo,
    };

    // mountinfo 行构造辅助：id parent 前缀随机但要真实成对。
    // 33493=ns 根挂载；33494=系统 MediaProvider FUSE /storage/emulated；
    // 33513=共享宿主 srx FUSE /storage/emulated/0；33509-33512=execute 屏障。
    const COLLAPSED_MOUNTINFO: &str = concat!(
        "33493 1 259:40 / / rw,... - ext2 /dev/block/dm-40 rw,...\n",
        "33494 33493 0:225 / /storage/emulated rw,nosuid,nodev,noexec,noatime - fuse /dev/fuse rw,...\n",
        "33509 33494 0:726 / /storage/emulated/0/DCIM ro,nosuid,nodev,relatime - tmpfs none ro,seclabel,size=4096k\n",
        "33510 33494 0:727 / /storage/emulated/0/Android ro,nosuid,nodev,relatime - tmpfs none ro,seclabel,size=4096k\n",
        "33511 33494 0:728 / /storage/emulated/0/Download ro,nosuid,nodev,relatime - tmpfs none ro,seclabel,size=4096k\n",
        "33512 33494 0:729 / /storage/emulated/0/Pictures ro,nosuid,nodev,relatime - tmpfs none ro,seclabel,size=4096k\n",
        "33513 33494 0:154 / /storage/emulated/0 rw,nosuid,nodev,noatime - fuse srx_fuse_host[10062] rw,...\n",
    );

    // 真机 2026-10-15 误报回归：坍缩形态下屏障（33509-33512）与视图根 srx FUSE
    // （33513）是同一父挂载下的兄弟，路径遍历先进视图根 FUSE，屏障整只被遮蔽且
    // 按路径不可达（umount2 EINVAL/ENOENT），必须判「已覆盖」。
    #[test]
    fn collapsed_sibling_barriers_are_shadowed_and_served() {
        assert!(is_storage_path_served_by_srx_fuse_mountinfo_in(
            COLLAPSED_MOUNTINFO,
            "/storage/emulated/0/DCIM"
        ));
        assert!(is_storage_path_served_by_srx_fuse_mountinfo_in(
            COLLAPSED_MOUNTINFO,
            "/storage/emulated/0/Android"
        ));
        assert!(is_storage_path_served_by_srx_fuse_mountinfo_in(
            COLLAPSED_MOUNTINFO,
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/log/"
        ));
    }

    // 坍缩形态下被遮蔽屏障的处置必须是 Inert：不可达（umount2 必 EINVAL），保留无害。
    #[test]
    fn collapsed_sibling_barriers_dispose_inert() {
        let entries: Vec<_> = COLLAPSED_MOUNTINFO
            .lines()
            .filter_map(super::parse_mountinfo_line)
            .collect();
        let outcome = traverse_mountinfo(&entries, "/storage/emulated/0/DCIM");
        assert_eq!(barrier_dispose_decision(&outcome), BarrierDispose::Inert);
    }

    // 屏障裸奔形态：子路径遍历栈顶是只读 tmpfs 屏障，即便视图根有 srx FUSE 也必须
    // 判未覆盖——屏障真实可达并遮蔽其子树（这是 fail-closed 要拦的真形态）。
    // 与坍缩兄弟形态的区别：此处屏障的父挂载是 0 下的 FUSE（33513），遍历会穿过
    // FUSE 到达屏障。
    #[test]
    fn reachable_barrier_on_top_is_not_served() {
        let mountinfo = concat!(
            "33493 1 259:40 / / rw,... - ext2 /dev/block/dm-40 rw,...\n",
            "33494 33493 0:225 / /storage/emulated rw,... - fuse /dev/fuse rw,...\n",
            "33513 33494 0:154 / /storage/emulated/0 rw,... - fuse srx_fuse_host[10062] rw,...\n",
            "33599 33513 0:726 / /storage/emulated/0/Android ro,nosuid,nodev,relatime - tmpfs none ro,seclabel,size=4096k\n",
        );
        assert!(!is_storage_path_served_by_srx_fuse_mountinfo_in(
            mountinfo,
            "/storage/emulated/0/Android"
        ));
        assert!(!is_storage_path_served_by_srx_fuse_mountinfo_in(
            mountinfo,
            "/storage/emulated/0/Android/media"
        ));
    }

    // 同挂载点叠放：FUSE 压在屏障之上（文件序在后的为栈顶），判已覆盖。
    #[test]
    fn stacked_fuse_on_top_of_barrier_is_served() {
        let mountinfo = concat!(
            "33493 1 259:40 / / rw,... - ext2 /dev/block/dm-40 rw,...\n",
            "33510 33493 0:727 / /storage/emulated/0/Android ro,... - tmpfs none ro,...\n",
            "33513 33493 0:154 / /storage/emulated/0/Android rw,... - fuse srx_fuse_host[10062] rw,...\n",
        );
        assert!(is_storage_path_served_by_srx_fuse_mountinfo_in(
            mountinfo,
            "/storage/emulated/0/Android"
        ));
    }

    // 残留形态：屏障压在同挂载点 srx FUSE 之上（屏障文件序在后），判未覆盖且处置
    // 为 Detach——拆掉屏障即可恢复 FUSE 服务。
    #[test]
    fn barrier_on_top_of_fuse_requires_detach() {
        let mountinfo = concat!(
            "33493 1 259:40 / / rw,... - ext2 /dev/block/dm-40 rw,...\n",
            "33513 33493 0:154 / /storage/emulated/0/Android rw,... - fuse srx_fuse_host[10062] rw,...\n",
            "33510 33493 0:727 / /storage/emulated/0/Android ro,... - tmpfs none ro,...\n",
        );
        assert!(!is_storage_path_served_by_srx_fuse_mountinfo_in(
            mountinfo,
            "/storage/emulated/0/Android"
        ));
        let entries: Vec<_> = mountinfo
            .lines()
            .filter_map(super::parse_mountinfo_line)
            .collect();
        let outcome = traverse_mountinfo(&entries, "/storage/emulated/0/Android");
        assert_eq!(barrier_dispose_decision(&outcome), BarrierDispose::Detach);
    }

    // 无任何 srx FUSE（连祖先都没有）时判未覆盖，保持 fail-closed 语义。
    #[test]
    fn path_without_any_srx_fuse_is_not_served() {
        let mountinfo = concat!(
            "33493 1 259:40 / / rw,... - ext2 /dev/block/dm-40 rw,...\n",
            "33494 33493 0:225 / /storage/emulated rw,... - fuse /dev/fuse rw,...\n",
        );
        assert!(!is_storage_path_served_by_srx_fuse_mountinfo_in(
            mountinfo,
            "/storage/emulated/0/Android"
        ));
    }

    // 健康坍缩、无屏障：视图根 srx FUSE 直接服务子路径。
    #[test]
    fn healthy_collapsed_view_root_serves_subpaths() {
        let mountinfo = concat!(
            "33493 1 259:40 / / rw,... - ext2 /dev/block/dm-40 rw,...\n",
            "33494 33493 0:225 / /storage/emulated rw,... - fuse /dev/fuse rw,...\n",
            "33513 33494 0:154 / /storage/emulated/0 rw,... - fuse srx_fuse_host[10062] rw,...\n",
        );
        assert!(is_storage_path_served_by_srx_fuse_mountinfo_in(
            mountinfo,
            "/storage/emulated/0/Android/media"
        ));
    }

    // 转义空格归一：挂载点带 \040 时仍能匹配。
    #[test]
    fn escaped_space_is_normalized() {
        let mountinfo = concat!(
            "33493 1 259:40 / / rw,... - ext2 /dev/block/dm-40 rw,...\n",
            "33513 33493 0:154 / /storage/emulated/0/my\\040dir rw,... - fuse srx_fuse_host[1] rw,...\n",
        );
        assert!(is_storage_path_served_by_srx_fuse_mountinfo_in(
            mountinfo,
            "/storage/emulated/0/my dir/"
        ));
    }
}
