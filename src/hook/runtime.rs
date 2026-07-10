use super::stats::InterceptHub;
use super::{caller, context, diagnostic, monitor, path as path_utils};
use crate::config::SettingsHub;
use crate::platform::{self, fs, paths};
use crate::redirect::{policy, process_redirect_path, record_redirect_hit};
use libc::{AT_FDCWD, c_char, c_void, mode_t};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};

static CALL_PREV_FALLBACK_MISS: AtomicU64 = AtomicU64::new(0);
static FORK_CHILD_HOOK_BYPASS: AtomicU64 = AtomicU64::new(0);
const ANDROID_APP_UID_START: i32 = 10000;
const MEDIA_RW_GID: u32 = 1023;
const STORAGE_DIR_MODE: mode_t = 0o2773;
const PRIVATE_CHILD_DIR_REQUIRED_MODE: mode_t = 0o2751;
const PRIVATE_CHILD_FILE_REQUIRED_MODE: mode_t = 0o664;

struct RedirectDirOwner {
    uid: i32,
    user_id: i32,
    package_name: String,
}

#[inline]
fn should_log_fallback_miss(count: u64) -> bool {
    count == 1 || count.is_multiple_of(256)
}

#[inline]
fn log_call_prev_fallback(site: &str) {
    let count = CALL_PREV_FALLBACK_MISS.fetch_add(1, Ordering::Relaxed) + 1;
    if should_log_fallback_miss(count) {
        log::warn!("{}: prev unavail, libc fallback n={}", site, count);
    }
}

#[inline]
fn should_bypass_hook_in_fork_child(hub: &InterceptHub) -> bool {
    if !srx_hook::is_forked_child() {
        return false;
    }

    if policy::is_system_writer_package(&hub.get_package_name()) {
        return false;
    }

    if policy::is_saf_native_monitor_bridge_package(&hub.get_package_name()) {
        return false;
    }

    let process_uid = unsafe { libc::getuid() as i32 };
    !policy::is_shared_uid_process(process_uid)
}

#[inline]
fn log_fork_child_hook_bypass(package_name: &str) {
    let count = FORK_CHILD_HOOK_BYPASS.fetch_add(1, Ordering::Relaxed) + 1;
    if should_log_fallback_miss(count) {
        log::warn!("fork child hook bypass pkg={} n={}", package_name, count);
    }
}

pub unsafe fn call_prev<R: Copy, F, Fb>(proxy_fn: *mut c_void, libc_fallback: Fb, f: F) -> R
where
    F: FnOnce(*mut c_void) -> R,
    Fb: FnOnce() -> R,
{
    match srx_hook::with_prev_func(
        proxy_fn,
        |prev| {
            if prev.is_null() { None } else { Some(f(prev)) }
        },
    ) {
        Some(Some(result)) => result,
        None => {
            log_call_prev_fallback("call_prev");
            libc_fallback()
        }
        Some(None) => libc_fallback(),
    }
}

// 惰性版本：避免带副作用的 fallback 被提前求值
pub unsafe fn call_prev_lazy<R, F, Fb>(proxy_fn: *mut c_void, fallback_fn: Fb, f: F) -> R
where
    F: FnOnce(*mut c_void) -> R,
    Fb: FnOnce() -> R,
{
    match srx_hook::with_prev_func(
        proxy_fn,
        |prev| {
            if prev.is_null() { None } else { Some(f(prev)) }
        },
    ) {
        Some(Some(result)) => result,
        None => {
            log_call_prev_fallback("call_prev_lazy");
            fallback_fn()
        }
        Some(None) => fallback_fn(),
    }
}

#[inline]
pub fn current_errno() -> i32 {
    unsafe { *libc::__errno() }
}

#[inline]
pub fn set_errno(errno: i32) {
    unsafe { *libc::__errno() = errno };
}

#[inline]
pub fn set_read_only_errno() {
    set_errno(libc::EROFS);
}

