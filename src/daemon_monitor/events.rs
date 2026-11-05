use super::inotify::{cstring_path, last_errno};
use crate::config::SettingsHub;
use crate::monitor::{infer_public_path_package_name, infer_recent_path_caller_identity};
use crate::platform::paths;
use crate::redirect::policy;
use libc::{
    IN_ATTRIB, IN_CLOSE_WRITE, IN_DELETE, IN_MODIFY, IN_MOVED_FROM, IN_MOVED_TO, mode_t, time, tm,
};
use std::os::unix::fs::MetadataExt;

const LOGCAT_OP_TAG: &str = "FileMonitorOp";
const ANDROID_APP_UID_START: i32 = 10000;
const MEDIA_RW_UID: u32 = 1023;
const MEDIA_RW_GID: u32 = 1023;
// 目录模式只在 `platform::fs` 定义一次，避免与挂载侧副本各自漂移。
use crate::platform::fs::REAL_PUBLIC_DIR_MODE;

pub(super) struct MonitorIdentity {
    pub(super) package_name: String,
    pub(super) identify_method: &'static str,
    pub(super) identify_reliability: &'static str,
    pub(super) correlation_id: String,
}

pub(super) struct MonitorEventPaths {
    pub(super) backend_path: String,
    pub(super) display_path: String,
    pub(super) from_path: String,
}

pub(super) fn should_skip_ambiguous_allowed_real_path_event(
    identity: &MonitorIdentity,
    source: &str,
    _display_path: &str,
    watch_package_name: &str,
) -> bool {
    if source != "allowed_real_path" {
        return false;
    }

    if is_media_provider_fallback_identity(identity) {
        return false;
    }

    if identity.identify_method == "public_path_package"
        || is_exact_provider_caller_identity(identity)
    {
        return false;
    }

    if should_keep_system_intermediate_owner_identity(identity, source, watch_package_name) {
        return false;
    }

    if identity.package_name != watch_package_name {
        return true;
    }

    !matches!(identity.identify_method, "owner_uid" | "path_owner")
}

pub(super) fn should_skip_ambiguous_read_only_path_event(
    identity: &MonitorIdentity,
    source: &str,
    watch_package_name: &str,
) -> bool {
    if source != "read_only_path" {
        return false;
    }

    if is_media_provider_fallback_identity(identity) {
        return false;
    }

    if identity.identify_method == "public_path_package"
        || is_exact_provider_caller_identity(identity)
    {
        return false;
    }

    if should_keep_system_intermediate_owner_identity(identity, source, watch_package_name) {
        return false;
    }

    if identity.package_name != watch_package_name {
        return true;
    }

    !matches!(identity.identify_method, "owner_uid" | "path_owner")
}

pub(super) fn should_skip_public_root_event_identity(
    identity: &MonitorIdentity,
    source: &str,
    watch_package_name: &str,
) -> bool {
    if is_media_provider_fallback_identity(identity) {
        return false;
    }

    if identity.identify_method == "public_path_package"
        || is_exact_provider_caller_identity(identity)
    {
        return false;
    }

    if should_keep_system_intermediate_owner_identity(identity, source, watch_package_name) {
        return false;
    }

    source == "public_root"
        && !is_exact_provider_caller_identity(identity)
        && (identity.identify_method != "owner_uid" || identity.package_name != watch_package_name)
}

fn is_exact_provider_caller_identity(identity: &MonitorIdentity) -> bool {
    matches!(
        identity.identify_method,
        "provider_open" | "provider_mkdir" | "saf_provider"
    ) && identity.identify_reliability == "high"
        && !identity.correlation_id.is_empty()
}

pub(super) fn emit_monitor_event(
    identity: &MonitorIdentity,
    paths: &MonitorEventPaths,
    watch_package_name: &str,
    source: &str,
    mask: u32,
    operation_name: &str,
) {
    if identity.package_name.is_empty() || paths.display_path.is_empty() {
        return;
    }

    let event_kind = if operation_name.starts_with("open") {
        "OPEN"
    } else if operation_name.starts_with("delete") {
        "DELETE"
    } else if operation_name.starts_with("rename") {
        "RENAME"
    } else if operation_name.starts_with("attrib") {
        "ATTRIB"
    } else {
        "CREATE"
    };
    // 短窗去重：同一物理写入会被多条 watch 链路（映射源 watch 与公共根 watch）
    // 各上报一条观察记录，backend 相同、from 不同；窗口内只保留首条。
    if crate::platform::paths::should_suppress_duplicate_monitor_record(
        "observation",
        event_kind,
        &paths.backend_path,
        identity.package_name.as_str(),
    ) {
        return;
    }
    let mut line = format!(
        "{}|{}|{}|{}|{}|ret=0|errno=0|identify_method={}|identify_reliability={}|event_role=observation|op={}|source={}|mask=0x{:x}|backend={}",
        build_timestamp(),
        identity.package_name,
        identity.package_name,
        event_kind,
        paths.display_path,
        identity.identify_method,
        identity.identify_reliability,
        operation_name,
        source,
        mask,
        paths.backend_path
    );
    if watch_package_name != identity.package_name {
        line.push_str("|watch_package=");
        line.push_str(watch_package_name);
    }
    if !identity.correlation_id.is_empty() {
        line.push_str("|correlation_id=");
        line.push_str(&identity.correlation_id);
    }
    line.push_str("|observer_package=srx_daemon");
    if !paths.from_path.is_empty() && paths.from_path != paths.display_path {
        line.push_str("|from=");
        line.push_str(&paths.from_path);
    }
    log::info!(target: LOGCAT_OP_TAG, "{}", line);
}

