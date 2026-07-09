use super::context;
use super::stats::InterceptHub;
use crate::config::SettingsHub;
use crate::monitor::{AuditTrail, OpKind};
use crate::platform::{self, paths};
use crate::redirect::{policy, writer};
use once_cell::sync::Lazy;
use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::Mutex;

const READ_ONLY_DENY_REASON: &str = "deny_reason=read_only_rule";
const FUSE_CALLER_MAX_AGE_MS: i64 = 1500;
const RECENT_SQLITE_ACCESS_WINDOW_MS: i64 = 30_000;
const MAX_RECENT_SQLITE_ACCESSES: usize = 16;
const SQLITE_SHM_MIN_SIZE: libc::off_t = 32 * 1024;
const FUSE_KIND_OPEN: i32 = 1;
const FUSE_KIND_MKDIR: i32 = 2;
const FUSE_KIND_MKNOD: i32 = 3;
const FUSE_KIND_RENAME: i32 = 4;
const FUSE_KIND_UNLINK: i32 = 5;
const FUSE_KIND_RMDIR: i32 = 6;

#[derive(Clone, Debug)]
struct RecentPrivateOwnerSqliteAccess {
    normalized_path: String,
    owner_package: String,
    caller_uid: i32,
    caller_packages: Vec<String>,
    updated_ms: i64,
}

static RECENT_PRIVATE_OWNER_SQLITE_ACCESS: Lazy<Mutex<VecDeque<RecentPrivateOwnerSqliteAccess>>> =
    Lazy::new(|| Mutex::new(VecDeque::new()));

pub fn record_read_only_fuse_operation(
    kind_code: i32,
    op_name: &str,
    op_filter: &str,
    path: &str,
    from_path: &str,
    caller_uid: i32,
    flags: i32,
) -> bool {
    let normalized_path = normalize_storage_path(path);
    let normalized_from = normalize_storage_path(from_path);
    if normalized_path.is_empty() && normalized_from.is_empty() {
        return false;
    }

    let probe_paths = [&normalized_path, &normalized_from];
    let audit_user_id = read_only_audit_user_id(&probe_paths);
    if audit_user_id < 0
        || !SettingsHub::instance().has_enabled_read_only_paths_for_user(audit_user_id)
    {
        return false;
    }

    let caller_package = if let Some(caller_package) =
        resolve_read_only_caller_package(caller_uid, &probe_paths)
    {
        caller_package
    } else if let Some(recent) = resolve_recent_read_only_owner(caller_uid, &probe_paths) {
        let Some(caller_package) = recent else {
            return false;
        };
        caller_package
    } else if caller_uid >= writer::ANDROID_APP_UID_START || is_system_writer_uid(caller_uid) {
        return false;
    } else {
        let Some(caller_package) = infer_read_only_caller_package_by_path(caller_uid, &probe_paths)
        else {
            return false;
        };
        caller_package
    };

    let Some(kind) = kind_from_code(kind_code) else {
        return false;
    };

    let record_path = if !normalized_path.is_empty() {
        normalized_path.as_str()
    } else {
        normalized_from.as_str()
    };

    let source = source_from_op_name(op_name);
    let mut extra = format!("op={}", sanitize_extra_value(op_name, source));
    if !op_filter.is_empty() {
        extra.push_str("|op_filter=");
        extra.push_str(&sanitize_extra_value(op_filter, "media_fuse"));
    }
    if flags >= 0 {
        extra.push_str("|flags=0x");
        extra.push_str(&format!("{:x}", flags));
    }
    if !normalized_from.is_empty() && normalized_from != record_path {
        extra.push_str("|from=");
        extra.push_str(&normalized_from);
    }
    extra.push_str("|source=");
    extra.push_str(source);
    extra.push_str("|caller_uid=");
    extra.push_str(&caller_uid.to_string());
    extra.push('|');
    extra.push_str(READ_ONLY_DENY_REASON);

    AuditTrail::instance().record_operation_result(
        kind,
        &caller_package,
        record_path,
        -1,
        libc::EROFS,
        &extra,
    );
    true
}

pub fn should_allow_private_owner_sqlite_access(path: &str, caller_uid: i32) -> bool {
    should_allow_private_owner_sqlite_access_for_caller(path, caller_uid, "")
}

pub fn should_allow_private_owner_sqlite_access_for_caller(
    path: &str,
    caller_uid: i32,
    caller_package: &str,
) -> bool {
    let normalized_path = normalize_storage_path(path);
    if normalized_path.is_empty() || caller_uid < writer::ANDROID_APP_UID_START {
        return false;
    }

    let user_id = paths::extract_user_id_from_storage_path(&normalized_path);
    if user_id < 0 || platform::user_id_from_uid(caller_uid) != user_id {
        return false;
    }

    if !paths::is_sqlite_database_or_sidecar_path(&normalized_path) {
        return false;
    }

    let owner_package = paths::extract_android_private_path_owner(&normalized_path);
    if owner_package.is_empty()
        || policy::is_media_intermediate_package(&owner_package)
        || policy::is_system_writer_package(&owner_package)
    {
        return false;
    }

    let owner_uid = resolve_private_owner_uid(&owner_package);
    if owner_uid >= writer::ANDROID_APP_UID_START && owner_uid == caller_uid {
        return false;
    }
    if owner_uid >= writer::ANDROID_APP_UID_START
        && platform::user_id_from_uid(owner_uid) != user_id
    {
        return false;
    }
    let caller_packages = resolve_caller_packages_with_explicit(caller_uid, caller_package);
    if caller_packages
        .iter()
        .any(|package| package == &owner_package)
        || caller_packages
            .iter()
            .any(|package| policy::is_system_writer_package(package))
    {
        return false;
    }

    let allow_by_owner = should_allow_private_owner_sqlite_for_owner(&owner_package, user_id);
    let allow_by_path_token = should_allow_private_owner_sqlite_for_caller_token(
        &normalized_path,
        &owner_package,
        &caller_packages,
    );
    if !allow_by_owner && !allow_by_path_token {
        return false;
    }

    remember_recent_private_owner_sqlite_access(
        &normalized_path,
        &owner_package,
        caller_uid,
        &caller_packages,
    );

    if SettingsHub::instance().is_file_monitor_enabled() {
        remember_private_owner_sqlite_caller_hint(
            &normalized_path,
            &owner_package,
            &caller_packages,
            caller_uid,
            user_id,
        );
    }

    log::debug!(
        "allow media fuse private owner sqlite access caller_uid={} owner={} owner_uid={} path={}",
        caller_uid,
        owner_package,
        owner_uid,
        normalized_path
    );
    true
}

