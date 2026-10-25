use super::stats::InterceptHub;
use super::{caller, context, diagnostic, monitor, path as path_utils};
use crate::config::SettingsHub;
use crate::platform::{self, fs, paths};
use crate::redirect::{policy, process_redirect_path, record_redirect_hit, writer};
use libc::{AT_FDCWD, c_char, c_void, mode_t};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};

static CALL_PREV_FALLBACK_MISS: AtomicU64 = AtomicU64::new(0);
static FORK_CHILD_HOOK_BYPASS: AtomicU64 = AtomicU64::new(0);
const ANDROID_APP_UID_START: i32 = 10000;
const MEDIA_RW_GID: u32 = 1023;
const STORAGE_DIR_MODE: mode_t = 0o2773;
/// 私有树目录的原生 MediaProvider 语义（owner→group 镜像、others 清零）。
/// auto-mkdir 曾对私有树沿用公共目标的 0o2773（others 可写可穿越），与原生
/// 02770 相悖且弱化隔离——用户 2026-10-21 反馈的微信私有目录 2773 即此来源。
const PRIVATE_TREE_DIR_MODE: mode_t = 0o2770;

/// 按路径形态派生 auto-mkdir 的目录 mode：私有树（Android/{data,media,obb}）
/// 对齐原生 02770，其余（公共重定向目标等）维持 0o2773。
fn redirect_mkdir_mode_for(path: &str, base: mode_t) -> mode_t {
    let normalized = crate::platform::paths::normalize(path);
    if let Some(rest) = normalized.strip_prefix("/data/media/") {
        let mut parts = rest.split('/');
        if let Some(user) = parts.next()
            && user.chars().all(|ch| ch.is_ascii_digit())
            && parts.next() == Some("Android")
            && matches!(parts.next(), Some("data" | "media" | "obb"))
        {
            return PRIVATE_TREE_DIR_MODE;
        }
    }
    base
}
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

    if hub.with_package_name(policy::is_system_writer_package) {
        return false;
    }

    if hub.with_package_name(policy::is_saf_native_monitor_bridge_package) {
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
        hub.with_package_name(log_fork_child_hook_bypass);
        return original_call();
    }

    let _guard = context::ReentryGuard::enter();
    hook_call(hub)
}

pub fn should_resolve_caller_context(hub: &InterceptHub) -> bool {
    if context::is_current_caller_scope_active() {
        return false;
    }
    hub.is_monitor_enabled() || hub.with_package_name(policy::is_system_writer_package)
}