#[inline]
pub fn errno_for_result(result: i32) -> i32 {
    if result < 0 { current_errno() } else { 0 }
}

// 重入场景走原函数；fork 子进程只保留系统代写进程的 hook
pub fn with_hook_guard<OriginalCall, HookCall, R>(
    original_call: OriginalCall,
    hook_call: HookCall,
) -> R
where
    OriginalCall: FnOnce() -> R,
    HookCall: FnOnce(&InterceptHub) -> R,
{
    if context::ReentryGuard::is_reentrant() {
        return original_call();
    }

    let hub = InterceptHub::instance();
    if should_bypass_hook_in_fork_child(hub) {
        log_fork_child_hook_bypass(&hub.get_package_name());
        return original_call();
    }

    let _guard = context::ReentryGuard::enter();
    hook_call(hub)
}

pub fn should_resolve_caller_context(hub: &InterceptHub) -> bool {
    if context::is_current_caller_scope_active() {
        return false;
    }
    hub.is_monitor_enabled() || policy::is_system_writer_package(&hub.get_package_name())
}

// 写入类重定向时按需补齐目标父目录
pub fn ensure_redirect_parent_directory(op_name: &str, from_path: &str, to_path: &str, flags: i32) {
    if from_path == to_path || !monitor::has_write_intent_flags(flags) {
        return;
    }

    let parent_dir = paths::parent(to_path);
    if parent_dir.is_empty() || parent_dir == "/" {
        return;
    }

    if fs::is_directory(&parent_dir) {
        ensure_redirect_parent_dirs(to_path, STORAGE_DIR_MODE);
        return;
    }

    ensure_redirect_parent_dirs(to_path, STORAGE_DIR_MODE);
    if fs::is_directory(&parent_dir) {
        log::debug!("redirect parent mkdir ok op={} dir={}", op_name, parent_dir);
        return;
    }

    let error_no = current_errno();
    log::warn!(
        "redirect parent mkdir failed op={} dir={} errno={} from={} to={}",
        op_name,
        parent_dir,
        error_no,
        from_path,
        to_path
    );
}

// 入口为用户可见 /storage/emulated/X，底层要落到 /data/media/X
pub fn ensure_redirect_parent_dirs(path: &str, mode: mode_t) {
    if path.is_empty() {
        return;
    }

    let owner = resolve_redirect_dir_owner();
    if path.starts_with("/storage/emulated/") {
        let underlying = paths::storage_to_data_media_path(path);
        create_storage_parent_dirs_recursive(&underlying, STORAGE_DIR_MODE, owner.as_ref());
    } else if path.starts_with("/data/media/") {
        create_storage_parent_dirs_recursive(path, STORAGE_DIR_MODE, owner.as_ref());
    } else {
        create_parent_dirs_recursive(path, mode, owner.as_ref());
    }
}

pub fn normalize_redirect_directory(path: &str) {
    if path.is_empty() {
        return;
    }

    let owner = resolve_redirect_dir_owner();
    normalize_redirect_dir_metadata(path, STORAGE_DIR_MODE, owner.as_ref());
}

pub fn fix_system_writer_android_private_owner(path: &str, include_path: bool) {
    if path.is_empty() {
        return;
    }

    let hub = InterceptHub::instance();
    if !policy::is_system_writer_package(&hub.get_package_name()) {
        return;
    }

    let saved_errno = current_errno();
    fix_system_writer_android_private_owner_inner(path, include_path);
    set_errno(saved_errno);
}

fn create_storage_parent_dirs_recursive(
    path: &str,
    mode: mode_t,
    owner: Option<&RedirectDirOwner>,
) {
    let base = data_media_user_dir(path);
    if base.is_empty() {
        create_parent_dirs_recursive(path, mode, owner);
        return;
    }

    if !fs::is_directory(&base) {
        log::warn!(
            "skip auto mkdir missing storage top dir base={} path={}",
            base,
            path
        );
        return;
    }

    create_parent_dirs_recursive_until(path, &base, mode, owner);
}