pub fn has_recent_private_owner_sqlite_access_for_caller(
    path: &str,
    caller_uid: i32,
    caller_package: &str,
) -> bool {
    let Some((normalized_path, owner_package, user_id)) = resolve_private_owner_sqlite_path(path)
    else {
        return false;
    };
    if caller_uid < writer::ANDROID_APP_UID_START
        || platform::user_id_from_uid(caller_uid) != user_id
    {
        return false;
    }

    let caller_packages = resolve_caller_packages_with_explicit(caller_uid, caller_package);
    if caller_packages.is_empty() {
        return false;
    }

    let now_ms = paths::monotonic_ms();
    let Ok(mut accesses) = RECENT_PRIVATE_OWNER_SQLITE_ACCESS.lock() else {
        return false;
    };
    retain_recent_private_owner_sqlite_accesses(&mut accesses, now_ms);

    accesses.iter().rev().any(|access| {
        access.normalized_path == normalized_path
            && access.owner_package == owner_package
            && access.caller_uid == caller_uid
            && caller_packages.iter().any(|package| {
                is_private_owner_sqlite_caller_package(package, &owner_package)
                    && access
                        .caller_packages
                        .iter()
                        .any(|recent_package| recent_package.eq_ignore_ascii_case(package))
            })
    })
}

pub fn has_recent_private_owner_sqlite_access(path: &str) -> Option<i32> {
    let Some((normalized_path, owner_package, user_id)) = resolve_private_owner_sqlite_path(path)
    else {
        return None;
    };

    let now_ms = paths::monotonic_ms();
    let Ok(mut accesses) = RECENT_PRIVATE_OWNER_SQLITE_ACCESS.lock() else {
        return None;
    };
    retain_recent_private_owner_sqlite_accesses(&mut accesses, now_ms);

    accesses.iter().rev().find_map(|access| {
        if access.normalized_path == normalized_path
            && access.owner_package == owner_package
            && platform::user_id_from_uid(access.caller_uid) == user_id
        {
            Some(access.caller_uid)
        } else {
            None
        }
    })
}

pub fn should_allow_private_owner_sqlite_owner_backend(path: &str) -> Option<i32> {
    let Some((normalized_path, owner_package, user_id)) = resolve_private_owner_sqlite_path(path)
    else {
        return None;
    };

    if !should_force_private_owner_sqlite_for_owner(&owner_package, user_id) {
        return None;
    }

    let owner_uid = resolve_private_owner_uid(&owner_package);
    if owner_uid >= writer::ANDROID_APP_UID_START {
        if platform::user_id_from_uid(owner_uid) != user_id {
            return None;
        }
        log::debug!(
            "allow media fuse private owner sqlite backend owner={} owner_uid={} path={}",
            owner_package,
            owner_uid,
            normalized_path
        );
        return Some(owner_uid);
    }

    let synthetic_owner_uid = user_id
        .saturating_mul(platform::ANDROID_USER_ID_OFFSET)
        .saturating_add(writer::ANDROID_APP_UID_START);
    log::debug!(
        "allow media fuse private owner sqlite backend owner={} synthetic_uid={} path={}",
        owner_package,
        synthetic_owner_uid,
        normalized_path
    );
    Some(synthetic_owner_uid)
}

pub fn should_allow_public_mapping_target_access(path: &str, caller_uid: i32) -> bool {
    let normalized_path = normalize_storage_path(path);
    if normalized_path.is_empty() || caller_uid < writer::ANDROID_APP_UID_START {
        return false;
    }

    let user_id = paths::extract_user_id_from_storage_path(&normalized_path);
    if user_id < 0 || platform::user_id_from_uid(caller_uid) != user_id {
        return false;
    }

    if !SettingsHub::instance().is_public_mapping_target_path_for_user(user_id, &normalized_path) {
        return false;
    }

    log::debug!(
        "allow media fuse public mapping target access caller_uid={} user_id={} path={}",
        caller_uid,
        user_id,
        normalized_path
    );
    true
}

pub fn should_force_userspace_for_private_owner_sqlite_path(path: &str) -> bool {
    let Some((normalized_path, owner_package, user_id)) = resolve_private_owner_sqlite_path(path)
    else {
        return false;
    };

    if should_force_private_owner_sqlite_for_owner(&owner_package, user_id) {
        log::info!(
            "force media fuse userspace for private owner sqlite owner={} user_id={} path={}",
            owner_package,
            user_id,
            normalized_path
        );
        return true;
    }

    let (caller_uid, caller_package) = current_fuse_private_owner_sqlite_caller();
    if !should_allow_private_owner_sqlite_access_for_caller(
        &normalized_path,
        caller_uid,
        &caller_package,
    ) && !has_recent_private_owner_sqlite_access_for_caller(
        &normalized_path,
        caller_uid,
        &caller_package,
    ) {
        return false;
    }

    log::info!(
        "force media fuse userspace for private owner sqlite caller_uid={} owner={} user_id={} path={}",
        caller_uid,
        owner_package,
        user_id,
        normalized_path
    );
    true
}

