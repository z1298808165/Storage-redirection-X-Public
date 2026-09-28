// 挂载诊断与快照。
//
// 这里只做「读取事实并打印/记录」：跨层身份快照、doctor 报告、挂载目标视图日志、
// 子进程卡死时的 /proc 取证。它们不参与挂载决策，也不持有任何状态；
// 拆出来之后主流程文件只剩决策与执行，排查逻辑的改动不会碰状态机。

use crate::daemon_mount::{
    MountOperation, MountRequest, current_mount_target_count, module_mapped_into,
    mount_point_covers, package_processes, read_trimmed,
};
use crate::fuse_supervisor;
use crate::mount_identity::{self, MountLedger, MountVerdict};
use crate::platform::paths::monotonic_ms;
use crate::platform::unique_fd::UniqueFd;
use crate::platform::{module_paths, paths};
use libc::{O_CLOEXEC, O_RDONLY, c_void, open};
use std::ffi::CString;

pub(crate) fn log_child_diagnostics(child: i32, phase: &str) {
    let wchan = read_proc_text(&format!("/proc/{}/wchan", child))
        .unwrap_or_else(|| "<unavailable>".to_string());
    let status_summary = read_proc_status_summary(&format!("/proc/{}/status", child))
        .unwrap_or_else(|| "<unavailable>".to_string());
    let stack = read_proc_text(&format!("/proc/{}/stack", child))
        .unwrap_or_else(|| "<unavailable>".to_string());

    log::warn!(
        "daemon child stuck child={} phase={} wchan={} status={}",
        child,
        phase,
        wchan.trim(),
        status_summary
    );
    let stack_trimmed = stack.trim();
    if !stack_trimmed.is_empty() && stack_trimmed != "<unavailable>" {
        log::warn!(
            "daemon child stuck child={} phase={} stack:\n{}",
            child,
            phase,
            stack_trimmed
        );
    }
}