fn data_media_user_dir(path: &str) -> String {
    paths::data_media_user_root(path).unwrap_or_default()
}

fn fix_system_writer_android_private_owner_inner(path: &str, include_path: bool) {
    let normalized = paths::normalize(path);
    let storage_path = if normalized.starts_with("/data/media/") {
        paths::data_media_to_storage_path(&normalized)
    } else {
        normalized.clone()
    };
    let Some(owner_package) = android_private_owner_package(&storage_path) else {
        return;
    };

    let user_id = paths::extract_user_id_from_storage_path(&storage_path);
    if user_id < 0 {
        return;
    }

    let owner_uid = resolve_private_owner_uid(&owner_package);
    if owner_uid < ANDROID_APP_UID_START || platform::user_id_from_uid(owner_uid) != user_id {
        return;
    }

    let allow_cross_caller_sqlite = should_allow_sqlite_private_owner_fix_for_current_caller(
        &storage_path,
        &owner_package,
        owner_uid,
        user_id,
    );
    if !allow_cross_caller_sqlite && !is_private_owner_fix_enabled(&owner_package, owner_uid) {
        return;
    }
    if !allow_cross_caller_sqlite
        && !is_private_owner_fix_allowed_for_current_caller(&owner_package, owner_uid)
    {
        return;
    }

    let Some(private_root) =
        paths::android_private_data_media_root(&storage_path, &owner_package, user_id)
    else {
        return;
    };

    let backend_path = if normalized.starts_with("/data/media/") {
        normalized
    } else {
        paths::storage_to_data_media_path(&storage_path)
    };
    let parent = paths::parent(&backend_path);
    if path_is_same_or_child(&parent, &private_root) {
        chown_private_path_if_needed(&parent, owner_uid, &owner_package, &private_root);
    }

    if should_fix_private_path_node(&backend_path, &private_root, include_path) {
        chown_private_path_if_needed(&backend_path, owner_uid, &owner_package, &private_root);
    }
}

fn should_fix_private_path_node(
    backend_path: &str,
    private_root: &str,
    include_path: bool,
) -> bool {
    if !path_is_same_or_child(backend_path, private_root) {
        return false;
    }
    include_path || (backend_path != private_root && path_node_exists(backend_path))
}

fn android_private_owner_package(normalized_path: &str) -> Option<String> {
    let owner = paths::extract_android_private_path_owner(normalized_path);
    if owner.is_empty()
        || policy::is_media_intermediate_package(&owner)
        || policy::is_system_writer_package(&owner)
    {
        None
    } else {
        Some(owner)
    }
}

fn resolve_private_owner_uid(package_name: &str) -> i32 {
    policy::get_fresh_uid_for_package(package_name)
}

fn is_private_owner_fix_enabled(owner_package: &str, owner_uid: i32) -> bool {
    SettingsHub::instance().should_redirect(owner_package, owner_uid)
}

fn is_private_owner_fix_allowed_for_current_caller(owner_package: &str, owner_uid: i32) -> bool {
    let hub = InterceptHub::instance();
    let caller_uid = hub.get_current_caller_uid();
    if caller_uid >= ANDROID_APP_UID_START && caller_uid != owner_uid {
        log::debug!(
            "skip private owner fix external uid={} owner={} owner_uid={}",
            caller_uid,
            owner_package,
            owner_uid
        );
        return false;
    }

    let caller_package = hub.get_current_caller_package();
    if !caller_package.is_empty()
        && !policy::is_system_writer_package(&caller_package)
        && caller_package != owner_package
    {
        log::debug!(
            "skip private owner fix external caller={} uid={} owner={} owner_uid={}",
            caller_package,
            caller_uid,
            owner_package,
            owner_uid
        );
        return false;
    }

    true
}

