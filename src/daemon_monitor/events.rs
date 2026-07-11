use super::WatchNode;
use super::inotify::{cstring_path, last_errno};
use super::roots::map_record_from_path;
use crate::config::SettingsHub;
use crate::platform::{self, paths};
use crate::redirect::policy;
use libc::{IN_ATTRIB, IN_CLOSE_WRITE, IN_DELETE, IN_MOVED_FROM, IN_MOVED_TO, mode_t, time, tm};
use std::os::unix::fs::MetadataExt;

const LOGCAT_OP_TAG: &str = "FileMonitorOp";
const ANDROID_APP_UID_START: i32 = 10000;
const MEDIA_RW_GID: u32 = 1023;
const REDIRECT_BACKEND_DIR_REQUIRED_MODE: mode_t = 0o2773;
const PRIVATE_CHILD_DIR_REQUIRED_MODE: mode_t = 0o2751;
const PRIVATE_CHILD_FILE_REQUIRED_MODE: mode_t = 0o664;

pub(super) struct MonitorIdentity {
    pub(super) package_name: String,
    pub(super) identify_method: &'static str,
    pub(super) identify_reliability: &'static str,
}

pub(super) struct MonitorEventPaths {
    pub(super) backend_path: String,
    pub(super) display_path: String,
    pub(super) from_path: String,
}

impl MonitorEventPaths {
    pub(super) fn from_node(node: &WatchNode, name: &str) -> Self {
        let display_path = paths::normalize(&paths::join(&node.display_dir, name));
        Self {
            backend_path: paths::join(&node.backend_dir, name),
            display_path: display_path.clone(),
            from_path: map_record_from_path(
                &display_path,
                &node.record_display_root,
                &node.record_from_root,
            ),
        }
    }
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

    if should_keep_system_intermediate_owner_identity(identity, source, watch_package_name) {
        return false;
    }

    source == "public_root"
        && (identity.identify_method != "owner_uid" || identity.package_name != watch_package_name)
}

pub(super) struct AndroidPrivateOwnerRepairScope {
    pub(super) owner_package: String,
    pub(super) owner_uid: i32,
    pub(super) backend_root: String,
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

    if SettingsHub::instance().should_filter_monitor_record(&paths.display_path, operation_name) {
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
    let mut line = format!(
        "{}|{}|{}|{}|{}|ret=0|errno=0|identify_method={}|identify_reliability={}|op={}|source={}|mask=0x{:x}|backend={}",
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
        };
    }

    if let Some(package_name) = resolve_package_by_owner_uid(backend_path) {
        if should_prefer_watch_package_for_system_writer_owner(source, &package_name) {
            return watch_package_identity(watch_package_name, "watch_package", "medium");
        }
        return MonitorIdentity {
            package_name,
            identify_method: "owner_uid",
            identify_reliability: "high",
        };
    }

    if should_use_media_provider_fallback_identity(source, backend_path) {
        return media_provider_fallback_identity();
    }

    watch_package_identity(watch_package_name, "daemon_inotify", "medium")
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
        && policy::is_media_intermediate_package(&identity.package_name)
}

pub(super) fn should_prefer_watch_package_for_system_writer_owner(
    source: &str,
    owner_package: &str,
) -> bool {
    matches!(source, "path_mapping" | "sandbox_path" | "redirect_root")
        && (policy::is_system_writer_package(owner_package)
            || policy::is_media_intermediate_package(owner_package))
}

pub(super) fn repair_monitored_backend_owner(
    source: &str,
    watch_package_name: &str,
    display_path: &str,
    backend_path: &str,
) {
    let scope = if source == "redirect_root" {
        redirect_backend_owner_repair_scope(watch_package_name)
    } else {
        android_private_owner_repair_scope(display_path)
    };
    let Some(scope) = scope else {
        return;
    };
    repair_backend_owner_in_scope(&scope, backend_path);
}

fn repair_backend_owner_in_scope(scope: &AndroidPrivateOwnerRepairScope, backend_path: &str) {
    if !path_is_same_or_child(backend_path, &scope.backend_root) {
        return;
    }

    let mut pending = Vec::new();
    let mut current = backend_path.to_string();
    loop {
        if !path_is_same_or_child(&current, &scope.backend_root) {
            break;
        }
        pending.push(current.clone());
        if current == scope.backend_root {
            break;
        }
        let parent = raw_parent(&current);
        if parent.is_empty() || parent == current {
            break;
        }
        current = parent;
    }

    for path in pending.into_iter().rev() {
        chown_android_private_path_if_needed(
            &path,
            scope.owner_uid,
            &scope.owner_package,
            &scope.backend_root,
        );
    }
}

fn redirect_backend_owner_repair_scope(
    package_name: &str,
) -> Option<AndroidPrivateOwnerRepairScope> {
    if package_name.is_empty()
        || policy::is_media_intermediate_package(package_name)
        || policy::is_system_writer_package(package_name)
    {
        return None;
    }

    let owner_uid = resolve_android_private_owner_uid(package_name);
    if owner_uid < ANDROID_APP_UID_START {
        return None;
    }
    let user_id = platform::user_id_from_uid(owner_uid);
    if user_id < 0 || !SettingsHub::instance().should_redirect(package_name, owner_uid) {
        return None;
    }

    let storage_root = paths::default_redirect_target(package_name, user_id);
    Some(AndroidPrivateOwnerRepairScope {
        owner_package: package_name.to_string(),
        owner_uid,
        backend_root: paths::storage_to_data_media_path(&storage_root),
    })
}