pub(super) fn resolve_monitor_identity(
    watch_package_name: &str,
    display_path: &str,
    backend_path: &str,
    source: &str,
) -> MonitorIdentity {
    let owner = paths::extract_android_private_path_owner(display_path);
    if !owner.is_empty() {
        return MonitorIdentity {
            package_name: owner,
            identify_method: "path_owner",
            identify_reliability: "high",
            correlation_id: String::new(),
        };
    }

    if let Some(identity) = resolve_recent_path_caller_identity(display_path, source) {
        return identity;
    }

    if let Some(package_name) = resolve_package_by_owner_uid(backend_path) {
        if should_prefer_watch_package_for_system_writer_owner(source, &package_name) {
            return watch_package_identity(watch_package_name, "watch_package", "medium");
        }
        return MonitorIdentity {
            package_name,
            identify_method: "owner_uid",
            identify_reliability: "high",
            correlation_id: String::new(),
        };
    }

    let user_id = paths::extract_user_id_from_storage_path(display_path);
    if let Some(package_name) = infer_public_path_package_name(display_path, user_id) {
        return MonitorIdentity {
            package_name,
            identify_method: "public_path_package",
            identify_reliability: "high",
            correlation_id: String::new(),
        };
    }

    if should_use_media_provider_fallback_identity(source, backend_path) {
        return media_provider_fallback_identity();
    }

    watch_package_identity(watch_package_name, "daemon_inotify", "medium")
}

fn resolve_recent_path_caller_identity(
    display_path: &str,
    source: &str,
) -> Option<MonitorIdentity> {
    if !matches!(
        source,
        "allowed_real_path" | "read_only_path" | "public_root"
    ) {
        return None;
    }

    let user_id = paths::extract_user_id_from_storage_path(display_path);
    let identity = infer_recent_path_caller_identity(display_path, user_id)?;
    if identity.confidence != "high"
        || !matches!(
            identity.source,
            "provider_open" | "provider_mkdir" | "saf_provider"
        )
        || identity.package_name.is_empty()
        || policy::is_attribution_intermediate_package(&identity.package_name)
        || policy::is_system_writer_package(&identity.package_name)
    {
        return None;
    }

    Some(MonitorIdentity {
        package_name: identity.package_name,
        identify_method: identity.source,
        identify_reliability: identity.confidence,
        correlation_id: identity.correlation_id,
    })
}

fn watch_package_identity(
    watch_package_name: &str,
    identify_method: &'static str,
    identify_reliability: &'static str,
) -> MonitorIdentity {
    MonitorIdentity {
        package_name: watch_package_name.to_string(),
        identify_method,
        identify_reliability,
        correlation_id: String::new(),
    }
}

fn resolve_package_by_owner_uid(path: &str) -> Option<String> {
    let uid = std::fs::metadata(path).ok()?.uid() as i32;
    if uid < ANDROID_APP_UID_START {
        return None;
    }

    let mut packages = policy::get_packages_for_uid(uid);
    if packages.is_empty() {
        policy::refresh_shared_uid_cache();
        packages = policy::get_packages_for_uid(uid);
    }
    if packages.len() == 1 {
        packages.into_iter().next()
    } else {
        None
    }
}

fn should_use_media_provider_fallback_identity(source: &str, backend_path: &str) -> bool {
    if !matches!(
        source,
        "allowed_real_path" | "read_only_path" | "public_root"
    ) {
        return false;
    }

    let Ok(metadata) = std::fs::metadata(backend_path) else {
        return false;
    };
    let uid = metadata.uid();
    let gid = metadata.gid();
    uid < ANDROID_APP_UID_START as u32 || gid == MEDIA_RW_GID
}

fn media_provider_fallback_identity() -> MonitorIdentity {
    MonitorIdentity {
        package_name: policy::media_provider_record_package(),
        identify_method: "media_provider_fallback",
        identify_reliability: "fallback",
        correlation_id: String::new(),
    }
}

fn is_media_provider_fallback_identity(identity: &MonitorIdentity) -> bool {
    identity.identify_method == "media_provider_fallback"
        && policy::is_media_provider_package(&identity.package_name)
}

fn should_keep_system_intermediate_owner_identity(
    identity: &MonitorIdentity,
    source: &str,
    watch_package_name: &str,
) -> bool {
    matches!(
        source,
        "allowed_real_path" | "read_only_path" | "public_root"
    ) && identity.package_name != watch_package_name
        && matches!(identity.identify_method, "owner_uid" | "path_owner")
        && !policy::is_media_provider_package(&identity.package_name)
        && policy::is_attribution_intermediate_package(&identity.package_name)
}