fn should_allow_sqlite_private_owner_fix_for_current_caller(
    storage_path: &str,
    owner_package: &str,
    owner_uid: i32,
    user_id: i32,
) -> bool {
    if owner_package.is_empty()
        || user_id < 0
        || owner_uid < ANDROID_APP_UID_START
        || platform::user_id_from_uid(owner_uid) != user_id
        || !paths::is_sqlite_database_or_sidecar_path(storage_path)
        || paths::extract_android_private_path_owner(storage_path) != owner_package
    {
        return false;
    }

    let hub = InterceptHub::instance();
    let caller_uid = hub.get_current_caller_uid();
    let caller_package = hub.get_current_caller_package();
    if caller_uid < ANDROID_APP_UID_START {
        return false;
    }
    if caller_package.is_empty() {
        return false;
    }
    if caller_uid == owner_uid || caller_package == owner_package {
        return false;
    }
    if !caller_package.is_empty() && policy::is_system_writer_package(&caller_package) {
        return false;
    }

    log::debug!(
        "allow private owner sqlite fix cross caller={} uid={} owner={} owner_uid={} path={}",
        caller_package,
        caller_uid,
        owner_package,
        owner_uid,
        storage_path
    );
    true
}

fn path_is_same_or_child(path: &str, root: &str) -> bool {
    paths::is_same_or_child(path, root)
}

fn path_node_exists(path: &str) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };

    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    unsafe { libc::lstat(c_path.as_ptr(), st.as_mut_ptr()) == 0 }
}

fn chown_private_path_if_needed(
    path: &str,
    owner_uid: i32,
    owner_package: &str,
    private_root: &str,
) {
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    let stat_ret = unsafe { libc::lstat(c_path.as_ptr(), st.as_mut_ptr()) };
    if stat_ret != 0 {
        return;
    }
    let st = unsafe { st.assume_init() };
    if st.st_uid == owner_uid as u32 && st.st_gid == MEDIA_RW_GID {
        chmod_private_node_if_needed(&c_path, path, &st, owner_package, private_root);
        return;
    }

    let ret = unsafe { libc::lchown(c_path.as_ptr(), owner_uid as u32, MEDIA_RW_GID) };
    if ret == 0 {
        chmod_private_node_if_needed(&c_path, path, &st, owner_package, private_root);
        log::debug!(
            "system writer private owner fix path={} owner={} uid={}",
            path,
            owner_package,
            owner_uid
        );
        return;
    }

    let error_no = current_errno();
    if error_no == libc::EPERM || error_no == libc::EACCES {
        chmod_private_node_if_needed(&c_path, path, &st, owner_package, private_root);
    }
    if error_no != libc::ENOENT && error_no != libc::EPERM && error_no != libc::EACCES {
        log::warn!(
            "system writer private owner fix failed path={} owner={} uid={} errno={}",
            path,
            owner_package,
            owner_uid,
            error_no
        );
    }
}

fn chmod_private_node_if_needed(
    c_path: &CString,
    path: &str,
    st: &libc::stat,
    owner_package: &str,
    private_root: &str,
) {
    if path == private_root {
        return;
    }
    let file_type = st.st_mode & libc::S_IFMT as mode_t;
    let required_mode = match file_type {
        mode if mode == libc::S_IFDIR as mode_t => private_dir_required_mode(path),
        mode if mode == libc::S_IFREG as mode_t => PRIVATE_CHILD_FILE_REQUIRED_MODE,
        _ => return,
    };
    let current_mode = st.st_mode & 0o7777;

    if current_mode & required_mode == required_mode {
        return;
    }

    let fixed_mode = current_mode | required_mode;
    let ret = unsafe { libc::chmod(c_path.as_ptr(), fixed_mode) };
    if ret == 0 {
        log::debug!(
            "system writer private chmod fix path={} owner={} mode={:o}",
            path,
            owner_package,
            fixed_mode
        );
        return;
    }

    let error_no = current_errno();
    if error_no != libc::ENOENT && error_no != libc::EPERM && error_no != libc::EACCES {
        log::warn!(
            "system writer private chmod fix failed path={} owner={} errno={}",
            path,
            owner_package,
            error_no
        );
    }
}