pub fn prepare_private_owner_sqlite_sidecar(path: &str) -> bool {
    let Some((normalized_path, owner_package, user_id)) = resolve_private_owner_sqlite_path(path)
    else {
        return false;
    };

    if !should_force_private_owner_sqlite_for_owner(&owner_package, user_id)
        || !is_sqlite_shm_sidecar_path(&normalized_path)
    {
        return false;
    }

    let backend_path = writer::storage_to_data_media_path(&normalized_path);
    if backend_path == normalized_path || !backend_path.starts_with("/data/media/") {
        return false;
    }

    prepare_sqlite_shm_backend(&normalized_path, &backend_path)
}

fn resolve_private_owner_sqlite_path(path: &str) -> Option<(String, String, i32)> {
    let normalized_path = normalize_storage_path(path);
    if normalized_path.is_empty() || !paths::is_sqlite_database_or_sidecar_path(&normalized_path) {
        return None;
    }

    let user_id = paths::extract_user_id_from_storage_path(&normalized_path);
    if user_id < 0 {
        return None;
    }

    let owner_package = paths::extract_android_private_path_owner(&normalized_path);
    if owner_package.is_empty()
        || policy::is_media_intermediate_package(&owner_package)
        || policy::is_system_writer_package(&owner_package)
    {
        return None;
    }

    Some((normalized_path, owner_package, user_id))
}

fn is_sqlite_shm_sidecar_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".db-shm") || lower.ends_with(".sqlite-shm") || lower.ends_with(".sqlite3-shm")
}

fn prepare_sqlite_shm_backend(storage_path: &str, backend_path: &str) -> bool {
    let Ok(c_path) = CString::new(backend_path) else {
        return false;
    };

    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if fd < 0 {
        log::warn!(
            "prepare sqlite shm backend open failed errno={} storage={} backend={}",
            current_errno(),
            storage_path,
            backend_path
        );
        return false;
    }

    let size = fd_size(fd);
    if size >= SQLITE_SHM_MIN_SIZE {
        unsafe {
            libc::close(fd);
        }
        return true;
    }

    if size > 0 {
        let truncate_zero = unsafe { libc::ftruncate(fd, 0) };
        if truncate_zero != 0 {
            log::warn!(
                "prepare sqlite shm backend reset failed errno={} size={} storage={} backend={}",
                current_errno(),
                size,
                storage_path,
                backend_path
            );
            unsafe {
                libc::close(fd);
            }
            return false;
        }
    }

    let result = unsafe { libc::ftruncate(fd, SQLITE_SHM_MIN_SIZE) };
    let errno = current_errno();
    let final_size = fd_size(fd);
    unsafe {
        libc::close(fd);
    }

    if result == 0 {
        log::info!(
            "prepare sqlite shm backend ok size={} final_size={} storage={} backend={}",
            size,
            final_size,
            storage_path,
            backend_path
        );
        true
    } else {
        log::warn!(
            "prepare sqlite shm backend failed errno={} size={} final_size={} storage={} backend={}",
            errno,
            size,
            final_size,
            storage_path,
            backend_path
        );
        false
    }
}

fn fd_size(fd: libc::c_int) -> libc::off_t {
    let mut statbuf = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe { libc::fstat(fd, statbuf.as_mut_ptr()) };
    if result != 0 {
        return -1;
    }
    let statbuf = unsafe { statbuf.assume_init() };
    statbuf.st_size as libc::off_t
}

fn current_errno() -> i32 {
    unsafe { *libc::__errno() }
}

fn current_fuse_caller_uid() -> i32 {
    let caller_uid = InterceptHub::instance().get_fuse_caller_uid();
    if caller_uid < writer::ANDROID_APP_UID_START {
        return -1;
    }

    let age_ms = context::get_fuse_caller_uid_age_ms();
    if (0..=FUSE_CALLER_MAX_AGE_MS).contains(&age_ms) {
        caller_uid
    } else {
        -1
    }
}

fn current_fuse_private_owner_sqlite_caller() -> (i32, String) {
    let caller_uid = current_fuse_caller_uid();
    if caller_uid < writer::ANDROID_APP_UID_START {
        return (-1, String::new());
    }

    let hub = InterceptHub::instance();
    let current_package = hub.get_current_caller_package();
    let current_uid = hub.get_current_caller_uid();
    let current_age_ms = context::get_current_caller_age_ms();
    if current_uid == caller_uid && (0..=FUSE_CALLER_MAX_AGE_MS).contains(&current_age_ms) {
        (caller_uid, current_package)
    } else {
        (caller_uid, String::new())
    }
}

fn normalize_storage_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let normalized = paths::normalize(path);
    if paths::starts_with(&normalized, "/storage/emulated/") {
        normalized
    } else {
        String::new()
    }
}

fn resolve_private_owner_uid(package_name: &str) -> i32 {
    // FUSE callbacks run inside MediaProvider request threads. Keep this path
    // memory-only: refreshing the UID cache here can block the FUSE server.
    policy::get_uid_for_package(package_name)
}

fn resolve_caller_packages(caller_uid: i32) -> Vec<String> {
    let mut packages = policy::get_packages_for_uid(caller_uid);
    packages.sort();
    packages.dedup();
    packages
}

fn resolve_caller_packages_with_explicit(caller_uid: i32, caller_package: &str) -> Vec<String> {
    let mut packages = resolve_caller_packages(caller_uid);
    let caller_package = caller_package.trim();
    if !caller_package.is_empty()
        && !packages
            .iter()
            .any(|package| package.eq_ignore_ascii_case(caller_package))
    {
        packages.push(caller_package.to_string());
    }
    packages.sort();
    packages.dedup();
    packages
}

