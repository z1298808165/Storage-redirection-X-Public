use crate::config::StorageBackendMode;
use crate::domain::PathMapping;
use crate::platform::{fs, module_paths, paths};
use fuser::{MountOption, SessionACL};
use std::ffi::CString;
use std::os::unix::fs::FileTypeExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FuseCapability {
    Unknown,
    Available,
    Unavailable,
}

#[derive(Clone)]
pub struct FuseRedirectConfig {
    pub package_name: String,
    pub app_pid: i32,
    pub app_start_time_ticks: Option<u64>,
    pub uid: i32,
    pub app_data_dir: String,
    pub redirect_target: String,
    pub mount_root: Option<String>,
    pub real_root_override: Option<String>,
    pub is_file_monitor_enabled: bool,
    pub allowed_real_paths: Vec<String>,
    pub excluded_real_paths: Vec<String>,
    pub sandboxed_paths: Vec<String>,
    pub read_only_paths: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
    pub is_mapping_mode_only: bool,
}

impl FuseRedirectConfig {
    pub(super) fn user_id(&self) -> i32 {
        crate::platform::user_id_from_uid(self.uid)
    }
}

/// 挂载请求中构造 FUSE 配置与计算 scoped 挂载根所需的字段。
///
/// daemon 侧的 `MountRequest` 与 companion 侧的 `CompanionMountRequest` 各自演进，字段
/// 相同但类型不同（前者另有 `operation`），此前两处的配置构造与挂载根计算是逐字重复的。
/// 由该 trait 统一取值，两处共用下面的 [`fuse_config_from_request`] 与
/// [`scoped_fuse_mount_roots_for_request`]，避免规则字段增减时漏改一侧。
pub trait MountRequestFields {
    fn package_name(&self) -> &str;
    fn pid(&self) -> i32;
    fn uid(&self) -> i32;
    fn app_data_dir(&self) -> &str;
    fn redirect_target(&self) -> &str;
    fn is_file_monitor_enabled(&self) -> bool;
    fn storage_backend_mode(&self) -> StorageBackendMode;
    fn allowed_real_paths(&self) -> &[String];
    fn excluded_real_paths(&self) -> &[String];
    fn sandboxed_paths(&self) -> &[String];
    fn read_only_paths(&self) -> &[String];
    fn path_mappings(&self) -> &[PathMapping];
    fn is_mapping_mode_only(&self) -> bool;
}

/// 按挂载请求构造 FUSE 重定向配置。
pub fn fuse_config_from_request<R: MountRequestFields + ?Sized>(
    request: &R,
    mount_root: Option<String>,
    real_root_override: Option<String>,
) -> FuseRedirectConfig {
    FuseRedirectConfig {
        package_name: request.package_name().to_string(),
        app_pid: request.pid(),
        app_start_time_ticks: crate::platform::process_start_time_ticks(request.pid()),
        uid: request.uid(),
        app_data_dir: request.app_data_dir().to_string(),
        redirect_target: request.redirect_target().to_string(),
        mount_root,
        real_root_override,
        is_file_monitor_enabled: request.is_file_monitor_enabled(),
        allowed_real_paths: request.allowed_real_paths().to_vec(),
        excluded_real_paths: request.excluded_real_paths().to_vec(),
        sandboxed_paths: request.sandboxed_paths().to_vec(),
        read_only_paths: request.read_only_paths().to_vec(),
        path_mappings: request.path_mappings().to_vec(),
        is_mapping_mode_only: request.is_mapping_mode_only(),
    }
}

/// 计算挂载请求对应的 scoped 挂载根；未启用 FUSE daemon 重定向时返回空列表。
pub fn scoped_fuse_mount_roots_for_request<R: MountRequestFields + ?Sized>(
    request: &R,
) -> Vec<String> {
    if matches!(
        request.storage_backend_mode(),
        StorageBackendMode::Namespace
    ) {
        return Vec::new();
    }

    let backend_mode = request.storage_backend_mode();
    let capability_available = match backend_mode {
        StorageBackendMode::Fuse => fuse_first_capability_available(),
        StorageBackendMode::Auto => fuse_capability() == FuseCapability::Available,
        StorageBackendMode::Namespace => false,
    };
    if !capability_available {
        return Vec::new();
    }

    if matches!(backend_mode, StorageBackendMode::Fuse) {
        let user_id = crate::platform::user_id_from_uid(request.uid());
        return vec![paths::storage_user_root_for_user(user_id)];
    }

    scoped_mount_roots_for_hybrid_rules(
        request.uid(),
        request.allowed_real_paths(),
        request.excluded_real_paths(),
        request.sandboxed_paths(),
        request.read_only_paths(),
        request.path_mappings(),
        request.is_mapping_mode_only(),
    )
}