pub(crate) fn read_proc_text(path: &str) -> Option<String> {
    let Ok(c_path) = CString::new(path) else {
        return None;
    };
    // SAFETY: c_path 在本作用域内存活且以 NUL 结尾；fd 立即交给 UniqueFd 托管关闭。
    let fd = unsafe { open(c_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let file = UniqueFd::new(fd);
    let mut text = String::new();
    let mut buf = [0u8; 1024];
    loop {
        // SAFETY: buf 由本作用域独占借用且长度以 buf.len() 传入，file 持有有效 fd。
        let n = unsafe { libc::read(file.get(), buf.as_mut_ptr() as *mut c_void, buf.len()) };
        if n <= 0 {
            break;
        }
        let Ok(s) = std::str::from_utf8(&buf[..n as usize]) else {
            break;
        };
        text.push_str(s);
        if text.len() >= 8192 {
            break;
        }
    }
    Some(text)
}

pub(crate) fn read_proc_status_summary(path: &str) -> Option<String> {
    let raw = read_proc_text(path)?;
    let mut name = String::from("?");
    let mut state = String::from("?");
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("Name:") {
            name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("State:") {
            state = rest.trim().to_string();
        }
    }
    Some(format!("name={} state={}", name, state))
}

/// 跨层身份快照：把「这是哪个应用」的五个口径并列输出。
///
/// 项目里每一层用不同口径回答同一个问题——zygisk 看进程、Java hook 看调用方 uid、native 看
/// caller uid、FUSE 会话看会话创建时绑定的应用、namespace 看 `ns(dev:ino)+start_time`。失败几乎
/// 都出在层间口径不一致处，而此前只有挂载账本可查，其余各层要逐条 adb 命令拼。把五层一次列清
/// 可以让排查从「猜」变成「读表」。
fn print_cross_layer_snapshot(package_name: &str, path: Option<&str>) {
    println!("== module ==");
    let prop = std::fs::read_to_string(format!("{}/module.prop", module_paths::MODULE_DIR))
        .unwrap_or_default();
    let version = prop
        .lines()
        .find_map(|line| line.strip_prefix("version="))
        .unwrap_or("-");
    let zygisk_lib = format!("{}/zygisk/arm64-v8a.so", module_paths::MODULE_DIR);
    println!(
        "version={} boot_ok={} runtime_disabled={} zygisk_lib_bytes={}",
        version,
        read_trimmed(&format!("{}/.boot_ok", module_paths::MODULE_DIR))
            .unwrap_or_else(|| "-".to_string()),
        if std::path::Path::new(module_paths::RUNTIME_DISABLE_FILE).exists() {
            "yes"
        } else {
            "no"
        },
        std::fs::metadata(&zygisk_lib)
            .map(|meta| meta.len())
            .unwrap_or(0)
    );

    println!("== capability ==");
    let summary = crate::fuse_redirect::config::fuse_capability_summary();
    let now_ms = monotonic_ms().max(0) as u64;
    println!(
        "state={} device_fail={} tracked_scopes={} backoff_step={} teardown_fail={} retry_at_ms={} retry_in_ms={}",
        crate::fuse_redirect::config::fuse_capability_as_str(summary.capability),
        summary.device_failures,
        summary.scope_failures.len(),
        summary.backoff_step,
        summary.teardown_failures,
        summary.retry_at_ms,
        summary.retry_at_ms.saturating_sub(now_ms)
    );
    for (scope, count) in &summary.scope_failures {
        println!("  scope_fail={scope}:{count}");
    }
    let allowed_auto = crate::fuse_redirect::config::scoped_mount_allowed_for_scope(
        package_name,
        crate::config::StorageBackendMode::Auto,
    );
    println!(
        "gate pkg={} mode=auto allowed={} note=实际模式以 app_config 的字段为准",
        package_name, allowed_auto
    );

    println!("== java_hook ==");
    println!(
        "install_state={} deferred_marker={} hot_reload_request_pending={}",
        read_trimmed(module_paths::MEDIA_HOOK_INSTALL_STATE_FILE)
            .unwrap_or_else(|| "-".to_string()),
        read_trimmed(module_paths::MEDIA_HOOK_DEFERRED_FILE).unwrap_or_else(|| "-".to_string()),
        if std::path::Path::new(module_paths::MEDIA_PROVIDER_HOT_RELOAD_REQUEST_FILE).exists() {
            "yes"
        } else {
            "no"
        }
    );

    println!("== app_config ==");
    let config_path = format!("{}/apps/{}.json", module_paths::CONFIG_DIR, package_name);
    match std::fs::read_to_string(&config_path) {
        Ok(content) => println!(
            "path={} bytes={} content={}",
            config_path,
            content.len(),
            content.trim()
        ),
        Err(_) => println!("path={} present=false", config_path),
    }

    println!("== processes ==");
    let processes = package_processes(package_name);
    if processes.is_empty() {
        println!("none");
    }
    for (pid, cmdline) in &processes {
        println!(
            "pid={} cmdline={} module_mapped_now={} start_ticks={}",
            pid,
            cmdline,
            module_mapped_into(*pid)
                .map(|mapped| mapped.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            crate::platform::process_start_time_ticks(*pid).unwrap_or_default()
        );
    }
    println!(
        "note: module_mapped_now=false 是常见现象——注入完成后模块会 dlclose 自己，启动之后再查 maps 查不到不能据此判定「没被注入」；该结论要看 install_state 与下面的 ledger。"
    );

    println!("== app_runtime_state ==");
    let prefix = format!("{package_name}_");
    for dir in [
        module_paths::MOUNT_STATE_DIR,
        module_paths::MOUNT_INTENT_DIR,
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(prefix.as_str()) {
                continue;
            }
            let path = entry.path();
            println!(
                "{}={}",
                path.display(),
                read_trimmed(&path.display().to_string()).unwrap_or_default()
            );
        }
    }

    let Some(path) = path else {
        return;
    };
    println!("== path ==");
    let user_id = paths::extract_user_id_from_storage_path(path);
    println!("input={} user={}", path, user_id);
    match paths::storage_to_data_media_for_user(path, user_id) {
        Some(backend) => println!("backend={}", backend),
        None => println!("backend=- (不是 /storage/emulated/<user>/... 形态)"),
    }
    println!("note: backend 是绕过重定向层看到的真实落点；是否被重定向看下面的 ledger_mount。");
    for ledger in mount_identity::list_ledgers() {
        if ledger.package_name != package_name {
            continue;
        }
        for mount in &ledger.mounts {
            println!(
                "ledger_mount point={} mount_id={} covers_input={}",
                mount.mount_point,
                mount.mount_id,
                mount_point_covers(&mount.mount_point, path)
            );
        }
    }
}

/// 这是一个独立进程入口，不共享 daemon 进程内的监督计数，因此只报告磁盘上可观察的事实：
/// 账本记录的挂载身份、目标进程是否仍是同一实例、命名空间是否被替换、以及每个挂载点当前
/// 的端点健康。用于回答"daemon 认为它挂了什么、那些挂载现在还活着吗"。
///
/// 用法：
/// - `srx_daemon doctor`：报告挂载账本总体健康；
/// - `srx_daemon doctor <包名> [路径]`：先输出该包的跨层身份快照，再只报告该包的账本。
pub fn doctor_report(args: &[String]) -> i32 {
    let package_name = args
        .first()
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    let path = args
        .get(1)
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    if let Some(package_name) = package_name {
        print_cross_layer_snapshot(package_name, path);
    }

    let ledgers: Vec<MountLedger> = match package_name {
        Some(package_name) => mount_identity::list_ledgers()
            .into_iter()
            .filter(|ledger| ledger.package_name == package_name)
            .collect(),
        None => mount_identity::list_ledgers(),
    };
    let mut unhealthy = 0usize;
    println!(
        "mount identity ledger entries={} dir={}",
        ledgers.len(),
        module_paths::MOUNT_STATE_DIR
    );
    for ledger in &ledgers {
        let current_namespace = mount_identity::namespace_identity(ledger.target_pid);
        let target_is_current = ledger.target_is_current();
        let namespace_state = match current_namespace {
            Some(namespace) if namespace == ledger.namespace => "current",
            Some(_) => "replaced",
            None => "unavailable",
        };
        println!(
            "ledger pkg={} pid={} generation={} target={} namespace={} poisoned={} detach_attempts={}",
            ledger.package_name,
            ledger.target_pid,
            ledger.generation,
            if target_is_current { "alive" } else { "gone" },
            namespace_state,
            ledger.is_poisoned(),
            ledger.detach_attempts
        );
        for mount in &ledger.mounts {
            let health = fuse_supervisor::probe_endpoint(ledger.target_pid, &mount.mount_point);
            if health.is_dead_connection() {
                unhealthy = unhealthy.saturating_add(1);
            }
            let live = mount_identity::topmost_live_mount(ledger.target_pid, &mount.mount_point);
            let verdict = mount_identity::classify_mount(
                ledger,
                &mount.mount_point,
                live.as_ref(),
                current_namespace,
                target_is_current,
            );
            println!(
                "  mount point={} recorded_id={} live_id={} health={} verdict={}",
                mount.mount_point,
                mount.mount_id,
                live.as_ref()
                    .map(|live| live.mount_id.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                health.as_str(),
                match verdict {
                    MountVerdict::Owned(_) => "owned",
                    MountVerdict::Detached => "detached",
                    MountVerdict::Superseded(_) => "superseded",
                    MountVerdict::StaleNamespace => "stale_namespace",
                }
            );
        }
    }
    println!(
        "supervisor {}",
        fuse_supervisor::SupervisorSummary::snapshot().render()
    );
    if unhealthy > 0 {
        println!("result unhealthy_mounts={}", unhealthy);
        return 1;
    }
    println!("result ok");
    0
}

/// 在目标进程的挂载命名空间内核对本轮记录的目标是否可解析、是否真的挂上了。
///
/// 调用点位于已经 `setns` 到应用命名空间的挂载子进程里，因此 `metadata` 与
/// `/proc/self/mountinfo` 反映的都是**应用自己的视图**，而不是守护进程的视图。热重载类
/// 问题需要这条记录才能把两种情况分开：绑定根本没在应用视图里生效，还是生效之后又被
/// 后续请求摘掉——后者会在下一次清理里留下 `daemon unmount ok` 记录，两条对照即可定位。
pub(crate) fn log_mounted_target_view(targets: &[String], request: &MountRequest) {
    if targets.is_empty() {
        return;
    }
    // 诊断：热重载后出现「子进程 mount 返回 0，但同 ns 的 /proc/<pid>/mountinfo 与
    // 事后采集都看不到新挂载」的矛盾。这里一次性自证三件事——本进程当前 ns 的
    // inode、目标应用进程 ns 的 inode、以及本进程 mountinfo 里映射目标的原始行，
    // 用于判定 setns 目标错误、ns 内被二次摘除、还是字符串形态不匹配。
    let self_ns = std::fs::read_link("/proc/self/ns/mnt")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unreadable".to_string());
    let app_ns = std::fs::read_link(format!("/proc/{}/ns/mnt", request.pid))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unreadable".to_string());
    let diag_lines = std::fs::read_to_string("/proc/self/mountinfo")
        .map(|content| {
            let matched: Vec<&str> = content
                .lines()
                .filter(|line| line.contains("SrtProbe"))
                .collect();
            if matched.is_empty() {
                "<none>".to_string()
            } else {
                matched.join(" | ")
            }
        })
        .unwrap_or_else(|_| "<read failed>".to_string());
    // 同时记录目标应用进程视角的映射条目：`/proc/<pid>/mountinfo` 由内核按 pid
    // 所属命名空间实时渲染，与 `read_link(ns/mnt)` 的 inode 对比互为印证，可区分
    // 「读到的就是应用的 ns 但条目确实缺失」与「ns 不一致」两种情形。
    let app_diag_lines = std::fs::read_to_string(format!("/proc/{}/mountinfo", request.pid))
        .map(|content| {
            let matched: Vec<&str> = content
                .lines()
                .filter(|line| line.contains("SrtProbe"))
                .collect();
            if matched.is_empty() {
                "<none>".to_string()
            } else {
                matched.join(" | ")
            }
        })
        .unwrap_or_else(|_| "<read failed>".to_string());
    log::warn!(
        "mount view diag self_ns={} app_ns={} app_pid={} srtprobe_lines={} app_srtprobe_lines={}",
        self_ns,
        app_ns,
        request.pid,
        diag_lines,
        app_diag_lines
    );
    let mut unreadable = 0usize;
    let mut not_mounted = 0usize;
    for target in targets {
        if std::fs::metadata(target).is_err() {
            unreadable += 1;
            log::warn!("daemon target view unreadable target={}", target);
            continue;
        }
        if current_mount_target_count(target) == 0 {
            not_mounted += 1;
            log::warn!("daemon target view not mounted target={}", target);
        }
    }
    log::info!(
        "daemon target view checked={} unreadable={} not_mounted={}",
        targets.len(),
        unreadable,
        not_mounted
    );
}

/// 热重载（`Reload`）前后各采一次关键挂载点的最上层挂载，直接回答「视图根/映射目标最上层
/// 到底是 MediaProvider FUSE 层、模块 ext4 bind、还是被摘空」。
///
/// 此前只有成功路径采 `hot_view_root_layers`（视图根）与 [`log_mounted_target_view`]（映射
/// 目标），失败侧既有进程重载后的挂载栈只能靠 `app_status.log` 里的旧快照反推，无法定论。
/// 本函数在重载**摘除前**与**重建后**各跑一次，用 `topmost_live_mount(0, …)` 读当前进程（已
/// `setns` 进应用命名空间）的 `/proc/self/mountinfo`，`fs_type` 直接区分 FUSE 与 ext4。
pub(crate) fn log_reload_view_root_stack(request: &MountRequest, phase: &str) {
    if request.operation != MountOperation::Reload {
        return;
    }
    let user_id = crate::platform::user_id_from_uid(request.uid);
    let view_root = paths::storage_user_root_for_user(user_id);
    let mut points = Vec::with_capacity(1 + request.path_mappings.len());
    points.push(view_root);
    // 映射的显示请求路径（`/storage/emulated/<user>/<request_path>`）是否还在、承载它的最上层
    // 是什么，是区分「整块重定向失效」与「只有映射目标失效」的关键。
    for mapping in &request.path_mappings {
        let target = format!(
            "{}/{}",
            paths::storage_user_root_for_user(user_id),
            mapping.request_path
        );
        points.push(target);
    }
    for point in points {
        match mount_identity::topmost_live_mount(0, &point) {
            Some(mount) => log::info!(
                "reload view root stack phase={} pid={} pkg={} target={} top_source={} top_fs={} top_root={} mount_id={}",
                phase,
                request.pid,
                request.package_name,
                point,
                mount.source,
                mount.fs_type,
                mount.root,
                mount.mount_id
            ),
            None => log::info!(
                "reload view root stack phase={} pid={} pkg={} target={} top=none",
                phase,
                request.pid,
                request.package_name,
                point
            ),
        }
    }
}