fn remember_recent_private_owner_sqlite_access(
    normalized_path: &str,
    owner_package: &str,
    caller_uid: i32,
    caller_packages: &[String],
) {
    let caller_packages: Vec<String> = caller_packages
        .iter()
        .filter(|package| is_private_owner_sqlite_caller_package(package, owner_package))
        .cloned()
        .collect();
    if caller_uid < writer::ANDROID_APP_UID_START || caller_packages.is_empty() {
        return;
    }

    let Ok(mut accesses) = RECENT_PRIVATE_OWNER_SQLITE_ACCESS.lock() else {
        return;
    };
    let now_ms = paths::monotonic_ms();
    retain_recent_private_owner_sqlite_accesses(&mut accesses, now_ms);
    accesses.retain(|access| {
        !(access.normalized_path == normalized_path
            && access.owner_package == owner_package
            && access.caller_uid == caller_uid)
    });
    accesses.push_back(RecentPrivateOwnerSqliteAccess {
        normalized_path: normalized_path.to_string(),
        owner_package: owner_package.to_string(),
        caller_uid,
        caller_packages,
        updated_ms: now_ms,
    });
    while accesses.len() > MAX_RECENT_SQLITE_ACCESSES {
        accesses.pop_front();
    }
}

fn retain_recent_private_owner_sqlite_accesses(
    accesses: &mut VecDeque<RecentPrivateOwnerSqliteAccess>,
    now_ms: i64,
) {
    accesses.retain(|access| {
        (0..=RECENT_SQLITE_ACCESS_WINDOW_MS).contains(&now_ms.saturating_sub(access.updated_ms))
    });
}

fn should_allow_private_owner_sqlite_for_owner(owner_package: &str, user_id: i32) -> bool {
    let config = SettingsHub::instance();
    config.is_file_monitor_enabled()
        || config.is_user_profile_enabled_in_memory(owner_package, user_id)
}

fn should_force_private_owner_sqlite_for_owner(owner_package: &str, user_id: i32) -> bool {
    SettingsHub::instance().is_user_profile_enabled_in_memory(owner_package, user_id)
}

fn should_allow_private_owner_sqlite_for_caller_token(
    normalized_path: &str,
    owner_package: &str,
    caller_packages: &[String],
) -> bool {
    caller_packages.iter().any(|package_name| {
        is_private_owner_sqlite_caller_package(package_name, owner_package)
            && private_owner_sqlite_path_mentions_caller_package(
                normalized_path,
                owner_package,
                package_name,
            )
    })
}

fn private_owner_sqlite_path_mentions_caller_package(
    normalized_path: &str,
    owner_package: &str,
    caller_package: &str,
) -> bool {
    let Some(private_root) = paths::android_private_data_media_root(
        normalized_path,
        owner_package,
        paths::extract_user_id_from_storage_path(normalized_path),
    ) else {
        return false;
    };
    let storage_private_root = paths::data_media_to_storage_path(&private_root);
    let Some(relative) = paths::relative_child_path(normalized_path, &storage_private_root) else {
        return false;
    };
    caller_package
        .rsplit('.')
        .any(|token| is_distinctive_package_path_token(token, relative))
}

fn is_distinctive_package_path_token(token: &str, relative_path: &str) -> bool {
    let token = token.trim();
    token.len() >= 4
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
        && relative_path
            .to_ascii_lowercase()
            .contains(&token.to_ascii_lowercase())
}

fn remember_private_owner_sqlite_caller_hint(
    normalized_path: &str,
    owner_package: &str,
    caller_packages: &[String],
    caller_uid: i32,
    user_id: i32,
) {
    if let Some(caller_package) =
        current_private_owner_sqlite_caller_package(owner_package, caller_uid)
    {
        crate::monitor::remember_private_path_caller_hint_in_memory(
            normalized_path,
            owner_package,
            &caller_package,
            caller_uid,
            user_id,
        );
        return;
    }

    if let Some(caller_package) = caller_packages
        .iter()
        .find(|package| is_private_owner_sqlite_caller_package(package, owner_package))
        .map(String::as_str)
    {
        crate::monitor::remember_private_path_caller_hint_in_memory(
            normalized_path,
            owner_package,
            caller_package,
            caller_uid,
            user_id,
        );
        return;
    }

    crate::monitor::remember_private_path_caller_uid_hint_in_memory(
        normalized_path,
        owner_package,
        caller_uid,
        user_id,
    );
}

fn current_private_owner_sqlite_caller_package(
    owner_package: &str,
    caller_uid: i32,
) -> Option<String> {
    let hub = InterceptHub::instance();
    if hub.get_current_caller_uid() != caller_uid {
        return None;
    }

    let age_ms = context::get_current_caller_age_ms();
    if !(0..=FUSE_CALLER_MAX_AGE_MS).contains(&age_ms) {
        return None;
    }

    let package_name = hub.get_current_caller_package();
    if is_private_owner_sqlite_caller_package(&package_name, owner_package) {
        Some(package_name)
    } else {
        None
    }
}

fn is_private_owner_sqlite_caller_package(package_name: &str, owner_package: &str) -> bool {
    !package_name.is_empty()
        && package_name != owner_package
        && !policy::is_system_writer_package(package_name)
        && !policy::is_media_intermediate_package(package_name)
}

fn read_only_audit_user_id(probe_paths: &[&String; 2]) -> i32 {
    probe_paths
        .iter()
        .find_map(|path| {
            if path.is_empty() {
                None
            } else {
                let user_id = paths::extract_user_id_from_storage_path(path);
                (user_id >= 0).then_some(user_id)
            }
        })
        .unwrap_or(-1)
}

fn resolve_read_only_caller_package(caller_uid: i32, paths: &[&String; 2]) -> Option<String> {
    if caller_uid < writer::ANDROID_APP_UID_START {
        return None;
    }

    let mut packages = policy::get_packages_for_uid(caller_uid);
    packages.sort();
    packages.dedup();

    let config = SettingsHub::instance();
    packages.into_iter().find(|package_name| {
        !policy::is_system_writer_package(package_name)
            && config.should_redirect(package_name, caller_uid)
            && paths.iter().any(|path| {
                !path.is_empty()
                    && writer::is_path_or_mapped_target_read_only_by_caller_paths(
                        path,
                        package_name,
                        caller_uid,
                    )
            })
    })
}