/// FUSE-first 只在设备暴露可读写 `/dev/fuse` 且模块具备 root 运行环境时启用。
/// 具体挂载仍由子进程验证；能力探测失败会回退到 namespace。
pub fn fuse_device_present() -> bool {
    std::fs::metadata("/dev/fuse")
        .map(|metadata| metadata.file_type().is_char_device())
        .unwrap_or(false)
}

/// 返回 daemon 最近一次记录的 FUSE 能力。
///
/// 普通应用只读取这个原子替换的快照，不直接打开 `/dev/fuse`。快照缺失时保持
/// `Unknown`，由规划层走保守的 namespace fallback 路径。
pub fn fuse_capability() -> FuseCapability {
    let Ok(content) = std::fs::read_to_string(module_paths::FUSE_CAPABILITY_FILE) else {
        return FuseCapability::Unknown;
    };
    let current_boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().to_string());
    let snapshot_boot_id = content
        .lines()
        .find_map(|line| line.strip_prefix("boot_id="))
        .map(str::trim);
    if current_boot_id.as_deref() != snapshot_boot_id {
        return FuseCapability::Unknown;
    }
    match content
        .lines()
        .find_map(|line| line.strip_prefix("state="))
        .map(str::trim)
    {
        Some("available") => FuseCapability::Available,
        Some("unavailable") => FuseCapability::Unavailable,
        _ => FuseCapability::Unknown,
    }
}

pub fn fuse_capability_as_str(capability: FuseCapability) -> &'static str {
    match capability {
        FuseCapability::Unknown => "unknown",
        FuseCapability::Available => "available",
        FuseCapability::Unavailable => "unavailable",
    }
}

/// 自动后端使用的 fallback 路径决策，应用侧与 daemon 共享同一个判断接口。
pub fn expand_mount_fallbacks_for_mode(mode: StorageBackendMode) -> bool {
    match mode {
        StorageBackendMode::Namespace => true,
        StorageBackendMode::Fuse => false,
        StorageBackendMode::Auto => fuse_capability() != FuseCapability::Available,
    }
}

/// 在实际 FUSE 启动失败后，把 namespace fallback 所需的通配规则收敛到父目录。
pub fn expand_namespace_fallback_rules(uid: i32, rules: &[String]) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut expanded = Vec::with_capacity(rules.len());
    for rule in rules {
        let trimmed = rule.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (excluded, body) = if let Some(stripped) = trimmed.strip_prefix('!') {
            (true, stripped.trim_start())
        } else {
            (false, trimmed)
        };
        let resolved = if !excluded && paths::contains_wildcards(body) {
            paths::wildcard_policy_fallback_parent(body, &storage_root)
                .unwrap_or_else(|| body.to_string())
        } else {
            body.to_string()
        };
        expanded.push(if excluded {
            format!("!{resolved}")
        } else {
            resolved
        });
    }
    paths::sort_dedup_paths_case_insensitive(&mut expanded);
    expanded
}

fn write_fuse_capability_snapshot(capability: FuseCapability, reason: &str) -> FuseCapability {
    let state = match capability {
        FuseCapability::Available => "available",
        FuseCapability::Unavailable => "unavailable",
        FuseCapability::Unknown => "unknown",
    };
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let content = format!("schema=1\nboot_id={boot_id}\nstate={state}\nreason={reason}\n");
    let path = std::path::Path::new(module_paths::FUSE_CAPABILITY_FILE);
    let temp = path.with_extension("tmp");
    if std::fs::write(&temp, content).is_ok() && std::fs::rename(&temp, path).is_ok() {
        log::info!("fuse capability snapshot state={} reason={}", state, reason);
    } else {
        let _ = std::fs::remove_file(temp);
        log::warn!(
            "fuse capability snapshot write failed state={} reason={}",
            state,
            reason
        );
    }
    capability
}

