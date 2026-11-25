use super::context;
use super::stats::InterceptHub;
use crate::config::SettingsHub;
use crate::monitor::{AuditTrail, OpKind};
use crate::platform::{self, paths};
use crate::redirect::{policy, writer};
use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::{LazyLock, Mutex};

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

static RECENT_PRIVATE_OWNER_SQLITE_ACCESS: LazyLock<
    Mutex<VecDeque<RecentPrivateOwnerSqliteAccess>>,
> = LazyLock::new(|| Mutex::new(VecDeque::new()));

pub fn record_read_only_fuse_operation(
    kind_code: i32,
    op_name: &str,
    op_filter: &str,
    path: &str,
    from_path: &str,
    caller_uid: i32,
    flags: i32,
) -> bool {
    if context::is_caller_attribution_stopped()
        || context::is_current_caller_scope_active()
            && InterceptHub::instance()
                .get_current_caller_package()
                .is_empty()
    {
        return false;
    }
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
    } else if context::is_current_caller_scope_active() {
        return false;
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
        extra.push_str(&crate::platform::log_fields::encode(&normalized_from));
    }
    extra.push_str("|source=");
    extra.push_str(source);
    extra.push('|');
    extra.push_str(READ_ONLY_DENY_REASON);

    AuditTrail::instance().record_operation_result(
        kind,
        &caller_package,
        record_path,
        (-1, libc::EROFS),
        &extra,
        (
            caller_uid,
            context::get_effective_caller_pid_for_monitor(),
            None,
        ),
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

    let owner_uid = resolve_private_owner_uid(&owner_package, user_id);
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

    let owner_uid = resolve_private_owner_uid(&owner_package, user_id);
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

    let owner_uid = resolve_private_owner_uid(&owner_package, user_id);
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

    // 与 [`prepare_private_owner_sqlite_sidecar`] 同一判据：MediaProvider 打不开的
    // 入口不值得拉回用户态（拉了也只会把 EACCES 透传给应用），保持原生直通。
    let Some(backend_path) = sqlite_backend_path(&normalized_path) else {
        return false;
    };
    if !media_provider_can_open_backend(&backend_path) {
        log::debug!(
            "media fuse keep passthrough for unreadable private owner sqlite owner={} path={}",
            owner_package,
            normalized_path
        );
        return false;
    }

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

/// 判定本次 open 是否**必须**由 MediaProvider 用户态接管，返回 true 表示需要清掉
/// FUSE 直通。
///
/// 只有两种情况值得把原生直通摘掉：
/// 1. 本次发现 `-shm` 边车毒化并已删除——必须让 MP 重新解析路径，由应用随后的
///    `O_CREAT` 拿到新 inode（沿用既有删除式自愈链）；
/// 2. 该后端文件 MP 自己打得开（组位齐全，即模块与 MP 原生创建口径产出的常规入口）
///    ——用户态接管才可能真正承担这次 IO，拦截与自愈也才有意义。
///
/// 其余情况（健康但 owner-only、不可判定）一律保持直通：MP 的 FuseDaemon 用自己的
/// 身份打开下层文件，owner-only 入口它必然 EACCES，硬拉回用户态等于把 EACCES 透传
/// 给应用（真机 2026-11-04：XRadiant.db-wal/-shm 变 0600 后 SQLITE_CANTOPEN）。
pub fn prepare_private_owner_sqlite_sidecar(path: &str) -> bool {
    let Some((normalized_path, owner_package, user_id)) = resolve_private_owner_sqlite_path(path)
    else {
        return false;
    };

    if !should_force_private_owner_sqlite_for_owner(&owner_package, user_id) {
        return false;
    }

    let Some(backend_path) = sqlite_backend_path(&normalized_path) else {
        return false;
    };

    // 边车名字必须始终可解析：`-wal`/`-journal`/`-shm` 缺失时先补 0 字节同名文件。
    // 不依赖 WAL：rollback journal 模式的库没有 `-shm`/`-wal`，但它同样会因为
    // 「后端被绕过视图删除 → MP 记成不可创建」而卡在 `-journal` 的 O_CREAT 上。
    //
    // 覆盖前提（有意为之）：本函数只在 `should_force_private_owner_sqlite_for_owner`
    // 命中时被调用，也就是「跨属主/共享库」这一子集。模块自己制造的名字缺失只发生在
    // `-shm` 毒化自愈上，而它同样只针对该子集，所以覆盖面与风险面一致。若将来有别的
    // 路径也开始在后端删除边车，必须把补位提到这个闸门之前。
    ensure_empty_sqlite_sidecar(&backend_path);

    if is_sqlite_shm_sidecar_path(&normalized_path)
        && prepare_sqlite_shm_backend(&normalized_path, &backend_path)
    {
        return true;
    }

    media_provider_can_open_backend(&backend_path)
}

/// 存储视图路径换算成 MediaProvider FuseDaemon 实际打开的后端（f2fs）路径。
fn sqlite_backend_path(normalized_path: &str) -> Option<String> {
    let backend_path = writer::storage_to_data_media_path(normalized_path);
    if backend_path == normalized_path || !backend_path.starts_with("/data/media/") {
        return None;
    }
    Some(backend_path)
}

/// MediaProvider 能否以**自身身份**打开这个后端文件。
///
/// MP 的 FuseDaemon 用自己进程的身份打开下层文件（AOSP `pf_open`：
/// `open(io_path, open_info.flags)`，直通只是把这个已打开的 fd 交给内核），
/// 而 MP 没有 CAP_SETUID / CAP_DAC_OVERRIDE（真机 `CapEff: 0`，附加组只有
/// media_rw(1023)/ext_data_rw(1078) 一类）。因此「组位」就是 MP 的可服务性：
/// 有组位能开，owner-only 0600 一律 EACCES。ENOENT 视为可服务（文件尚未创建，
/// MP 的 O_CREAT 路径能建出来）。
fn media_provider_can_open_backend(backend_path: &str) -> bool {
    let Ok(c_path) = CString::new(backend_path) else {
        return false;
    };
    // SAFETY: c_path 以 NUL 结尾且在调用期间保持有效；open 只返回新 fd，不写内存。
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if fd < 0 {
        let errno = current_errno();
        if errno == libc::ENOENT {
            return true;
        }
        return false;
    }
    // SAFETY: fd 是上方成功 open 返回的合法描述符，此处仅关闭一次。
    unsafe {
        libc::close(fd);
    }
    true
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

/// 检查并自愈 `-shm` 后端毒化，返回 true 表示**已删除**毒化边车。
///
/// 尺寸判定走 `O_PATH`+`fstat`：`O_PATH` 不需要读权限，owner-only 0600 的 `-shm`
/// 也能拿到大小。此前这里用 `O_RDWR` 打开，MP 身份对 owner-only 入口必然 errno=13，
/// 于是整条 MP 侧毒化自愈链在「应用自建 owner-only 边车」的形态下静默失效
/// （真机 2026-11-04：`prepare sqlite shm backend open failed errno=13`）。
fn prepare_sqlite_shm_backend(storage_path: &str, backend_path: &str) -> bool {
    let Some(size) = backend_file_size(backend_path) else {
        // ENOENT：可能只是全新配置目录（边车尚未创建），也可能是 MediaProvider 把
        // 这个名字记成了不可创建——后端被绕过视图删除后，同名 `O_CREAT` 会一直返回
        // ENOENT（2026-11-25 真机复现）。此时 `media_provider_can_open_backend` 里
        // 「ENOENT 说明尚未创建、MP 能建出来」的假设不再成立：调用方会保持直通，应用
        // 却永远建不出边车。所以这里补一个 0 字节同名文件：名字立刻可解析，SQLite
        // 随后按需初始化 wal-index / journal。
        let ensured = ensure_empty_sqlite_sidecar(backend_path);
        log::debug!(
            "prepare sqlite shm backend absent or unreadable ensured={} storage={} backend={}",
            ensured,
            storage_path,
            backend_path
        );
        return false;
    };

    if size == 0 {
        // 合法初生态：放行，SQLite 会按 `-wal` 重建 wal-index。
        return false;
    }

    if !is_poisoned_sqlite_shm_size(size) {
        // 满尺寸也可能毒化：wal-index 头 salt 与同级 `-wal` 文件头失配（跨视图
        // inode 分裂、异常会话残留）。SQLite 信任已初始化的 shm 头而不复查 `-wal`
        // 文件，会把未 checkpoint 的 WAL 数据整体判无效并静默回滚到旧检查点
        // （跨应用共享库掉登录），随后的写入覆写 WAL 造成不可逆数据丢失。删除后
        // SQLite 按 `-wal` 重建 wal-index，对任何消费方都无损。
        //
        // owner-only 入口读不到内容时该判定返回 false（保守不删），不额外告警：
        // 这类入口本来就不该由 MP 承担 IO。
        if !sqlite_shm_wal_index_stale(std::path::Path::new(backend_path)) {
            return false;
        }
    }

    // 走到这里的 sidecar 都是毒化态：非零且小于一个完整 wal-index 页框的 inode
    // （真机实测 3 字节全零），或满尺寸但 wal-index 与 `-wal` 头 salt 失配。就地
    // 扩回尺寸不能恢复 SQLite 的共享内存头，必须先让 MediaProvider 失效视图节点，
    // 再删除真实后端文件，下一次 O_CREAT 才会拿到新 inode。
    // 同一时刻只允许一个自愈在跑：MP 的 FUSE 是多线程的，两次并发 open 可能各自走到
    // 「先删后补」，后到的那个会把先到者刚补出的 0 字节文件再删掉，反而制造名字缺失。
    // `try_lock` 抢不到就跳过本轮——文件仍处于毒化态，下一次 open 会再判一次。
    static HEAL_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_heal_guard) = HEAL_GUARD.try_lock() else {
        return false;
    };

    // 删除前记录身份：补空文件时尽量还原，避免应用侧因 owner 变化被 DAC 拒绝。
    let attrs = std::fs::metadata(backend_path).ok().map(|metadata| {
        use std::os::unix::fs::MetadataExt as _;
        (metadata.uid(), metadata.gid(), metadata.mode())
    });
    let removed_storage = unlink_path(storage_path);
    let removed_backend = if std::fs::metadata(backend_path).is_ok() {
        unlink_path(backend_path)
    } else {
        false
    };
    if removed_storage || removed_backend {
        // 与 `fuse_redirect::callbacks::heal_poisoned_sqlite_shm_backend` 同一不变量：
        // 删除毒化内容后立刻补 0 字节同名文件（SQLite 合法初生态），让名称始终可解析。
        // 否则 MediaProvider 会把这个名字记成不可创建，后续同名 `O_CREAT` 恒 ENOENT。
        let placeholder = attrs.is_some_and(|(uid, gid, mode)| {
            recreate_empty_sqlite_shm(backend_path, uid, gid, mode)
        });
        log::info!(
            "heal sqlite shm backend removed storage={} backend={} placeholder={} size={} path={} backend_path={}",
            removed_storage,
            removed_backend,
            placeholder,
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

/// 后端文件大小；不存在或不可 `O_PATH` 打开时返回 `None`。
fn backend_file_size(backend_path: &str) -> Option<libc::off_t> {
    let c_path = CString::new(backend_path).ok()?;
    // SAFETY: c_path 以 NUL 结尾且在调用期间保持有效；O_PATH 只建立路径引用，
    // 不要求读权限，也不触发任何数据访问。
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return None;
    }
    let size = fd_size(fd);
    // SAFETY: fd 为上方 open 得到的合法描述符，此处仅关闭一次。
    unsafe {
        libc::close(fd);
    }
    (size >= 0).then_some(size)
}

/// 满尺寸 `-shm` 的内容级毒化判定。
///
/// 实现收敛在 [`crate::platform::sqlite_sidecar::wal_index_stale`]（daemon 与 hook
/// 共用同一份判定，含「两次快照 + 身份稳定」的换代复核），避免两侧再各写一份而分叉。
fn sqlite_shm_wal_index_stale(real_shm_path: &std::path::Path) -> bool {
    crate::platform::sqlite_sidecar::wal_index_stale(real_shm_path)
}

fn unlink_path(path: &str) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    // SAFETY: c_path 以 NUL 结尾，并在系统调用期间保持有效。
    unsafe { libc::syscall(libc::SYS_unlinkat, libc::AT_FDCWD, c_path.as_ptr(), 0) == 0 }
}

/// 删除毒化 `-shm` 后补一个空的同名文件（0 字节 = SQLite 合法初生态）。
///
/// 目的不是恢复内容，而是保住名字：MediaProvider 对「后端被绕过视图删除」的名称会记成
/// 不可创建，同名 `O_CREAT` 恒 `ENOENT`（2026-11-25 真机复现：后端 `rm` 后同名创建失败、
/// 同目录新名字成功，重启 MP 才恢复）。0 字节边车由 SQLite 首次打开时按 `-wal` 初始化，
/// 与原先「删掉等应用重建」语义等价，但不留幽灵负节点。
///
/// MP 进程没有 `CAP_CHOWN`，还原属主可能失败；此时保留 MP 自建的文件本身是可接受的
/// （本 ROM 上 MP 代写的 `Android/media` 文件正常就是 MP 属主，应用经 MP FUSE 仍可打开）。
fn recreate_empty_sqlite_shm(path: &str, uid: u32, gid: u32, mode: u32) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    // SAFETY: c_path 以 NUL 结尾且在调用期间保持有效；O_EXCL 不覆盖并发创建的同名文件，
    // O_NOFOLLOW 避免被符号链接带偏。
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            (mode & 0o777) as libc::mode_t,
        )
    };
    if fd < 0 {
        // EEXIST：应用或另一个 heal 已经补上，名字在场即达到目的。
        return current_errno() == libc::EEXIST;
    }
    // SAFETY: fd 是上方 open 得到的合法描述符，本函数独占，随后只关闭一次。
    unsafe {
        libc::fchown(fd, uid, gid);
        libc::fchmod(fd, (mode & 0o7777) as libc::mode_t);
        libc::close(fd);
    }
    true
}