fn resolve_recent_read_only_owner(caller_uid: i32, paths: &[&String; 2]) -> Option<Option<String>> {
    for path in paths {
        if path.is_empty() {
            continue;
        }
        let user_id = paths::extract_user_id_from_storage_path(path);
        if user_id < 0 {
            continue;
        }
        let Some(identity) = crate::monitor::infer_recent_path_caller_identity(path, user_id)
        else {
            continue;
        };
        if policy::is_system_writer_package(&identity.package_name) {
            continue;
        }
        let package_uid = policy::get_fresh_uid_for_package(&identity.package_name);
        if caller_uid >= writer::ANDROID_APP_UID_START
            && !is_system_writer_uid(caller_uid)
            && package_uid != caller_uid
        {
            continue;
        }
        let effective_uid = if package_uid >= writer::ANDROID_APP_UID_START {
            package_uid
        } else if caller_uid >= writer::ANDROID_APP_UID_START {
            caller_uid
        } else {
            user_id
                .saturating_mul(platform::ANDROID_USER_ID_OFFSET)
                .saturating_add(writer::ANDROID_APP_UID_START)
        };
        if SettingsHub::instance().should_redirect(&identity.package_name, effective_uid)
            && paths.iter().any(|probe| {
                !probe.is_empty()
                    && writer::is_path_or_mapped_target_read_only_by_caller_paths(
                        probe,
                        &identity.package_name,
                        effective_uid,
                    )
            })
        {
            return Some(Some(identity.package_name));
        }
        log::debug!(
            "media fuse readonly skip recent caller={} uid={} path={}",
            identity.package_name,
            effective_uid,
            path
        );
        return Some(None);
    }
    None
}

fn infer_read_only_caller_package_by_path(caller_uid: i32, paths: &[&String; 2]) -> Option<String> {
    if crate::hook::is_path_owner_inference_disabled() {
        return None;
    }

    let config = SettingsHub::instance();
    for path in paths {
        if path.is_empty() {
            continue;
        }
        let user_id = if caller_uid >= writer::ANDROID_APP_UID_START {
            platform::user_id_from_uid(caller_uid)
        } else {
            paths::extract_user_id_from_storage_path(path)
        };
        if user_id < 0 {
            continue;
        }
        let synthetic_uid = user_id
            .saturating_mul(platform::ANDROID_USER_ID_OFFSET)
            .saturating_add(writer::ANDROID_APP_UID_START);
        let package_name = config.resolve_redirect_package_by_path_for_user(synthetic_uid, path);
        if package_name.is_empty() || policy::is_system_writer_package(&package_name) {
            continue;
        }
        let package_uid = policy::get_uid_for_package(&package_name);
        if caller_uid >= writer::ANDROID_APP_UID_START
            && !is_system_writer_uid(caller_uid)
            && package_uid != caller_uid
        {
            continue;
        }
        let effective_uid = if package_uid >= writer::ANDROID_APP_UID_START {
            package_uid
        } else if caller_uid >= writer::ANDROID_APP_UID_START {
            caller_uid
        } else {
            synthetic_uid
        };
        if config.should_redirect(&package_name, effective_uid)
            && paths.iter().any(|probe| {
                !probe.is_empty()
                    && writer::is_path_or_mapped_target_read_only_by_caller_paths(
                        probe,
                        &package_name,
                        effective_uid,
                    )
            })
        {
            log::debug!(
                "media fuse readonly infer caller={} uid={} raw_uid={} path={}",
                package_name,
                effective_uid,
                caller_uid,
                path
            );
            return Some(package_name);
        }
    }
    None
}

fn is_system_writer_uid(uid: i32) -> bool {
    uid >= 0
        && (policy::is_shared_uid_process(uid)
            || policy::get_packages_for_uid(uid)
                .iter()
                .any(|package_name| policy::is_system_writer_package(package_name)))
}

fn kind_from_code(kind_code: i32) -> Option<OpKind> {
    match kind_code {
        FUSE_KIND_OPEN => Some(OpKind::Open),
        FUSE_KIND_MKDIR => Some(OpKind::Mkdir),
        FUSE_KIND_MKNOD => Some(OpKind::Mknod),
        FUSE_KIND_RENAME => Some(OpKind::Rename),
        FUSE_KIND_UNLINK => Some(OpKind::Unlink),
        FUSE_KIND_RMDIR => Some(OpKind::Rmdir),
        _ => None,
    }
}

fn source_from_op_name(op_name: &str) -> &'static str {
    if op_name.contains("Fuse") || op_name.contains("fuse") {
        "media_fuse"
    } else {
        "media_provider_open"
    }
}