/// 在 root daemon 或 companion 挂载路径中刷新能力快照。
// quality-allow(lint-suppression): 该入口由 Android daemon 二进制调用，cdylib 目标不会直接调用。
#[allow(dead_code)]
pub fn refresh_fuse_capability_snapshot(reason: &str) -> FuseCapability {
    let capability = if fuse_first_capability_available() {
        FuseCapability::Available
    } else {
        FuseCapability::Unavailable
    };
    write_fuse_capability_snapshot(capability, reason)
}

/// 记录实际 scoped FUSE 挂载结果，失败时让后续请求立即走 namespace fallback。
pub fn record_fuse_capability_result(available: bool, reason: &str) -> FuseCapability {
    write_fuse_capability_snapshot(
        if available {
            FuseCapability::Available
        } else {
            FuseCapability::Unavailable
        },
        reason,
    )
}

fn fuse_first_capability_available() -> bool {
    if !fuse_device_present() {
        log::warn!("fuse-first capability missing /dev/fuse");
        return false;
    }
    let Ok(path) = CString::new("/dev/fuse") else {
        return false;
    };
    // SAFETY: path 指向以 NUL 结尾的固定字符串，flags 只读写设备能力探测；返回 fd 立即关闭。
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if fd < 0 {
        log::warn!("fuse-first capability cannot open /dev/fuse");
        return false;
    }
    // SAFETY: fd 来自上面的 open，且在当前线程中尚未交给其它所有者。
    unsafe { libc::close(fd) };
    true
}

pub fn mount_blocking_with_ready(
    config: FuseRedirectConfig,
    ready_sock: Option<libc::c_int>,
) -> bool {
    let app_pid = config.app_pid;
    let Some(app_start_time_ticks) = config.app_start_time_ticks else {
        log::warn!(
            "fuse redirect app identity unavailable pkg={} app_pid={}",
            config.package_name,
            app_pid
        );
        send_ready_result(ready_sock, -1);
        return false;
    };
    let package_name = config.package_name.clone();
    let user_id = config.user_id();
    let mount_point = fuse_mount_point(&config, user_id);
    let metadata_dir = mount_point_metadata_dir(&mount_point, user_id);
    if !fs::create_directory(&metadata_dir, config.uid) {
        log::error!(
            "fuse redirect mount point missing: {} metadata={}",
            mount_point,
            metadata_dir
        );
        send_ready_result(ready_sock, -1);
        return false;
    }

    let fs = match super::FuseRedirectFs::new(config) {
        Some(fs) => fs,
        None => {
            send_ready_result(ready_sock, -1);
            return false;
        }
    };
    let mut mount_options = fuser::Config::default();
    mount_options.mount_options = vec![
        MountOption::FSName("srx_fuse_redirect".to_string()),
        MountOption::Subtype("srx".to_string()),
        MountOption::RW,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::NoAtime,
        MountOption::Async,
    ];
    mount_options.acl = SessionACL::All;
    mount_options.n_threads = Some(4);
    mount_options.clone_fd = true;

    log::info!(
        "fuse redirect mount start pkg={} uid={} user={} mp={} rel={} real={} map_only={} allow={} excl={} sandbox={} ro={} map={}",
        fs.policy.package_name,
        fs.policy.uid,
        user_id,
        mount_point,
        fs.policy.mount_rel,
        fs.policy.real_root.display(),
        fs.policy.is_mapping_mode_only,
        fs.policy.allowed_real_paths.len(),
        fs.policy.excluded_real_paths.len(),
        fs.policy.sandboxed_paths.len(),
        fs.policy.read_only_paths.len(),
        fs.policy.path_mappings.len()
    );

    let background = match fuser::spawn_mount2(fs, &mount_point, &mount_options) {
        Ok(background) => background,
        Err(error) => {
            send_ready_result(ready_sock, -1);
            log::warn!(
                "fuse redirect mount failed mp={} err={}",
                mount_point,
                error
            );
            return false;
        }
    };
    send_ready_result(ready_sock, 0);

    loop {
        if background.guard.is_finished() {
            return finish_background_session(background, &mount_point, false);
        }
        if !crate::platform::is_process_instance_alive(app_pid, app_start_time_ticks) {
            log::info!(
                "fuse redirect app exited, unmount session pkg={} app_pid={} mp={}",
                package_name,
                app_pid,
                mount_point
            );
            return finish_background_session(background, &mount_point, true);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn finish_background_session(
    background: fuser::BackgroundSession,
    mount_point: &str,
    app_exited: bool,
) -> bool {
    match background.umount_and_join() {
        Ok(()) => {
            log::info!(
                "fuse redirect session ended cleanly mp={} app_exited={}",
                mount_point,
                app_exited
            );
            true
        }
        Err(error) => {
            log::warn!(
                "fuse redirect session ended with error mp={} app_exited={} err={}",
                mount_point,
                app_exited,
                error
            );
            false
        }
    }
}

pub(super) fn fuse_mount_point(config: &FuseRedirectConfig, user_id: i32) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(raw_mount_root) = config.mount_root.as_deref() else {
        return storage_root;
    };
    let mut mount_root = paths::resolve_user_path(&paths::normalize(raw_mount_root), user_id);
    if !paths::is_absolute(&mount_root) {
        mount_root = paths::normalize(&paths::join(&storage_root, &mount_root));
    }
    if paths::eq_ignore_case(&mount_root, &storage_root)
        || paths::is_child(&mount_root, &storage_root)
    {
        mount_root
    } else {
        storage_root
    }
}

fn mount_point_metadata_dir(mount_point: &str, user_id: i32) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    if paths::eq_ignore_case(mount_point, &storage_root) {
        return paths::data_media_user_root_for_user(user_id);
    }
    paths::storage_to_data_media_for_user(mount_point, user_id)
        .unwrap_or_else(|| mount_point.to_string())
}

pub fn scoped_mount_roots_for_wildcard_rules<'a>(
    uid: i32,
    rules: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut roots = Vec::new();
    for raw in rules {
        let raw = raw.trim_start();
        let raw = raw.strip_prefix('!').unwrap_or(raw).trim_start();
        let mut resolved = paths::resolve_user_path(&paths::normalize(raw), user_id);
        if resolved.is_empty()
            || paths::has_unsafe_segments(&resolved)
            || !paths::contains_wildcards(&resolved)
        {
            continue;
        }
        if !paths::is_absolute(&resolved) {
            resolved = paths::normalize(&paths::join(&storage_root, &resolved));
        }
        if !paths::is_child(&resolved, &storage_root)
            && !paths::eq_ignore_case(&resolved, &storage_root)
        {
            continue;
        }
        let prefix = paths::concrete_prefix_before_wildcard(&resolved);
        if let Some(root) = scoped_mount_root_for_wildcard_prefix(&prefix, &storage_root) {
            roots.push(root);
        }
    }
    compact_scoped_mount_roots(roots, &storage_root)
}