pub(super) fn android_private_owner_repair_scope(
    display_path: &str,
) -> Option<AndroidPrivateOwnerRepairScope> {
    let normalized = paths::normalize(display_path);
    let owner_package = paths::extract_android_private_path_owner(&normalized);
    if owner_package.is_empty()
        || policy::is_media_intermediate_package(&owner_package)
        || policy::is_system_writer_package(&owner_package)
    {
        return None;
    }

    let user_id = paths::extract_user_id_from_storage_path(&normalized);
    if user_id < 0 {
        return None;
    }

    let owner_uid = resolve_android_private_owner_uid(&owner_package);
    if owner_uid < ANDROID_APP_UID_START || platform::user_id_from_uid(owner_uid) != user_id {
        return None;
    }

    if !SettingsHub::instance().should_redirect(&owner_package, owner_uid) {
        return None;
    }

    let backend_root =
        paths::android_private_data_media_root(&normalized, &owner_package, user_id)?;
    Some(AndroidPrivateOwnerRepairScope {
        owner_package,
        owner_uid,
        backend_root,
    })
}

fn resolve_android_private_owner_uid(package_name: &str) -> i32 {
    policy::get_fresh_uid_for_package(package_name)
}

fn chown_android_private_path_if_needed(
    path: &str,
    owner_uid: i32,
    owner_package: &str,
    private_root: &str,
) {
    let Some(c_path) = cstring_path(path) else {
        return;
    };

    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    let stat_ret = unsafe { libc::lstat(c_path.as_ptr(), st.as_mut_ptr()) };
    if stat_ret != 0 {
        return;
    }
    let st = unsafe { st.assume_init() };
    if st.st_uid == owner_uid as u32 && st.st_gid == MEDIA_RW_GID {
        chmod_android_private_path_if_needed(&c_path, path, &st, owner_package, private_root);
        return;
    }

    let ret = unsafe { libc::lchown(c_path.as_ptr(), owner_uid as u32, MEDIA_RW_GID) };
    if ret == 0 {
        chmod_android_private_path_if_needed(&c_path, path, &st, owner_package, private_root);
        log::info!(
            "daemon private owner fix path={} owner={} uid={}",
            path,
            owner_package,
            owner_uid
        );
        return;
    }

    let error_no = last_errno();
    if error_no != libc::ENOENT {
        log::warn!(
            "daemon private owner fix failed path={} owner={} uid={} errno={}",
            path,
            owner_package,
            owner_uid,
            error_no
        );
    }
}

fn chmod_android_private_path_if_needed(
    c_path: &std::ffi::CString,
    path: &str,
    st: &libc::stat,
    owner_package: &str,
    private_root: &str,
) {
    let file_type = st.st_mode & libc::S_IFMT as mode_t;
    if should_skip_android_private_cache_file_chmod(path, private_root, file_type) {
        return;
    }
    let required_mode = match file_type {
        value if value == libc::S_IFDIR as mode_t => android_private_dir_required_mode(path),
        value if value == libc::S_IFREG as mode_t => PRIVATE_CHILD_FILE_REQUIRED_MODE,
        _ => return,
    };
    let current_mode = st.st_mode & 0o7777;
    if current_mode & required_mode == required_mode {
        return;
    }

    let fixed_mode = current_mode | required_mode;
    let ret = unsafe { libc::chmod(c_path.as_ptr(), fixed_mode) };
    if ret == 0 {
        log::info!(
            "daemon private chmod fix path={} owner={} mode={:o}",
            path,
            owner_package,
            fixed_mode
        );
        return;
    }

    let error_no = last_errno();
    if error_no != libc::ENOENT {
        log::warn!(
            "daemon private chmod fix failed path={} owner={} mode={:o} errno={}",
            path,
            owner_package,
            fixed_mode,
            error_no
        );
    }
}

fn android_private_dir_required_mode(path: &str) -> mode_t {
    if paths::is_default_redirect_backend_path(path) {
        REDIRECT_BACKEND_DIR_REQUIRED_MODE
    } else {
        PRIVATE_CHILD_DIR_REQUIRED_MODE
    }
}

fn should_skip_android_private_cache_file_chmod(
    path: &str,
    private_root: &str,
    file_type: mode_t,
) -> bool {
    if file_type != libc::S_IFREG as mode_t || paths::is_sqlite_database_or_sidecar_path(path) {
        return false;
    }

    let Some(relative) = paths::child_suffix(path, private_root) else {
        return false;
    };
    let relative = relative.trim_start_matches('/');
    relative == "cache" || relative.starts_with("cache/")
}

pub(super) fn path_is_same_or_child(path: &str, root: &str) -> bool {
    paths::is_same_or_child(path, root)
}

fn raw_parent(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return path.to_string();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(pos) => trimmed[..pos].to_string(),
        None => String::new(),
    }
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
    } else if (mask & IN_CLOSE_WRITE) != 0 {
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
