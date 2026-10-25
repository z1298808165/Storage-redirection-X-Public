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
    let (normalized_path, owner_package, user_id) = resolve_private_owner_sqlite_path(path)?;

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
    let (normalized_path, owner_package, user_id) = resolve_private_owner_sqlite_path(path)?;

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

/// 放行应用访问其自身的 Android 私有外部存储目录。
///
/// MediaProvider 的 app-data isolation FUSE 层可能拒绝所有者对包目录的直接文件操作。
/// 此例外严格限定为路径所属包与调用 UID 完全匹配，且处于同一 Android 用户。
pub fn should_allow_own_android_private_path_access(path: &str, caller_uid: i32) -> bool {
    if caller_uid < writer::ANDROID_APP_UID_START {
        return false;
    }

    let normalized_path = normalize_storage_path(path);
    if normalized_path.is_empty() {
        return false;
    }

    let user_id = paths::extract_user_id_from_storage_path(&normalized_path);
    if user_id < 0 || platform::user_id_from_uid(caller_uid) != user_id {
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
    // `Android/media/<pkg>` 恢复原生共享语义：媒体目录按设计就是跨应用共享
    // （issue #2：Sen 这类插件把配置放在宿主应用的 media 目录、由不同 uid 的模块
    // 进程读取；A17 的 MediaProvider 收紧后原生放行消失，模块直接打不开配置）。
    // 这里对同 user 的应用 uid 放行整个 media 子树；`Android/{data,obb}` 仍是应用
    // 私有语义，跨 UID 访问维持 owner-only，走配置化的映射/允许路径。
    let media_prefix = format!("/Android/media/{}/", owner_package);
    let media_root = format!("/Android/media/{}", owner_package);
    let is_media_subtree =
        normalized_path.contains(&media_prefix) || normalized_path.ends_with(&media_root);
    let allowed = owner_uid >= writer::ANDROID_APP_UID_START
        && (owner_uid == caller_uid
            || (is_media_subtree && caller_uid >= writer::ANDROID_APP_UID_START));
    log::debug!(
        "media fuse own private path decision allowed={} owner_only={} media_subtree={} caller_uid={} user_id={} owner={} owner_uid={} path={}",
        allowed,
        owner_uid == caller_uid,
        is_media_subtree,
        caller_uid,
        user_id,
        owner_package,
        owner_uid,
        normalized_path
    );
    allowed
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

/// sqlite `-shm` 边车的尺寸级毒化判定（内容级 salt 校验由
/// [`sqlite_shm_wal_index_stale`] 负责）。合法尺寸只有 0 与 ≥32KiB 两种：
/// 0 字节是首个打开者以 O_CREAT 建出的合法初生态，SQLite 会依据同级 `-wal`
/// 重建 wal-index；非零且不足一个 wal-index 区域（真机实测：3 字节全零）
/// 才是毒化态。若把 0 字节也判毒化删除，会形成「见 0 删 0 → 后续重定向
/// open 恒 ENOENT」的死循环，应用侧 SQLITE_CANTOPEN 无法自愈（2026-10-15
/// XRadiant 真机实测）。判定须与 fuse_redirect 侧
/// `heal_poisoned_sqlite_shm_backend`（`fuse_redirect/callbacks.rs`）保持一致。
fn is_poisoned_sqlite_shm_size(size: libc::off_t) -> bool {
    size > 0 && size < SQLITE_SHM_MIN_SIZE
}

fn prepare_sqlite_shm_backend(storage_path: &str, backend_path: &str) -> bool {
    let Ok(c_path) = CString::new(backend_path) else {
        return false;
    };

    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if fd < 0 {
        let errno = current_errno();
        if errno == libc::ENOENT {
            // 全新配置目录：`-shm` 尚未由 MediaProvider 的 O_CREAT 创建，没有毒化
            // 可言（issue #2 日志里 216 条 warn 全是它，把真信号盖掉了）。放行给
            // 原生链路去创建即可。
            log::debug!(
                "prepare sqlite shm backend absent, skip poison check storage={} backend={}",
                storage_path,
                backend_path
            );
            return false;
        }
        log::warn!(
            "prepare sqlite shm backend open failed errno={} storage={} backend={}",
            errno,
            storage_path,
            backend_path
        );
        return false;
    }

    let size = fd_size(fd);
    // SAFETY: fd 是 prepare_sqlite_shm_backend 上方成功 open 返回的合法描述符，
    // 此处仅关闭一次，关闭后不再使用。
    unsafe {
        libc::close(fd);
    }
    if !is_poisoned_sqlite_shm_size(size) {
        if size == 0 {
            // 合法初生态：放行，SQLite 会按 `-wal` 重建 wal-index。
            return true;
        }
        // 满尺寸也可能毒化：wal-index 头 salt 与同级 `-wal` 文件头失配（跨视图
        // inode 分裂、异常会话残留）。SQLite 信任已初始化的 shm 头而不复查 `-wal`
        // 文件，会把未 checkpoint 的 WAL 数据整体判无效并静默回滚到旧检查点
        // （跨应用共享库掉登录），随后的写入覆写 WAL 造成不可逆数据丢失。删除后
        // SQLite 按 `-wal` 重建 wal-index，对任何消费方都无损。
        if !sqlite_shm_wal_index_stale(std::path::Path::new(backend_path)) {
            return true;
        }
    }

    // 走到这里的 sidecar 都是毒化态：非零且小于一个完整 wal-index 页框的 inode
    // （真机实测 3 字节全零），或满尺寸但 wal-index 与 `-wal` 头 salt 失配。就地
    // 扩回尺寸不能恢复 SQLite 的共享内存头，必须先让 MediaProvider 失效视图节点，
    // 再删除真实后端文件，下一次 O_CREAT 才会拿到新 inode。
    let removed_storage = unlink_path(storage_path);
    let removed_backend = if std::fs::metadata(backend_path).is_ok() {
        unlink_path(backend_path)
    } else {
        false
    };
    if removed_storage || removed_backend {
        log::info!(
            "heal sqlite shm backend removed storage={} backend={} size={} path={} backend_path={}",
            removed_storage,
            removed_backend,
            size,
            storage_path,
            backend_path
        );
        return true;
    }
    log::warn!(
        "heal sqlite shm backend unlink failed size={} path={} backend_path={}",
        size,
        storage_path,
        backend_path
    );
    false
}

/// 由 `-shm` 边车路径推出同级 `-wal` 路径：后缀按小写不敏感识别，替换为小写
/// `-wal`，库名主干保持原样。
fn sqlite_sidecar_wal_path(shm_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let file_name = shm_path.file_name()?.to_str()?;
    let lower = file_name.to_ascii_lowercase();
    let stem_len = lower.len().checked_sub("-shm".len())?;
    if stem_len == 0 || !lower.ends_with("-shm") {
        return None;
    }
    let mut wal_path = shm_path.to_path_buf();
    wal_path.set_file_name(std::ffi::OsString::from(format!(
        "{}-wal",
        &file_name[..stem_len]
    )));
    Some(wal_path)
}

/// 读取文件前 `len` 字节；文件不足 `len` 字节或不可读时返回 `None`。
fn read_file_prefix(path: &std::path::Path, len: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// 满尺寸 `-shm` 的内容级毒化判定：wal-index 头与同级 `-wal` 文件头 salt 失配。
///
/// SQLite 恢复 wal-index 时按 `memcpy` 原样拷贝 `-wal` 文件头（大端字节模式）的
/// 8 字节 salt 到 wal-index 头偏移 32..40，不做字节序转换，因此毒化判定用原始
/// 字节比对即可。两者失配说明 wal-index 停留在旧一代 WAL 或来自分裂视图。
/// `-shm` 头未初始化（isInit≠1）或 `-wal` 头缺失/魔数非法时 SQLite 自己会重建，
/// 一律不判定为毒化，保持保守。与 `fuse_redirect::callbacks` 的同名判定保持
/// 同一逻辑（该模块不依赖 hook 模块，无法共享实现）。
fn sqlite_shm_wal_index_stale(real_shm_path: &std::path::Path) -> bool {
    let Some(wal_path) = sqlite_sidecar_wal_path(real_shm_path) else {
        return false;
    };
    let Some(shm) = read_file_prefix(real_shm_path, 48) else {
        return false;
    };
    if shm[12] != 1 {
        return false;
    }
    let Some(wal) = read_file_prefix(&wal_path, 32) else {
        return false;
    };
    let wal_magic = u32::from_be_bytes([wal[0], wal[1], wal[2], wal[3]]);
    if wal_magic != 0x377f_0682 && wal_magic != 0x377f_0683 {
        return false;
    }
    shm[32..40] != wal[16..24]
}

fn unlink_path(path: &str) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    // SAFETY: c_path 以 NUL 结尾，并在系统调用期间保持有效。
    unsafe { libc::syscall(libc::SYS_unlinkat, libc::AT_FDCWD, c_path.as_ptr(), 0) == 0 }
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
    // FUSE 回调在 MediaProvider 请求线程内运行。此路径仅操作内存：
    // 在这里刷新 UID 缓存可能阻塞 FUSE 服务端。
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
    let relative_lower = relative.to_ascii_lowercase();
    caller_package
        .rsplit('.')
        .any(|token| is_distinctive_package_path_token(token, &relative_lower))
}

fn is_distinctive_package_path_token(token: &str, relative_path_lower: &str) -> bool {
    let token = token.trim();
    token.len() >= 4
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
        && relative_path_lower.contains(&token.to_ascii_lowercase())
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

    struct RecentAccessGuard(VecDeque<RecentPrivateOwnerSqliteAccess>);

    impl RecentAccessGuard {
        fn take() -> Self {
            let mut accesses = RECENT_PRIVATE_OWNER_SQLITE_ACCESS
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            Self(std::mem::take(&mut *accesses))
        }
    }

    impl Drop for RecentAccessGuard {
        fn drop(&mut self) {
            let mut accesses = RECENT_PRIVATE_OWNER_SQLITE_ACCESS
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            *accesses = std::mem::take(&mut self.0);
        }
    }

    #[test]
    fn caller_package_token_matches_xradiant_private_sqlite() {
        assert!(private_owner_sqlite_path_mentions_caller_package(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
            "com.eg.android.AlipayGphone",
            "com.leo.xposed.xradiant",
        ));
    }

    #[test]
    fn unrelated_package_token_does_not_match_generic_sqlite() {
        assert!(!private_owner_sqlite_path_mentions_caller_package(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/cache/app.db-shm",
            "com.eg.android.AlipayGphone",
            "com.leo.xposed.xradiant",
        ));
    }

    #[test]
    fn zero_size_shm_is_not_poisoned() {
        // 0 字节是 O_CREAT 初生态，删除会引发「见 0 删 0」死循环。
        assert!(!is_poisoned_sqlite_shm_size(0));
    }

    #[test]
    fn full_size_shm_size_is_not_poisoned() {
        assert!(!is_poisoned_sqlite_shm_size(32 * 1024));
        assert!(!is_poisoned_sqlite_shm_size(64 * 1024));
    }

    #[test]
    fn partial_shm_size_is_poisoned() {
        // 真机实测样例：XRadiant 的 XRadiant.db-shm 变成 3 字节全零。
        assert!(is_poisoned_sqlite_shm_size(3));
        assert!(is_poisoned_sqlite_shm_size(32 * 1024 - 1));
    }

    #[test]
    fn recent_sqlite_access_keeps_owner_caller_for_backend_retry() {
        let path = "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm";
        let owner = "com.eg.android.AlipayGphone";
        let caller_uid = 10164;

        let _guard = RecentAccessGuard::take();
        remember_recent_private_owner_sqlite_access(
            path,
            owner,
            caller_uid,
            &["com.leo.xposed.xradiant".to_string()],
        );

        assert_eq!(
            has_recent_private_owner_sqlite_access(path),
            Some(caller_uid)
        );
    }
}