// 写入类重定向时按需补齐目标父目录
pub fn ensure_redirect_parent_directory(op_name: &str, from_path: &str, to_path: &str, flags: i32) {
    if !monitor::has_write_intent_flags(flags) {
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
    if redirect_parent_chain_contains_target_cycle(path, owner.as_ref()) {
        return;
    }

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

pub fn fix_system_writer_android_private_owner(path: &str, include_path: bool, fd: i32) {
    if !crate::metadata_repair::enabled() || path.is_empty() {
        return;
    }

    let hub = InterceptHub::instance();
    if !hub.with_package_name(policy::is_system_writer_package) {
        return;
    }

    let saved_errno = current_errno();
    fix_system_writer_android_private_owner_inner(path, include_path, fd);
    fix_system_writer_mapping_target_access(path);
    set_errno(saved_errno);
}

fn fix_system_writer_mapping_target_access(path: &str) {
    let caller_package = InterceptHub::instance().get_current_caller_package();
    let caller_uid = InterceptHub::instance().get_current_caller_uid();
    if caller_package.is_empty() || caller_uid < ANDROID_APP_UID_START {
        return;
    }
    let normalized = paths::normalize(path);
    let user_id = platform::user_id_from_uid(caller_uid);
    if user_id < 0 {
        return;
    }
    let owner = SettingsHub::instance()
        .resolve_mapping_target_package_by_path_for_user(user_id, &normalized);
    if owner.is_empty() || owner != caller_package {
        return;
    }
    let target = paths::resolve_user_path(&normalized, user_id);
    let backend = paths::storage_to_data_media_for_user(&target, user_id).unwrap_or_default();
    if backend.is_empty() {
        return;
    }
    let Ok(c_path) = CString::new(backend.as_str()) else {
        return;
    };
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: c_path 是本地 CString，st 指向可写的未初始化 stat 空间，调用期间均保持有效。
    if unsafe { libc::lstat(c_path.as_ptr(), st.as_mut_ptr()) } != 0 {
        return;
    }
    // SAFETY: lstat 成功返回后，内核已完整初始化 st。
    let st = unsafe { st.assume_init() };
    if st.st_mode & libc::S_IFMT as mode_t != libc::S_IFREG as mode_t {
        return;
    }
    let mode = (st.st_mode as mode_t) & 0o7777;
    // MediaProvider 创建的目标文件通常为 0660/0664；QQ 等调用方随后仍需
    // 通过系统代写链路继续写入同一文件，因此映射目标必须具备其它用户读写位。
    let required = mode | 0o006;
    // SAFETY: c_path 仍在本地作用域内有效，required 仅包含文件权限位。
    if required != mode && unsafe { libc::chmod(c_path.as_ptr(), required) } == 0 {
        log::debug!(
            "system writer mapped target access path={} target={} mode=0{:o}",
            path,
            target,
            required
        );
    }
}

/// 映射到公共目录的文件仍可能由 MediaProvider 以 `media_rw:media_rw 0660`
/// 创建。应用通过自身 Android/data|media|obb 别名再次打开时，系统 FUSE
/// 会按底层权限检查，导致 QQ FileProvider 等进程收到 EACCES。为显式映射
/// 的私有别名补齐其它读写位，保持公共目录的 MediaStore 所有者不变。
pub fn fix_mapped_private_alias_access(hub: &InterceptHub, path: &str, flags: i32) {
    if !crate::metadata_repair::enabled() || path.is_empty() {
        return;
    }
    let package_name = hub.get_package_name();
    let private_owner = paths::extract_android_private_path_owner(path);
    if private_owner.is_empty() {
        return;
    }
    let effective_package = if private_owner == package_name {
        package_name
    } else {
        private_owner.clone()
    };
    let owner_uid = policy::get_fresh_uid_for_package(&effective_package);
    if owner_uid < ANDROID_APP_UID_START
        || !SettingsHub::instance().should_redirect(&effective_package, owner_uid)
    {
        return;
    }
    let mapped = writer::map_caller_path(path, &effective_package, owner_uid);
    if mapped.is_empty() || paths::eq_ignore_case(&mapped, path) {
        return;
    }
    let target = paths::normalize(&mapped);
    let user_id = platform::user_id_from_uid(owner_uid);
    if user_id < 0
        || !paths::is_child(&target, &paths::storage_user_root_for_user(user_id))
        || !SettingsHub::instance().is_public_mapping_target_path_for_user(user_id, &target)
    {
        return;
    }
    let backend = paths::storage_to_data_media_for_user(&target, user_id).unwrap_or_default();
    if backend.is_empty() {
        return;
    }
    let Ok(c_path) = CString::new(backend.as_str()) else {
        return;
    };
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: c_path 是本地 CString，st 指向可写的未初始化 stat 空间，调用期间均保持有效。
    if unsafe { libc::lstat(c_path.as_ptr(), st.as_mut_ptr()) } != 0 {
        return;
    }
    // SAFETY: lstat 成功返回后，内核已完整初始化 st。
    let st = unsafe { st.assume_init() };
    if st.st_mode & libc::S_IFMT as mode_t != libc::S_IFREG as mode_t {
        return;
    }
    let mode = (st.st_mode as mode_t) & 0o7777;
    let write_intent = flags & (libc::O_WRONLY | libc::O_RDWR | libc::O_TRUNC) != 0;
    let required = mode | 0o004 | if write_intent { 0o002 } else { 0 };
    if required == mode {
        return;
    }
    // SAFETY: c_path 仍在本地作用域内有效，required 仅包含文件权限位。
    if unsafe { libc::chmod(c_path.as_ptr(), required) } == 0 {
        log::debug!(
            "mapped private alias access path={} target={} mode=0{:o}",
            path,
            target,
            required
        );
    }
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

fn fix_system_writer_android_private_owner_inner(path: &str, include_path: bool, fd: i32) {
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

    // 不再要求 owner 配置了重定向：A17 原生 FUSE 对属主非本应用的
    // Android/{data,media,obb}/<pkg> 只给 others 位，该语义与是否开启重定向
    // 无关——未配置应用（真机 2026-10-18 反馈的 Unity 存档）被系统代写污染后
    // 同样不可读写，而挂载应答期修复只覆盖已配置应用，未配置应用此前仅剩
    // 30 分钟普查兜底。这里在代写创建的当下把属主即时修回应用 uid，是污染
    // 源头治理；uid 解析失败与跨 caller 保护仍然生效。
    let allow_cross_caller_sqlite = should_allow_sqlite_private_owner_fix_for_current_caller(
        &storage_path,
        &owner_package,
        owner_uid,
        user_id,
    );
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
        chown_private_path_if_needed(&parent, owner_uid, &owner_package, &private_root, -1);
    }

    if should_fix_private_path_node(&backend_path, &private_root, include_path) {
        chown_private_path_if_needed(&backend_path, owner_uid, &owner_package, &private_root, fd);
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
    fd: i32,
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
    // owner 已经是应用 UID 时，gid 可能是 ext_data_rw（1078）或 media_rw
    //（1023），两者都属于系统管理的原生私有目录语义，不能因为 hook 偏好
    // media_rw 就覆盖已有组。
    if st.st_uid == owner_uid as u32 {
        chmod_private_node_if_needed(&c_path, path, &st, owner_package, private_root);
        return;
    }

    // 只修复 owner，保留 MediaProvider 为该层选择的 gid。companion_fchown 同样
    // 使用 gid=-1，因此进程内成功与 root companion 回退的结果保持一致。
    // SAFETY: c_path 由 CString 保证以 NUL 结尾；gid=u32::MAX 明确表示保留原组。
    let ret = unsafe { libc::lchown(c_path.as_ptr(), owner_uid as u32, u32::MAX) };
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
        // 进程内 lchown 被 EPERM 拒绝（MP 等普通 uid 无 CAP_CHOWN，真机 2026-10-18
        // 实证 media.module 全链 denied）：经 companion（root）对 open fd 即时
        // fchown——fd 在应用拿到之前已是正确属主，零竞态零扫描。
        if fd >= 0 && crate::hook::companion_api::companion_fchown(fd, owner_uid) {
            return;
        }
        // 目录（mkdir/mkdirat 钩子不持有 fd）此前在此处静默失败：无 CAP_CHOWN
        // 的代写进程对私有树目录的属主修复全部落空，累积成 uid=1023/2773 的
        // 漂移目录（2026-10-21 用户反馈）。就地打开一个只读 fd 交给 companion
        // fchown，补上「目录无 fd 通道」的结构性缺口；文件路径 O_DIRECTORY
        // 打开会 ENOTDIR，回退普通只读打开再试一次。
        if fd < 0
            && open_fd_for_owner_fix(path, true)
                .or_else(|| open_fd_for_owner_fix(path, false))
                .is_some_and(|opened| {
                    let fixed = crate::hook::companion_api::companion_fchown(opened, owner_uid);
                    // SAFETY: opened 是刚打开的合法 fd，close 不借用其它数据。
                    unsafe { libc::close(opened) };
                    fixed
                })
        {
            return;
        }
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

/// 为无 fd 的属主修复目标打开一个只读 fd（目录用 O_DIRECTORY，文件回退普通打开）。
fn open_fd_for_owner_fix(path: &str, directory: bool) -> Option<libc::c_int> {
    let mut flags = libc::O_RDONLY | libc::O_CLOEXEC;
    if directory {
        flags |= libc::O_DIRECTORY;
    }
    let c_path = CString::new(path).ok()?;
    // SAFETY: c_path 以 NUL 结尾且在调用期间保持有效，open 只返回新 fd。
    let opened = unsafe { libc::open(c_path.as_ptr(), flags) };
    (opened >= 0).then_some(opened)
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

/// 检测重定向父链是否嵌入了调用方自身重定向目标的重复段。
///
/// 背景：一旦某条改写决策把「目标显示形态」当作普通路径再次改写（历史回归见
/// readlink 反解窗口），结果路径里目标段 `Android/data/<pkg>/sdcard` 会出现两次；
/// 父链的 mkdir -p 会把中间层全部物理创建，MediaStore 每轮往返再深一层，测试
/// 应用私有树一晚长出 284 层空目录，监视树重建随之被拖死。正常改写结果里
/// 目标段只会作为前缀出现一次；出现两次即目标自嵌套，此时继续创建父链只会
/// 繁殖垃圾目录——跳过创建，让操作以 ENOENT 自然失败，并留下可检索的证据日志。
fn redirect_parent_chain_contains_target_cycle(
    path: &str,
    owner: Option<&RedirectDirOwner>,
) -> bool {
    let Some(owner) = owner else {
        return false;
    };
    if owner.package_name.is_empty() {
        return false;
    }
    let target = writer::resolve_system_writer_redirect_target(
        &owner.package_name,
        owner.uid,
        owner.user_id,
        false,
    );
    if target.is_empty() {
        return false;
    }
    let storage_root = paths::storage_user_root_for_user(owner.user_id);
    let Some(target_segment) = target.strip_prefix(&storage_root) else {
        return false;
    };
    let target_segment = target_segment.trim_matches('/');
    if target_segment.is_empty() {
        return false;
    }
    let occurrences = path.matches(target_segment).count();
    if occurrences >= 2 {
        log::warn!(
            "redirect parent chain cycle suspected: target segment {} occurs {} times path={}",
            target_segment,
            occurrences,
            path
        );
        return true;
    }
    false
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
    if !crate::metadata_repair::enabled() {
        return;
    }
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    let mode = redirect_mkdir_mode_for(path, mode);
    let ret = unsafe { libc::chmod(c_path.as_ptr(), mode) };
    if ret != 0 {
        let error_no = current_errno();
        // 共享存储的可见路径 `/storage/emulated/<user>` 由系统 MediaProvider 的 FUSE
        // 承载，`/data/media/<user>/Android/data` 等目录由 media_rw 持有，应用进程对
        // 它们 chmod 只会得到 EPERM/EACCES/EROFS。这类结果与下方 chown 的预期失败一致，
        // 降级为 debug，避免系统盘符噪音掩盖真正的元数据修复失败。
        if matches!(error_no, libc::EPERM | libc::EACCES | libc::EROFS) {
            log::debug!(
                "redirect dir chmod skipped path={} errno={}",
                path,
                error_no
            );
        } else {
            log::warn!("redirect dir chmod failed path={} errno={}", path, error_no);
        }
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
    // MediaStore 提交阶段对 .pending- 文件的存在性检查决定了改名能否找到源文件，
    // 而常规决策日志按操作类型采样输出，stat 这类高频操作几乎不会命中。这里对
    // pending 形态单独无条件记录一次：该形态仅在 MediaStore 提交时出现，频率极低。
    if paths::media_store_pending_display_path(&path_text).is_some() {
        log::info!(
            "pending probe op={} path={} action={:?} new={}",
            op_name,
            path_text,
            redirect_result.action,
            redirect_result.new_path
        );
    }

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

    // SAFETY: c_parent 以 NUL 结尾并在调用期间保持有效；mode 由路径形态派生。
    let ret = unsafe {
        libc::mkdirat(
            AT_FDCWD,
            c_parent.as_ptr(),
            redirect_mkdir_mode_for(&parent, mode),
        )
    };
    if ret == 0 {
        normalize_redirect_dir_metadata(&parent, mode, owner);
        // 自动补建的父目录逐级交由系统代写属主治理：链中间层不经过 open/mkdir
        // 钩子触发面，漏掉就会残留 media_rw 属主（uid=1023/2773 签名，2026-10-21
        // 用户反馈）。治理入口自带系统代写进程门控与 errno 保存，应用进程内零开销。
        fix_system_writer_android_private_owner(&parent, true, -1);
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

    // SAFETY: c_parent 以 NUL 结尾并在调用期间保持有效；mode 由路径形态派生。
    let ret = unsafe {
        libc::mkdirat(
            AT_FDCWD,
            c_parent.as_ptr(),
            redirect_mkdir_mode_for(&parent, mode),
        )
    };
    if ret == 0 {
        normalize_redirect_dir_metadata(&parent, mode, owner);
        // 同 create_parent_dirs_recursive：存储父链每一级都要过属主治理。
        fix_system_writer_android_private_owner(&parent, true, -1);
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