fn sanitize_extra_value(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    trimmed
        .chars()
        .map(|ch| {
            if ch == '|' || ch == '\n' || ch == '\r' {
                '_'
            } else {
                ch
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_storage_path_accepts_storage_aliases() {
        assert_eq!(
            normalize_storage_path("/data/media/0/DCIM/a.jpg"),
            "/storage/emulated/0/DCIM/a.jpg"
        );
        assert_eq!(
            normalize_storage_path("/mnt/user/0/emulated/0/DCIM/a.jpg"),
            "/storage/emulated/0/DCIM/a.jpg"
        );
        assert_eq!(normalize_storage_path("/data/user/0/a"), "");
    }

    #[test]
    fn sanitize_extra_value_removes_log_separators() {
        assert_eq!(
            sanitize_extra_value("rename|bad\nx", "fallback"),
            "rename_bad_x"
        );
        assert_eq!(sanitize_extra_value("  ", "fallback"), "fallback");
    }

    #[test]
    fn source_distinguishes_fuse_from_provider_open() {
        assert_eq!(
            source_from_op_name("insertFileIfNecessaryForFuse"),
            "media_fuse"
        );
        assert_eq!(source_from_op_name("openFile"), "media_provider_open");
    }

    #[test]
    fn read_only_audit_user_id_uses_first_storage_probe() {
        let empty = String::new();
        let path = "/storage/emulated/10/Download/Nnngram/a.jpg".to_string();

        assert_eq!(read_only_audit_user_id(&[&empty, &path]), 10);
        assert_eq!(read_only_audit_user_id(&[&empty, &empty]), -1);
    }

    #[test]
    fn provider_open_read_only_skips_other_app_rule() {
        use crate::config::{AppProfile, UserProfile};
        use crate::domain::PathMapping;
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let previous_monitor = hub.replace_test_file_monitor_enabled(true);
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([
            (
                "com.aliyun.tongyi".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(
                        0,
                        UserProfile {
                            is_enabled: true,
                            is_mapping_mode_only: false,
                            allowed_real_paths: Vec::new(),
                            excluded_real_paths: Vec::new(),
                            sandboxed_paths: Vec::new(),
                            read_only_paths: vec![
                                "/storage/emulated/0/DCIM".to_string(),
                                "/storage/emulated/0/Pictures".to_string(),
                            ],
                            path_mappings: Vec::new(),
                        },
                    )]),
                },
            ),
            (
                "xyz.nextalone.nnngram".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(
                        0,
                        UserProfile {
                            is_enabled: true,
                            is_mapping_mode_only: false,
                            allowed_real_paths: vec![
                                "/storage/emulated/0/DCIM".to_string(),
                                "/storage/emulated/0/Pictures".to_string(),
                            ],
                            excluded_real_paths: Vec::new(),
                            sandboxed_paths: Vec::new(),
                            read_only_paths: Vec::new(),
                            path_mappings: vec![PathMapping::new(
                                "/storage/emulated/0/Download/Nnngram".to_string(),
                                "/storage/emulated/0/Download/第三方下载/Nnngram".to_string(),
                            )],
                        },
                    )]),
                },
            ),
        ]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([
            ("com.aliyun.tongyi".to_string(), 10340),
            ("xyz.nextalone.nnngram".to_string(), 10312),
            ("com.android.providers.media.module".to_string(), 10217),
        ]));
        let was_monitor_enabled = AuditTrail::instance().is_enabled();
        AuditTrail::instance().set_enabled(true);
        AuditTrail::instance().init("com.android.providers.media.module", 10217);

        let path = "/storage/emulated/0/Pictures/Nnngram/IMG_20260709_222730_377.jpg";
        AuditTrail::instance().record_provider_open_path(path, 10312, "xyz.nextalone.nnngram");

        let denied = record_read_only_fuse_operation(
            FUSE_KIND_OPEN,
            "openFile",
            "open:write",
            path,
            "",
            10217,
            0x28002,
        );

        AuditTrail::instance().set_enabled(was_monitor_enabled);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);
        crate::monitor::clear_recent_private_owner_hint_for_tests();

        assert!(!denied);
    }

    #[test]
    fn provider_open_read_only_denies_recent_read_only_caller() {
        use crate::config::{AppProfile, UserProfile};
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let previous_monitor = hub.replace_test_file_monitor_enabled(true);
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.aliyun.tongyi".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec!["/storage/emulated/0/Pictures".to_string()],
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([
            ("com.aliyun.tongyi".to_string(), 10340),
            ("com.android.providers.media.module".to_string(), 10217),
        ]));
        let was_monitor_enabled = AuditTrail::instance().is_enabled();
        AuditTrail::instance().set_enabled(true);
        AuditTrail::instance().init("com.android.providers.media.module", 10217);

        let path = "/storage/emulated/0/Pictures/Tongyi/photo.jpg";
        AuditTrail::instance().record_provider_open_path(path, 10340, "com.aliyun.tongyi");

        let denied = record_read_only_fuse_operation(
            FUSE_KIND_OPEN,
            "openFile",
            "open:write",
            path,
            "",
            10217,
            0x28002,
        );

        AuditTrail::instance().set_enabled(was_monitor_enabled);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);
        crate::monitor::clear_recent_private_owner_hint_for_tests();

        assert!(denied);
    }

    #[test]
    fn provider_open_read_only_without_recent_caller_does_not_infer_owner() {
        use crate::config::{AppProfile, UserProfile};
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let previous_monitor = hub.replace_test_file_monitor_enabled(true);
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.aliyun.tongyi".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec!["/storage/emulated/0/Pictures".to_string()],
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([
            ("com.aliyun.tongyi".to_string(), 10340),
            ("com.android.providers.media.module".to_string(), 10217),
        ]));
        let was_monitor_enabled = AuditTrail::instance().is_enabled();
        AuditTrail::instance().set_enabled(true);
        AuditTrail::instance().init("com.android.providers.media.module", 10217);

        let denied = record_read_only_fuse_operation(
            FUSE_KIND_OPEN,
            "openFile",
            "open:write",
            "/storage/emulated/0/Pictures/Nnngram/photo.jpg",
            "",
            10217,
            0x28002,
        );

        AuditTrail::instance().set_enabled(was_monitor_enabled);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);
        crate::monitor::clear_recent_private_owner_hint_for_tests();

        assert!(!denied);
    }

    #[test]
    fn provider_open_read_only_app_uid_without_rule_does_not_infer_owner() {
        use crate::config::{AppProfile, UserProfile};
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let previous_monitor = hub.replace_test_file_monitor_enabled(true);
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([
            (
                "com.aliyun.tongyi".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(
                        0,
                        UserProfile {
                            is_enabled: true,
                            is_mapping_mode_only: false,
                            allowed_real_paths: Vec::new(),
                            excluded_real_paths: Vec::new(),
                            sandboxed_paths: Vec::new(),
                            read_only_paths: vec!["/storage/emulated/0/Pictures".to_string()],
                            path_mappings: Vec::new(),
                        },
                    )]),
                },
            ),
            (
                "me.fakerqu.test.storageredirect".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(
                        0,
                        UserProfile {
                            is_enabled: false,
                            is_mapping_mode_only: false,
                            allowed_real_paths: Vec::new(),
                            excluded_real_paths: Vec::new(),
                            sandboxed_paths: Vec::new(),
                            read_only_paths: Vec::new(),
                            path_mappings: Vec::new(),
                        },
                    )]),
                },
            ),
        ]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([
            ("com.aliyun.tongyi".to_string(), 10340),
            ("me.fakerqu.test.storageredirect".to_string(), 10209),
        ]));
        let was_monitor_enabled = AuditTrail::instance().is_enabled();
        AuditTrail::instance().set_enabled(true);
        AuditTrail::instance().init("com.android.providers.media.module", 10217);

        let denied = record_read_only_fuse_operation(
            FUSE_KIND_OPEN,
            "openFile",
            "open:write",
            "/storage/emulated/0/Pictures/Nnngram/photo.jpg",
            "",
            10209,
            0x28002,
        );

        AuditTrail::instance().set_enabled(was_monitor_enabled);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);
        crate::monitor::clear_recent_private_owner_hint_for_tests();

        assert!(!denied);
    }

    #[test]
    fn allows_cross_app_sqlite_sidecar_for_enabled_private_owner() {
        use crate::config::{AppProfile, UserProfile};
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.eg.android.AlipayGphone".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::from([
                ("com.eg.android.AlipayGphone".to_string(), 10274),
                ("com.leo.xposed.xradiant".to_string(), 10164),
            ]));

        let allowed = should_allow_private_owner_sqlite_access(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
            10164,
        );
        let hint = crate::monitor::infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260626_170546.json",
            0,
        );

        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(allowed);
        let hint = hint.expect("XRadiant private sqlite access should seed export attribution");
        assert_eq!(hint.package_name, "com.leo.xposed.xradiant");
        assert_eq!(hint.source, "recent_private_caller");
        assert_eq!(hint.confidence, "medium");
    }

    #[test]
    fn file_monitor_allows_cross_app_sqlite_sidecar_without_owner_profile() {
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::new());
        let previous_monitor = hub.replace_test_file_monitor_enabled(true);
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::from([
                ("com.eg.android.AlipayGphone".to_string(), 10274),
                ("com.leo.xposed.xradiant".to_string(), 10164),
            ]));

        let allowed = should_allow_private_owner_sqlite_access(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
            10164,
        );
        let hint = crate::monitor::infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260629_185704.json",
            0,
        );

        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(allowed);
        let hint = hint.expect("file-monitor private sqlite access should seed export attribution");
        assert_eq!(hint.package_name, "com.leo.xposed.xradiant");
        assert_eq!(hint.source, "recent_private_caller");
    }

    #[test]
    fn current_caller_seeds_sqlite_hint_when_uid_cache_missing() {
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::new());
        let previous_monitor = hub.replace_test_file_monitor_enabled(true);
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::new());
        context::set_current_caller_from_external_signal("com.leo.xposed.xradiant", 10164);

        let allowed = should_allow_private_owner_sqlite_access(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
            10164,
        );
        let hint = crate::monitor::infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260629_192106.json",
            0,
        );

        context::clear_current_caller();
        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(allowed);
        let hint = hint.expect("thread caller should seed XRadiant export attribution");
        assert_eq!(hint.package_name, "com.leo.xposed.xradiant");
        assert_eq!(hint.source, "recent_private_caller");
    }

    #[test]
    fn explicit_caller_package_allows_sqlite_when_uid_cache_missing() {
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::new());
        let previous_monitor = hub.replace_test_file_monitor_enabled(false);
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::from([(
                "com.eg.android.AlipayGphone".to_string(),
                10274,
            )]));

        let allowed = should_allow_private_owner_sqlite_access_for_caller(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
            10164,
            "com.leo.xposed.xradiant",
        );

        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(allowed);
    }

    #[test]
    fn caller_token_matches_storage_path_under_data_media_private_root() {
        assert!(private_owner_sqlite_path_mentions_caller_package(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
            "com.eg.android.AlipayGphone",
            "com.leo.xposed.xradiant",
        ));
    }

    #[test]
    fn recent_sqlite_access_survives_later_missing_thread_caller() {
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::new());
        let previous_monitor = hub.replace_test_file_monitor_enabled(false);
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::from([
                ("com.eg.android.AlipayGphone".to_string(), 10274),
                ("com.leo.xposed.xradiant".to_string(), 10164),
            ]));

        let path = "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm";
        assert!(should_allow_private_owner_sqlite_access(path, 10164));
        let recent = has_recent_private_owner_sqlite_access_for_caller(
            path,
            10164,
            "com.leo.xposed.xradiant",
        );
        let recent_uid = has_recent_private_owner_sqlite_access(path);

        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(recent);
        assert_eq!(recent_uid, Some(10164));
    }

    #[test]
    fn disabled_monitor_allows_caller_token_sqlite_without_seeding_hint() {
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::new());
        let previous_monitor = hub.replace_test_file_monitor_enabled(false);
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::from([
                ("com.eg.android.AlipayGphone".to_string(), 10274),
                ("com.leo.xposed.xradiant".to_string(), 10164),
            ]));

        let allowed = should_allow_private_owner_sqlite_access(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
            10164,
        );
        let hint = crate::monitor::infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260630_090102.json",
            0,
        );

        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(allowed);
        assert!(hint.is_none());
    }

    #[test]
    fn disabled_zero_width_fuse_fix_still_allows_caller_token_sqlite() {
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::new());
        let previous_monitor = hub.replace_test_file_monitor_enabled(false);
        let previous_fuse_fix = hub.replace_test_fuse_fix_enabled(false);
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::from([
                ("com.eg.android.AlipayGphone".to_string(), 10274),
                ("com.leo.xposed.xradiant".to_string(), 10164),
            ]));

        let allowed = should_allow_private_owner_sqlite_access(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
            10164,
        );
        let hint = crate::monitor::infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260630_092859.json",
            0,
        );

        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_fuse_fix_enabled(previous_fuse_fix.0, previous_fuse_fix.1);
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(allowed);
        assert!(hint.is_none());
    }

    #[test]
    fn disabled_monitor_rejects_generic_cross_app_sqlite_path() {
        use std::collections::HashMap;

        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::new());
        let previous_monitor = hub.replace_test_file_monitor_enabled(false);
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::from([
                ("com.eg.android.AlipayGphone".to_string(), 10274),
                ("com.leo.xposed.xradiant".to_string(), 10164),
            ]));

        let allowed = should_allow_private_owner_sqlite_access(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/cache/app.db-shm",
            10164,
        );

        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(!allowed);
    }

    #[test]
    fn allows_public_mapping_target_for_same_user_caller() {
        use crate::config::{AppProfile, UserProfile};
        use crate::domain::PathMapping;
        use std::collections::HashMap;

        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "idm.internet.download.manager.plus".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/1DMP".to_string(),
                            "/storage/emulated/0/Download/第三方下载/1DMP".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let allowed = should_allow_public_mapping_target_access(
            "/storage/emulated/0/Download/第三方下载/1DMP/storage.redirect.x.zip",
            10123,
        );
        let request_path = should_allow_public_mapping_target_access(
            "/storage/emulated/0/Download/1DMP/storage.redirect.x.zip",
            10123,
        );
        let cross_user = should_allow_public_mapping_target_access(
            "/storage/emulated/10/Download/第三方下载/1DMP/storage.redirect.x.zip",
            10123,
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(allowed);
        assert!(!request_path);
        assert!(!cross_user);
    }

    #[test]
    fn public_mapping_target_access_is_independent_of_fuse_daemon_redirect() {
        use crate::config::{AppProfile, UserProfile};
        use crate::domain::PathMapping;
        use std::collections::HashMap;

        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "idm.internet.download.manager.plus".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/1DMP".to_string(),
                            "/storage/emulated/0/Download/第三方下载/1DMP".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let previous_fuse_daemon = hub.replace_test_fuse_daemon_redirect_enabled(false);
        let disabled = should_allow_public_mapping_target_access(
            "/storage/emulated/0/Download/第三方下载/1DMP/storage.redirect.x.zip",
            10123,
        );
        hub.restore_test_fuse_daemon_redirect_enabled(
            previous_fuse_daemon.0,
            previous_fuse_daemon.1,
        );

        let previous_fuse_daemon = hub.replace_test_fuse_daemon_redirect_enabled(true);
        let enabled = should_allow_public_mapping_target_access(
            "/storage/emulated/0/Download/第三方下载/1DMP/storage.redirect.x.zip",
            10123,
        );
        hub.restore_test_fuse_daemon_redirect_enabled(
            previous_fuse_daemon.0,
            previous_fuse_daemon.1,
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(disabled);
        assert!(enabled);
    }

    #[test]
    fn public_mapping_target_access_does_not_cover_android_private_paths() {
        use crate::config::{AppProfile, UserProfile};
        use crate::domain::PathMapping;
        use std::collections::HashMap;

        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.example.owner".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/Owner".to_string(),
                            "/storage/emulated/0/Android/media/com.example.owner/cache".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let allowed = should_allow_public_mapping_target_access(
            "/storage/emulated/0/Android/media/com.example.owner/cache/file.bin",
            10123,
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(!allowed);
    }

    #[test]
    fn forces_userspace_for_enabled_private_owner_sqlite_path() {
        use crate::config::{AppProfile, UserProfile};
        use std::collections::HashMap;

        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.eg.android.AlipayGphone".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::from([
                ("com.eg.android.AlipayGphone".to_string(), 10274),
                ("com.leo.xposed.xradiant".to_string(), 10164),
            ]));

        let owner_forced = should_force_userspace_for_private_owner_sqlite_path(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
        );
        context::set_fuse_caller_uid(10164);
        let cross_caller_forced = should_force_userspace_for_private_owner_sqlite_path(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
        );
        let non_sqlite = should_force_userspace_for_private_owner_sqlite_path(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/avatar.png",
        );
        context::clear_fuse_caller_uid();

        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(owner_forced);
        assert!(cross_caller_forced);
        assert!(!non_sqlite);
    }

    #[test]
    fn does_not_force_userspace_for_disabled_private_owner_sqlite_without_caller() {
        use crate::config::{AppProfile, UserProfile};
        use std::collections::HashMap;

        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.eg.android.AlipayGphone".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: false,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let forced = should_force_userspace_for_private_owner_sqlite_path(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(!forced);
    }

    #[test]
    fn denies_private_owner_fuse_access_outside_sqlite_scope() {
        use crate::config::{AppProfile, UserProfile};
        use std::collections::HashMap;

        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.eg.android.AlipayGphone".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let (previous_uids, previous_packages, previous_system_writers, previous_uid_loaded) =
            policy::replace_test_uid_cache(HashMap::from([
                ("com.eg.android.AlipayGphone".to_string(), 10274),
                ("com.leo.xposed.xradiant".to_string(), 10164),
            ]));

        let non_sqlite = should_allow_private_owner_sqlite_access(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/avatar.png",
            10164,
        );
        let owner_uid = should_allow_private_owner_sqlite_access(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
            10274,
        );
        let cross_user = should_allow_private_owner_sqlite_access(
            "/storage/emulated/10/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
            10164,
        );

        policy::restore_test_uid_cache(
            previous_uids,
            previous_packages,
            previous_system_writers,
            previous_uid_loaded,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(!non_sqlite);
        assert!(!owner_uid);
        assert!(!cross_user);
    }

    #[test]
    fn low_uid_can_infer_read_only_owner_from_mapping_path() {
        use crate::config::{AppProfile, UserProfile};
        use crate::domain::PathMapping;
        use std::collections::HashMap;

        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.mobileqq".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec!["/storage/emulated/0/Download".to_string()],
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/QQ".to_string(),
                            "/storage/emulated/0/Download/third-party/QQ".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let path = "/storage/emulated/0/Download/QQ".to_string();
        let empty = String::new();
        let inferred = infer_read_only_caller_package_by_path(1023, &[&path, &empty]);

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(inferred.as_deref(), Some("com.tencent.mobileqq"));
    }
}