/// 缺失的 SQLite 边车（`-wal` / `-journal` / `-shm`）补一个 0 字节同名文件。
///
/// 0 字节对这三种边车都是合法初生态：`-wal` 是空日志、`-journal` 表示没有热日志、
/// `-shm` 是未初始化的 wal-index。`.db` 不在其列——空库与空边车语义完全不同，绝不补。
///
/// 触发条件要求同级 `.db` 存在：只有「库在、边车不在」才说明这是被删掉的边车，而不是
/// 一个尚未建库的全新目录。属主保持 MediaProvider 自建口径（0660/media_rw）；MP 视图
/// 不做 DAC 判定，模块视图也不启用 `default_permissions`，两边都能正常打开。
fn ensure_empty_sqlite_sidecar(backend_path: &str) -> bool {
    let path = std::path::Path::new(backend_path);
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    let Some(stem) = ["-shm", "-wal", "-journal"]
        .iter()
        .find_map(|suffix| lower.strip_suffix(suffix))
        .filter(|stem| {
            let stem = stem.to_ascii_lowercase();
            stem.ends_with(".db") || stem.ends_with(".sqlite") || stem.ends_with(".sqlite3")
        })
    else {
        return false;
    };
    // 主干必须是真实存在的库文件：只有「库在、边车不在」才是被删掉的边车。
    let db_name = &name[..stem.len()];
    if !path.with_file_name(db_name).is_file() {
        return false;
    }
    // u32::MAX 落到 fchown 即 -1：不改属主，只落文件与权限位。
    recreate_empty_sqlite_shm(backend_path, u32::MAX, u32::MAX, 0o660)
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

fn resolve_private_owner_uid(package_name: &str, user_id: i32) -> i32 {
    // FUSE 回调在 MediaProvider 请求线程内运行。此路径仅操作内存：
    // 在这里刷新 UID 缓存可能阻塞 FUSE 服务端。
    policy::get_uid_for_package_in_user(package_name, user_id)
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

    if let [caller_package] = caller_packages
        && is_private_owner_sqlite_caller_package(caller_package, owner_package)
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

    let hub = InterceptHub::instance();
    let mut packages = if context::is_current_caller_scope_active() {
        if hub.get_current_caller_uid() != caller_uid {
            return None;
        }
        vec![hub.get_current_caller_package()]
    } else {
        policy::get_packages_for_uid(caller_uid)
    };
    packages.sort();
    packages.dedup();
    if packages.len() != 1 {
        return None;
    }
    let config = SettingsHub::instance();
    packages.into_iter().next().filter(|package_name| {
        !package_name.is_empty()
            && !policy::is_system_writer_package(package_name)
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
        let package_uid =
            policy::get_fresh_uid_for_package_in_user(&identity.package_name, user_id);
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
        let package_uid = policy::get_uid_for_package_in_user(&package_name, user_id);
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