fn private_dir_required_mode(path: &str) -> mode_t {
    if paths::is_default_redirect_backend_path(path) {
        STORAGE_DIR_MODE
    } else {
        PRIVATE_CHILD_DIR_REQUIRED_MODE
    }
}

fn resolve_redirect_dir_owner() -> Option<RedirectDirOwner> {
    let hub = InterceptHub::instance();
    let mut uid = hub.get_current_caller_uid();
    let mut package_name = hub.get_current_caller_package();

    if uid < ANDROID_APP_UID_START {
        let self_uid = unsafe { libc::getuid() as i32 };
        let self_package = hub.get_package_name();
        if self_uid >= ANDROID_APP_UID_START
            && !policy::is_system_writer_package(&self_package)
            && !policy::is_shared_uid_process(self_uid)
        {
            uid = self_uid;
            package_name = self_package;
        }
    }

    if uid < ANDROID_APP_UID_START {
        return None;
    }

    if package_name.is_empty() || policy::is_system_writer_package(&package_name) {
        let mut packages = policy::get_packages_for_uid(uid);
        packages.retain(|pkg| !pkg.is_empty() && !policy::is_system_writer_package(pkg));
        if packages.len() == 1 {
            package_name = packages.remove(0);
        } else {
            package_name.clear();
        }
    }

    Some(RedirectDirOwner {
        uid,
        user_id: platform::user_id_from_uid(uid),
        package_name,
    })
}

fn normalize_redirect_dir_metadata(path: &str, mode: mode_t, owner: Option<&RedirectDirOwner>) {
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    let ret = unsafe { libc::chmod(c_path.as_ptr(), mode) };
    if ret != 0 {
        log::warn!(
            "redirect dir chmod failed path={} errno={}",
            path,
            current_errno()
        );
    }

    let Some(owner) = owner else {
        return;
    };
    if !should_apply_shared_media_owner(path, owner) {
        return;
    }

    let ret = unsafe { libc::chown(c_path.as_ptr(), owner.uid as u32, MEDIA_RW_GID) };
    if ret == 0 {
        return;
    }

    let error_no = current_errno();
    if error_no == libc::EPERM || error_no == libc::EACCES {
        log::debug!(
            "redirect dir chown skipped path={} owner={} errno={}",
            path,
            owner.uid,
            error_no
        );
    } else {
        log::warn!(
            "redirect dir chown failed path={} owner={} errno={}",
            path,
            owner.uid,
            error_no
        );
    }
}

fn should_apply_shared_media_owner(path: &str, owner: &RedirectDirOwner) -> bool {
    if owner.uid < ANDROID_APP_UID_START || owner.user_id < 0 {
        return false;
    }

    let normalized = paths::normalize(path);
    let android_root = format!(
        "{}/Android/",
        paths::data_media_user_root_for_user(owner.user_id)
    );
    let Some(rest) = normalized.strip_prefix(&android_root) else {
        return false;
    };

    for category in ["data", "media", "obb"] {
        let category_prefix = format!("{}/", category);
        let Some(after_category) = rest.strip_prefix(&category_prefix) else {
            continue;
        };
        let package_name = after_category.split('/').next().unwrap_or("");
        return package_matches_owner(package_name, owner);
    }

    false
}

fn package_matches_owner(package_name: &str, owner: &RedirectDirOwner) -> bool {
    if package_name.is_empty() || policy::is_system_writer_package(package_name) {
        return false;
    }

    if !owner.package_name.is_empty() {
        return package_name == owner.package_name;
    }

    if policy::get_fresh_uid_for_package(package_name) == owner.uid {
        return true;
    }

    policy::get_packages_for_uid(owner.uid)
        .iter()
        .any(|pkg| pkg == package_name)
}