fn scoped_mount_root_for_wildcard_prefix(prefix: &str, storage_root: &str) -> Option<String> {
    if prefix.is_empty() || !paths::is_child(prefix, storage_root) {
        return Some(storage_root.to_string());
    }
    if let Some(root) = public_collection_mount_root(prefix, storage_root) {
        return Some(root);
    }
    Some(prefix.to_string())
}

fn public_collection_mount_root(prefix: &str, storage_root: &str) -> Option<String> {
    public_collection_name(prefix, storage_root).map(|first| paths::join(storage_root, first))
}

fn public_collection_name<'a>(prefix: &'a str, storage_root: &str) -> Option<&'a str> {
    let rel = paths::relative_child_path(prefix, storage_root)?;
    let first = rel.split('/').find(|part| !part.is_empty())?;
    match first {
        "Alarms" | "Audiobooks" | "DCIM" | "Documents" | "Download" | "Movies" | "Music"
        | "Notifications" | "Pictures" | "Podcasts" | "Recordings" | "Ringtones" => Some(first),
        _ => None,
    }
}

pub fn scoped_mount_roots_for_hybrid_rules(
    uid: i32,
    allowed_real_paths: &[String],
    excluded_real_paths: &[String],
    sandboxed_paths: &[String],
    read_only_paths: &[String],
    path_mappings: &[crate::domain::PathMapping],
    is_mapping_mode_only: bool,
) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let scoped_allowed_rules = allowed_real_paths.iter().map(String::as_str);
    let sandbox_include_rules = sandboxed_paths
        .iter()
        .filter(|rule| !rule.trim_start().starts_with('!'))
        .map(String::as_str);
    let mut roots = scoped_mount_roots_for_wildcard_rules(
        uid,
        scoped_allowed_rules
            .chain(excluded_real_paths.iter().map(String::as_str))
            .chain(sandbox_include_rules)
            .chain(read_only_paths.iter().map(String::as_str)),
    );

    if is_mapping_mode_only {
        for sandboxed_path in sandboxed_paths {
            if sandboxed_path.trim_start().starts_with('!') {
                continue;
            }
            let sandboxed_root =
                resolve_concrete_scoped_rule_parent(sandboxed_path, user_id, &storage_root);
            if !sandboxed_root.is_empty() {
                roots.push(sandboxed_root);
            }
        }
    }

    for allowed_path in allowed_real_paths {
        // 放行规则以自身为 scoped 根，而不是父目录。取父目录会带来两个问题：顶层规则
        // （如 DCIM、Pictures）的父目录就是存储根，会让整个存储被 FUSE 接管并在压缩时
        // 吞并其余更精确的根；而 Download/SrtMonitor 这类规则取到的 Download 也会吞并
        // 同配置下的兄弟目录（如只读的 Download/SrtMonitorLocked），使其拿不到独立挂载点。
        // 以规则自身为根即可覆盖该目录下的放行需求，且与兄弟根共存。
        // mapping_mode_only 的 sandboxed 分支仍沿用父目录，其语义要求按父目录整体接管。
        let allowed_root = resolve_scoped_rule_path(allowed_path, user_id, &storage_root);
        if allowed_root.is_empty()
            || paths::contains_wildcards(&allowed_root)
            || paths::eq_ignore_case(&allowed_root, &storage_root)
            || !paths::is_child(&allowed_root, &storage_root)
        {
            continue;
        }
        roots.push(allowed_root);
    }

    let normalized_read_only_paths = super::normalize_rule_list(read_only_paths.to_vec(), user_id);
    let (read_only_includes, read_only_excludes) =
        paths::split_exclusion_rules(&normalized_read_only_paths);
    let read_only_excludes =
        paths::overlapping_exclusion_rules(&read_only_includes, &read_only_excludes);
    let scoped_path_mappings = resolve_scoped_path_mappings(path_mappings, user_id, &storage_root);
    for read_only_root in &read_only_includes {
        if paths::contains_wildcards(read_only_root) {
            continue;
        }
        if read_only_excludes.iter().any(|excluded| {
            !paths::contains_wildcards(excluded) && paths::is_child(excluded, read_only_root)
        }) || scoped_path_mappings
            .iter()
            .any(|(request_path, final_path)| {
                (!paths::contains_wildcards(request_path)
                    && paths::is_child(request_path, read_only_root))
                    || (!paths::contains_wildcards(final_path)
                        && paths::is_child(final_path, read_only_root))
            })
        {
            roots.push(read_only_root.clone());
        }
    }

    // 挂载根的三级降级（去重剔子路径、退化顶层、退化整个存储根）只输出最终结果，
    // 一旦降级就看不出是哪条规则贡献了多余的根。这里在压缩前后各记录一次：该函数只在
    // 应用挂载时执行一次，频率极低。
    log::info!(
        "scoped roots raw count={} list={}",
        roots.len(),
        roots.join(",")
    );
    let compacted = compact_scoped_mount_roots(roots, &storage_root);
    log::info!(
        "scoped roots compacted count={} list={}",
        compacted.len(),
        compacted.join(",")
    );
    compacted
}