pub(super) fn should_prefer_watch_package_for_system_writer_owner(
    source: &str,
    owner_package: &str,
) -> bool {
    matches!(source, "path_mapping" | "sandbox_path" | "redirect_root")
        && (policy::is_system_writer_package(owner_package)
            || policy::is_attribution_intermediate_package(owner_package))
}

pub(super) fn repair_monitored_backend_owner(
    source: &str,
    _watch_package_name: &str,
    display_path: &str,
    backend_path: &str,
) {
    if !crate::metadata_repair::enabled() {
        return;
    }
    // 私有树属主修复链已随根因修复（CE 窗口公共归一误判门控）移除：
    // 解锁后目录名明文可辨，MP 原生所有权管理即可维持私有树属主。
    if source == "public_owner" {
        repair_public_directory_owner(display_path, backend_path);
    }
}

fn repair_public_directory_owner(display_path: &str, backend_path: &str) {
    // 与周期扫描同源的 CE 防护：解锁窗口内的目录名是密文，任何公共归一都不执行。
    if !crate::daemon::media_ce_storage_ready() {
        return;
    }
    let normalized = paths::normalize(display_path);
    let user_id = paths::extract_user_id_from_storage_path(&normalized);
    if user_id < 0 {
        return;
    }
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(&normalized, &storage_root) else {
        return;
    };
    if relative.is_empty() || is_android_app_private_relative_path(relative) {
        return;
    }
    let expected_backend = paths::storage_to_data_media_path(&normalized);
    if !paths::eq_ignore_case(&expected_backend, backend_path) {
        return;
    }

    let Some(c_path) = cstring_path(backend_path) else {
        return;
    };
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: c_path 以 NUL 结尾，st 指向可供 lstat 写入的存储空间。
    if unsafe { libc::lstat(c_path.as_ptr(), st.as_mut_ptr()) } != 0 {
        return;
    }
    // SAFETY: lstat 已成功返回并完整初始化 stat 值。
    let st = unsafe { st.assume_init() };
    if st.st_mode & libc::S_IFMT as mode_t != libc::S_IFDIR as mode_t {
        return;
    }

    if st.st_uid != MEDIA_RW_UID || st.st_gid != MEDIA_RW_GID {
        // SAFETY: c_path 在调用期间保持有效，并指向已验证的公共目录。
        let ret = unsafe { libc::lchown(c_path.as_ptr(), MEDIA_RW_UID, MEDIA_RW_GID) };
        if ret != 0 {
            log::warn!(
                "daemon public owner fix failed path={} errno={}",
                backend_path,
                last_errno()
            );
            return;
        }
        log::info!("daemon public owner fix path={}", backend_path);
    }

    let current_mode = st.st_mode & 0o7777;
    if current_mode != REAL_PUBLIC_DIR_MODE {
        // SAFETY: c_path 在调用期间保持有效，并指向已验证的公共目录。
        let ret = unsafe { libc::chmod(c_path.as_ptr(), REAL_PUBLIC_DIR_MODE) };
        if ret != 0 {
            log::warn!(
                "daemon public chmod fix failed path={} errno={}",
                backend_path,
                last_errno()
            );
        }
    }
}

fn is_android_app_private_relative_path(relative: &str) -> bool {
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    parts.next() == Some("Android") && matches!(parts.next(), Some("data" | "media" | "obb"))
}

pub(super) fn path_is_same_or_child(path: &str, root: &str) -> bool {
    paths::is_same_or_child(path, root)
}

pub(super) fn should_filter_display_path(path: &str, operation_name: &str) -> bool {
    paths::is_filtered_media_provider_path(path)
        || SettingsHub::instance().should_filter_monitor_record(path, operation_name)
}

pub(super) fn monitor_operation_from_mask(mask: u32) -> &'static str {
    if (mask & (IN_MOVED_TO | IN_MOVED_FROM)) != 0 {
        "rename"
    } else if (mask & IN_DELETE) != 0 {
        "delete"
    } else if (mask & IN_ATTRIB) != 0 {
        "attrib"
    } else if (mask & (IN_MODIFY | IN_CLOSE_WRITE)) != 0 {
        "open:write"
    } else {
        "inotify"
    }
}

fn build_timestamp() -> String {
    let mut now: libc::time_t = 0;
    unsafe { time(&mut now as *mut _) };

    let mut tm_value: tm = unsafe { std::mem::zeroed() };
    let tm_ptr = unsafe { libc::localtime_r(&now as *const _, &mut tm_value as *mut _) };
    if tm_ptr.is_null() {
        return String::new();
    }

    let mut buffer = [0u8; 32];
    let format = b"%Y-%m-%d %H:%M:%S\0";
    let written = unsafe {
        libc::strftime(
            buffer.as_mut_ptr() as *mut _,
            buffer.len(),
            format.as_ptr() as *const _,
            &tm_value as *const _,
        )
    };
    if written == 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buffer[..written]).to_string()
}