// 解析路径参数并在命中重定向时替换后调用原函数
pub fn with_redirected_path<F, R>(
    hub: &InterceptHub,
    op_name: &str,
    pathname: *const c_char,
    call_original: F,
) -> R
where
    F: FnOnce(*const c_char) -> R,
{
    if should_resolve_caller_context(hub) {
        caller::update_caller_package_for_current_thread(hub);
    }

    if pathname.is_null() || hub.is_monitor_only() {
        return call_original(pathname);
    }

    let path_text = unsafe { super::util::c_str_to_string(pathname) };
    if path_text.is_empty() {
        return call_original(pathname);
    }

    if !path_utils::is_relevant_storage_path(hub, &path_text) {
        diagnostic::record_fast_bypass(op_name, &path_text);
        return call_original(pathname);
    }

    diagnostic::log_diag_path_event(hub, op_name, "input", &path_text, -1);

    let redirect_result = process_redirect_path_for_runtime(hub, &path_text);
    diagnostic::log_diag_redirect_decision(hub, op_name, &path_text, &redirect_result);

    if redirect_result.is_redirect() {
        record_redirect_hit(hub, op_name, &path_text, &redirect_result.new_path);
        if let Ok(c_path) = CString::new(redirect_result.new_path) {
            return call_original(c_path.as_ptr());
        }
    }

    call_original(pathname)
}

fn create_parent_dirs_recursive(path: &str, mode: mode_t, owner: Option<&RedirectDirOwner>) {
    let parent = paths::parent(path);
    if parent.is_empty() || parent == "/" {
        return;
    }

    let Ok(c_parent) = CString::new(parent.clone()) else {
        return;
    };

    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    let ret = unsafe { libc::fstatat(AT_FDCWD, c_parent.as_ptr(), st.as_mut_ptr(), 0) };
    if ret == 0 {
        normalize_redirect_dir_metadata(&parent, mode, owner);
        return;
    }

    create_parent_dirs_recursive(&parent, mode, owner);

    let ret = unsafe { libc::mkdirat(AT_FDCWD, c_parent.as_ptr(), mode) };
    if ret == 0 {
        normalize_redirect_dir_metadata(&parent, mode, owner);
        log::debug!("auto mkdir parent {}", parent);
        return;
    }

    let error_no = current_errno();
    if error_no != libc::EEXIST {
        log::warn!("auto mkdir parent failed {} errno={}", parent, error_no);
    }
}

fn create_parent_dirs_recursive_until(
    path: &str,
    stop_dir: &str,
    mode: mode_t,
    owner: Option<&RedirectDirOwner>,
) {
    let parent = paths::parent(path);
    if parent.is_empty() || parent == "/" || parent == stop_dir {
        return;
    }

    let Ok(c_parent) = CString::new(parent.clone()) else {
        return;
    };

    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    let ret = unsafe { libc::fstatat(AT_FDCWD, c_parent.as_ptr(), st.as_mut_ptr(), 0) };
    if ret == 0 {
        create_parent_dirs_recursive_until(&parent, stop_dir, mode, owner);
        normalize_redirect_dir_metadata(&parent, mode, owner);
        return;
    }

    create_parent_dirs_recursive_until(&parent, stop_dir, mode, owner);

    let ret = unsafe { libc::mkdirat(AT_FDCWD, c_parent.as_ptr(), mode) };
    if ret == 0 {
        normalize_redirect_dir_metadata(&parent, mode, owner);
        log::debug!("auto mkdir storage parent {}", parent);
        return;
    }

    let error_no = current_errno();
    if error_no != libc::EEXIST {
        log::warn!(
            "auto mkdir storage parent failed {} errno={}",
            parent,
            error_no
        );
    }
}

fn process_redirect_path_for_runtime(
    hub: &crate::hook::stats::InterceptHub,
    path: &str,
) -> crate::redirect::RedirectDecision {
    process_redirect_path(hub, path)
}