fn resolve_scoped_path_mappings(
    path_mappings: &[crate::domain::PathMapping],
    user_id: i32,
    storage_root: &str,
) -> Vec<(String, String)> {
    let mut resolved = Vec::with_capacity(path_mappings.len());
    for mapping in path_mappings {
        let request_path = resolve_scoped_rule_path(&mapping.request_path, user_id, storage_root);
        let final_path = resolve_scoped_rule_path(&mapping.final_path, user_id, storage_root);
        if request_path.is_empty()
            || final_path.is_empty()
            || paths::eq_ignore_case(&request_path, &final_path)
            || !paths::is_same_or_child(&request_path, storage_root)
            || !paths::is_same_or_child(&final_path, storage_root)
        {
            continue;
        }
        resolved.push((request_path, final_path));
    }
    resolved
}

fn resolve_concrete_scoped_rule_parent(path: &str, user_id: i32, storage_root: &str) -> String {
    let resolved = resolve_scoped_rule_path(path, user_id, storage_root);
    if resolved.is_empty()
        || paths::contains_wildcards(&resolved)
        || paths::eq_ignore_case(&resolved, storage_root)
    {
        return String::new();
    }

    let parent = paths::parent(&resolved);
    if paths::eq_ignore_case(&parent, storage_root) || paths::is_child(&parent, storage_root) {
        parent
    } else {
        String::new()
    }
}

fn resolve_scoped_rule_path(path: &str, user_id: i32, storage_root: &str) -> String {
    let mut resolved = paths::resolve_user_path(&paths::normalize(path), user_id);
    if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
        return String::new();
    }
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    }
    if !paths::is_child(&resolved, storage_root) && !paths::eq_ignore_case(&resolved, storage_root)
    {
        return String::new();
    }
    resolved
}

fn compact_scoped_mount_roots(mut roots: Vec<String>, storage_root: &str) -> Vec<String> {
    paths::sort_dedup_paths_case_insensitive(&mut roots);
    roots.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    let mut effective: Vec<String> = Vec::new();
    for root in roots {
        if effective
            .iter()
            .any(|kept| paths::eq_ignore_case(kept, &root) || paths::is_child(&root, kept))
        {
            continue;
        }
        effective.push(root);
    }

    if effective.len() <= super::MAX_SCOPED_FUSE_ROOTS {
        return effective;
    }

    // 第二级降级：把各根收敛到其所属的顶层存储子目录（如 Download、DCIM）。
    // 收敛不出顶层子目录（即该根本身就是存储根）的情况直接丢弃，不能让它把
    // 整个存储根带进结果——那等于让模块内 FUSE 接管全部共享存储。
    let mut top_level: Vec<String> = effective
        .iter()
        .filter_map(|root| top_level_storage_child(root, storage_root))
        .collect();
    paths::sort_dedup_paths_case_insensitive(&mut top_level);
    if !top_level.is_empty() && top_level.len() <= super::MAX_SCOPED_FUSE_ROOTS {
        return top_level;
    }

    // 顶层降级后仍超限：放弃 scoped FUSE，返回空列表让调用方走 mount namespace。
    //
    // 此前这里退化为整个存储根 `/storage/emulated/<user>`，即模块内 FUSE 提供全部
    // 共享存储。这偏离了「只在通配规则的最小具体父目录挂载 FUSE」的设计前提：真机
    // 实测中 7 个顶层目录规则就会触发该退化（scoped roots raw count=7 → compacted
    // count=1 list=/storage/emulated/0），属常见配置而非极端情况；而接管整个存储的
    // 路径缺少验证，且历史上无条件启用 MediaProvider native FUSE 曾导致
    // Android 13 出现 Transport endpoint is not connected。
    //
    // mount namespace 方案会让通配规则退化为按已存在目录匹配，功能上弱于 FUSE，
    // 但作用范围可控，比接管整个存储更安全。
    log::warn!(
        "scoped roots exceed limit after top-level fallback: effective={} top_level={} limit={}, \
         skip scoped fuse and use mount namespace",
        effective.len(),
        top_level.len(),
        super::MAX_SCOPED_FUSE_ROOTS
    );
    Vec::new()
}

fn top_level_storage_child(path: &str, storage_root: &str) -> Option<String> {
    if paths::eq_ignore_case(path, storage_root) {
        return None;
    }
    let rel = paths::relative_child_path(path, storage_root)?;
    let first = rel.split('/').find(|part| !part.is_empty())?;
    Some(paths::join(storage_root, first))
}

fn send_ready_result(sock: Option<libc::c_int>, result: i32) {
    let Some(sock) = sock else {
        return;
    };
    // SAFETY: sock 是有效的 socket fd，buffer 指针指向栈上有效数据，size 与类型匹配，调用期间保持有效。
    let _ = unsafe {
        libc::send(
            sock,
            &result as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    // SAFETY: sock 是有效的 socket fd，此处是唯一的关闭点，调用后不再使用。
    unsafe { libc::close(sock) };
}
