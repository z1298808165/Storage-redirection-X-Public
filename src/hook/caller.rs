use super::context;
use super::runtime;
use super::stats::InterceptHub;
use crate::config::SettingsHub;
use crate::monitor::AuditTrail;
use crate::platform::paths;
use crate::redirect::policy;
use libc::{RTLD_LOCAL, RTLD_NOW, c_char, c_int, c_void, dlopen, dlsym, getpid, getuid, prctl};
use std::cell::Cell;
use std::ffi::CString;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicI64, AtomicU64, Ordering},
};

const ANDROID_APP_UID_START: i32 = 10000;
const FUSE_CALLER_MAX_AGE_MS: i64 = 1500;
/// 系统 uid FUSE 请求跨线程接力槽的取用窗口：读线程与处理线程分离时
/// （FuseDaemon 工作池模型），分发延迟可超过 100ms（2026-10-24 真机：shell
/// 的 mkdir 在繁忙窗口下槽过期导致归因落空）。系统 uid 请求本身低频，
/// 1s 窗口的串味风险（相邻两个不同系统进程的请求互相错配）远小于
/// 归因落空，取 1s。
const SYSTEM_FUSE_SLOT_MAX_AGE_MS: i64 = 1000;
/// 系统 uid 归因 miss 运维诊断的限频间隔。
const SYSTEM_MISS_LOG_INTERVAL_MS: i64 = 10_000;
const BINDER_SAVED_CALLER_MAX_AGE_MS: i64 = 1500;
const BINDER_SAVED_CALLER_HARD_LIMIT_MS: i64 = 30_000;
const CURRENT_CALLER_MAX_AGE_MS: i64 = 500;
const CALLER_LOG_SAMPLE_STEP: u64 = 1024;
static CALLER_FUSE_STALE_COUNT: AtomicU64 = AtomicU64::new(0);
static CALLER_SAVED_UID_STALE_COUNT: AtomicU64 = AtomicU64::new(0);
static CALLER_FALLBACK_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
static CALLER_UNRESOLVED_COUNT: AtomicU64 = AtomicU64::new(0);
static CALLER_SIGNAL_SKIP_COUNT: AtomicU64 = AtomicU64::new(0);
static CALLER_RECENT_REUSE_COUNT: AtomicU64 = AtomicU64::new(0);

thread_local! {
    // Binder 线程名终身不变，缓存避免重复 prctl 系统调用
    static BINDER_THREAD_CACHED: Cell<Option<bool>> = const { Cell::new(None) };
    // 写形态操作在调用 update_caller 前置位：系统 uid 接力槽只允许公共路径
    // 的写形态操作消费（产品边界：仅公共路径的共享/系统 uid FUSE 写需要
    // 源头发起应用识别）。MP 的高频读操作（stat/access，占 hook 流量 99%+）
    // 与私有路径写不取槽，否则并发读会在毫秒内把待归因的公共路径写请求
    // 的槽内容抢走（2026-10-24 真机：mkdir 槽内容被 stat 秒抢，监视记录
    // 全部 unknown）。标记在 update_caller 入口读取并复位，不跨调用泄漏。
    static SYSTEM_SLOT_TAKE_ALLOWED: Cell<bool> = const { Cell::new(false) };
}

fn is_likely_binder_thread_cached() -> bool {
    BINDER_THREAD_CACHED.with(|cell| {
        if let Some(cached) = cell.get() {
            return cached;
        }
        let result = is_likely_binder_thread();
        cell.set(Some(result));
        result
    })
}

// FUSE / Binder saved UID / Binder 线程三者皆无时调用方解析必定失败，可直接跳过
fn has_caller_signal(hub: &InterceptHub) -> bool {
    let fuse_uid = hub.get_fuse_caller_uid();
    if fuse_uid >= 0 {
        let fuse_age = context::get_fuse_caller_uid_age_ms();
        if (0..=FUSE_CALLER_MAX_AGE_MS).contains(&fuse_age) {
            return true;
        }
    }

    let saved_uid = context::get_binder_saved_caller_uid();
    if saved_uid >= ANDROID_APP_UID_START {
        let max_age = if context::is_binder_identity_cleared() {
            BINDER_SAVED_CALLER_HARD_LIMIT_MS
        } else {
            BINDER_SAVED_CALLER_MAX_AGE_MS
        };
        let saved_age = context::get_binder_saved_caller_uid_age_ms();
        if (0..=max_age).contains(&saved_age) {
            return true;
        }
    }

    is_likely_binder_thread_cached()
}

// 通过 Binder / FUSE / PID 多级回退解析当前线程调用方
pub fn update_caller_package_for_current_thread(hub: &InterceptHub) {
    // 读取并复位槽取用资格标记（由写形态 hook 经 update_caller_for_mutation
    // 置位）；复位放在入口，任何退出路径都不会向后续操作泄漏。
    let system_slot_take_allowed = SYSTEM_SLOT_TAKE_ALLOWED.with(|cell| cell.replace(false));
    // MediaProvider 的文件操作持续经过这里：借调用节流探测晚加载的
    // FUSE 引擎 libfuse.so 并补装 read 补丁（/dev/fuse 请求的真实读取方，晚于 hook
    // 安装）。仅媒体提供器进程探测，其它进程一次字符串比较即短路。
    if hub.with_package_name(policy::is_media_provider_package) {
        super::ops::read::ensure_fuse_engine_read_patch();
    }

    let previous_package = hub.get_current_caller_package();
    let previous_uid = hub.get_current_caller_uid();
    let previous_age_ms = context::get_current_caller_age_ms();
    let previous_from_external_signal = context::is_current_caller_from_external_signal();

    let can_reuse_recent_caller =
        hub.with_package_name(should_reuse_recent_current_caller_for_process);

    if !has_caller_signal(hub) {
        resolve_missing_caller_signal(
            hub,
            &previous_package,
            previous_uid,
            previous_age_ms,
            previous_from_external_signal,
            can_reuse_recent_caller,
            system_slot_take_allowed,
        );
        return;
    }

    let Some(resolved) = resolve_caller_uid_with_fuse_fallback(hub) else {
        return;
    };
    let CallerUidResolution {
        binder_uid,
        fuse_uid,
        fuse_age_ms,
        mut caller_uid,
        mut caller_source,
    } = resolved;

    let mut package_name = String::new();
    if caller_uid >= ANDROID_APP_UID_START {
        package_name = resolve_caller_package_by_uid(caller_uid, hub);
    }

    let mut caller_pid = -1;
    let saved_uid = context::get_binder_saved_caller_uid();
    let saved_uid_age_ms = context::get_binder_saved_caller_uid_age_ms();
    let identity_cleared = context::is_binder_identity_cleared();
    if package_name.is_empty() {
        caller_pid = resolve_caller_pid_by_binder();
        if caller_pid > 0 {
            package_name = resolve_caller_package_by_pid(caller_pid);
            if !package_name.is_empty() {
                caller_source = "binder_pid";
            }
        }
        // clearCallingIdentity 前保存的调用方 UID 作为末级回退
        // 处于 clear/restore 区间时直接使用，否则走时间窗口兜底
        if package_name.is_empty() && saved_uid >= ANDROID_APP_UID_START {
            let saved_uid_valid = if identity_cleared {
                (0..=BINDER_SAVED_CALLER_HARD_LIMIT_MS).contains(&saved_uid_age_ms)
            } else {
                (0..=BINDER_SAVED_CALLER_MAX_AGE_MS).contains(&saved_uid_age_ms)
            };
            if saved_uid_valid {
                caller_uid = saved_uid;
                let saved_package = context::get_binder_saved_caller_package();
                if is_package_uid_match(&saved_package, saved_uid, hub) {
                    package_name = saved_package;
                    caller_source = "saved_pkg";
                }
                if package_name.is_empty() {
                    package_name = resolve_caller_package_by_uid(saved_uid, hub);
                }
                if !package_name.is_empty() && caller_source != "saved_pkg" {
                    caller_source = "saved_uid";
                }
            } else {
                context::clear_binder_saved_caller_uid();
                context::set_binder_identity_cleared(false);
                if saved_uid_age_ms > BINDER_SAVED_CALLER_MAX_AGE_MS {
                    let count = CALLER_SAVED_UID_STALE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                    if should_log_sample(count) {
                        log::debug!(
                            "caller skip stale saved_uid uid={} age_ms={} cleared={} n={}",
                            saved_uid,
                            saved_uid_age_ms,
                            identity_cleared,
                            count
                        );
                    }
                }
            }
        }
        if package_name.is_empty() && caller_uid < ANDROID_APP_UID_START {
            caller_uid = -1;
        }
    }

    if !package_name.is_empty() {
        context::set_current_caller_from_external_signal(&package_name, caller_uid);
        AuditTrail::instance().update_caller_package(&package_name);
        if caller_source != "binder_uid" {
            let count = CALLER_FALLBACK_HIT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if should_log_sample(count) {
                log::debug!(
                    "caller fallback src={} pkg={} uid={} binder_uid={} fuse_uid={} fuse_age_ms={} cleared={} n={}",
                    caller_source,
                    package_name,
                    caller_uid,
                    binder_uid,
                    fuse_uid,
                    fuse_age_ms,
                    identity_cleared,
                    count
                );
            }
        }
        return;
    }

    let count = CALLER_UNRESOLVED_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if can_reuse_recent_caller
        && try_reuse_recent_current_caller(
            hub,
            &previous_package,
            previous_uid,
            previous_age_ms,
            "unresolved",
        )
    {
        return;
    }
    if should_preserve_unresolved_external_uid(&hub.get_package_name(), caller_uid) {
        context::set_current_caller_from_external_signal("", caller_uid);
        if should_log_sample(count) {
            log::warn!(
                "caller unresolved uid preserved proc={} uid={} binder_uid={} binder_pid={} saved_uid={} saved_age_ms={} fuse_uid={} fuse_age_ms={} n={}",
                hub.get_package_name(),
                caller_uid,
                binder_uid,
                caller_pid,
                saved_uid,
                saved_uid_age_ms,
                fuse_uid,
                fuse_age_ms,
                count
            );
        }
        return;
    }
    if should_log_sample(count) {
        log::warn!(
            "caller unresolved proc={} binder_uid={} binder_pid={} saved_uid={} saved_age_ms={} fuse_uid={} fuse_age_ms={} n={}",
            hub.get_package_name(),
            binder_uid,
            caller_pid,
            saved_uid,
            saved_uid_age_ms,
            fuse_uid,
            fuse_age_ms,
            count
        );
    }
    hub.clear_current_caller();
}

/// caller UID 解析的中间产物：后续诊断日志要原样输出各来源的 UID 与年龄。
struct CallerUidResolution {
    binder_uid: i32,
    fuse_uid: i32,
    fuse_age_ms: i64,
    caller_uid: i32,
    caller_source: &'static str,
}

/// 公共存储路径的廉价判定（hook 收到的原始显示路径形态）：/storage/emulated/
/// <user>/ 或其 /data/media 别名，且不在 Android/data、Android/obb 私有子树。
/// 与 monitor 侧完整归一化判定相比只做前缀与子树排除，供写形态 hook 在
/// update_caller 前做槽取用资格标记（产品边界：仅公共路径的共享/系统 uid
/// FUSE 写需要源头发起应用识别）。
fn is_public_display_path_for_slot(path: &str) -> bool {
    (paths::starts_with(path, "/storage/emulated/") || paths::starts_with(path, "/data/media/"))
        && !path.contains("/Android/data/")
        && !path.contains("/Android/obb/")
}

/// 写形态 hook（mkdir/mknod/rename/unlink/link/写意图 open/creat 等）的
/// 调用方解析入口：按目标路径是否公共存储标记系统 uid 接力槽的取用资格，
/// 再复用基础解析；退出时无条件复位标记（should_resolve_caller_context
/// 短路时也不向同线程的后续操作泄漏）。
///
/// # SAFETY (参数约定)
/// `pathname` 由 hook 调用方保证指向 NUL 结尾 C 字符串或为空（与
/// `prepare_relevant_path` 的读取约定一致）；空指针按无槽取用资格处理。
pub fn update_caller_for_mutation(hub: &InterceptHub, pathname: *const c_char) {
    // SAFETY: pathname 有效性见函数级 SAFETY 约定；c_str_to_string 内部
    // 处理空指针并按 NUL 结尾读取。
    let path_text = unsafe { super::util::c_str_to_string(pathname) };
    SYSTEM_SLOT_TAKE_ALLOWED.with(|cell| cell.set(is_public_display_path_for_slot(&path_text)));
    if runtime::should_resolve_caller_context(hub) {
        update_caller_package_for_current_thread(hub);
    }
    SYSTEM_SLOT_TAKE_ALLOWED.with(|cell| cell.set(false));
}

/// 无调用方信号路径：优先复用最近的代写外部信号或当前调用方；两者都不适用时
/// 采样记一条 skip 并清空当前调用方。本函数执行完毕后调用方必须直接返回。
/// 系统 uid FUSE 请求的身份解析：pid 表（root 扫描 /proc 的权威快照）优先，
/// /proc cmdline（包名形态 → native basename）兜底；解析出的身份必须能通过
/// uid 校验（表内 uid / uid 缓存 / /proc status 任一）才可信。
/// 返回 (包名, uid)。MP 域读 platform/shell 等进程的 /proc 常被 SELinux 拒绝，
/// 此时 pid 表是唯一可信来源。
fn resolve_system_fuse_identity(fuse_uid: i32, fuse_pid: i32) -> Option<(String, i32)> {
    if let Some((table_pkg, table_uid)) = lookup_system_pid_table(fuse_pid)
        && table_uid == fuse_uid
    {
        return Some((table_pkg, table_uid));
    }
    // 表未命中：区分「表里根本没有该 pid」（daemon 扫描空窗）与「uid 不匹配」
    //（身份错误），10 秒限频记录一次供运维判断竞态边界。
    let table_seen = lookup_system_pid_table(fuse_pid).is_some();
    let mut pkg = resolve_caller_package_by_pid(fuse_pid);
    if pkg.is_empty() {
        pkg = resolve_native_identity_by_pid(fuse_pid);
    }
    if pkg.is_empty() {
        static LAST_STAGE_LOG_MS: AtomicI64 = AtomicI64::new(-1);
        let now_ms = crate::platform::paths::monotonic_ms();
        let last_log_ms = LAST_STAGE_LOG_MS.load(Ordering::Relaxed);
        if last_log_ms < 0 || now_ms.saturating_sub(last_log_ms) >= SYSTEM_MISS_LOG_INTERVAL_MS {
            LAST_STAGE_LOG_MS.store(now_ms, Ordering::Relaxed);
            log::warn!(
                "fuse system resolve failed pid={} uid={} table_seen={} table_hint={:?}",
                fuse_pid,
                fuse_uid,
                table_seen,
                lookup_system_pid_table(fuse_pid)
            );
        }
        return None;
    }
    let pkg_uid = policy::get_uid_for_package(&pkg);
    let uid_matches = if pkg_uid >= 0 {
        pkg_uid == fuse_uid
    } else {
        read_uid_from_proc_status(fuse_pid) == fuse_uid
    };
    if uid_matches {
        return Some((pkg, fuse_uid));
    }
    static LAST_STAGE_LOG_MS: AtomicI64 = AtomicI64::new(-1);
    let now_ms = crate::platform::paths::monotonic_ms();
    let last_log_ms = LAST_STAGE_LOG_MS.load(Ordering::Relaxed);
    if last_log_ms < 0 || now_ms.saturating_sub(last_log_ms) >= SYSTEM_MISS_LOG_INTERVAL_MS {
        LAST_STAGE_LOG_MS.store(now_ms, Ordering::Relaxed);
        log::warn!(
            "fuse system resolve failed pid={} uid={} table_seen={} pkg={} pkg_uid={}",
            fuse_pid,
            fuse_uid,
            table_seen,
            pkg,
            pkg_uid
        );
    }
    None
}

fn resolve_missing_caller_signal(
    hub: &InterceptHub,
    previous_package: &str,
    previous_uid: i32,
    previous_age_ms: i64,
    previous_from_external_signal: bool,
    can_reuse_recent_caller: bool,
    system_slot_take_allowed: bool,
) {
    // 系统 uid FUSE 请求的跨线程接力：MediaProvider 读 /dev/fuse 的线程与处理
    // 请求的线程分离时，读线程捕获的请求头经全局槽传到这里（见 context.rs）。
    // 内核请求头的 pid 是强信号，优先于 recent 复用；cmdline 为包名形态且与
    // 请求 uid 匹配才采信，否则槽内容作废继续走既有回退。
    // 取槽资格由调用方（写形态 hook）标记：公共路径的写形态操作才允许消费，
    // 读操作与私有路径写直接跳过（见 SYSTEM_SLOT_TAKE_ALLOWED 注释）。
    if system_slot_take_allowed
        && let Some((request_uid, request_pid)) =
            context::take_recent_system_fuse_request(SYSTEM_FUSE_SLOT_MAX_AGE_MS)
        && request_pid > 0
    {
        // 跨线程接力的权威解析走 pid 表（root daemon 落盘的快照）：MP 域对
        // shell/platform 等进程的 /proc cmdline/status 直读常被 SELinux 拒绝，
        // 只靠 /proc 会让槽内容在跨线程场景全部作废（2026-10-24 真机：同线程
        // 路径归因成功日志在，跨线程的监视记录仍 unknown 的根因）。
        // resolve_system_fuse_identity 内部为「pid 表优先 + /proc 兜底 + uid
        // 校验」，是原 /proc 直读分支的严格超集，miss 时保留 10 秒限频诊断。
        if let Some((pkg, resolved_uid)) = resolve_system_fuse_identity(request_uid, request_pid) {
            context::set_current_caller_from_external_signal(&pkg, resolved_uid);
            AuditTrail::instance().update_caller_package(&pkg);
            let count = CALLER_FALLBACK_HIT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if should_log_sample(count) {
                log::debug!(
                    "caller fallback src=fuse_slot_system pkg={} uid={} pid={} n={}",
                    pkg,
                    resolved_uid,
                    request_pid,
                    count
                );
            }
            return;
        }
    }
    if try_reuse_recent_external_signal_for_system_writer(
        hub,
        previous_package,
        previous_uid,
        previous_age_ms,
        previous_from_external_signal,
        "signal_skip",
    ) {
        return;
    }
    if can_reuse_recent_caller
        && try_reuse_recent_current_caller(
            hub,
            previous_package,
            previous_uid,
            previous_age_ms,
            "signal_skip",
        )
    {
        return;
    }
    let count = CALLER_SIGNAL_SKIP_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if should_log_sample(count) {
        log::debug!(
            "caller signal skip proc={} n={}",
            hub.get_package_name(),
            count
        );
    }
    hub.clear_current_caller();
}

/// 解析调用方 UID：优先 Binder UID；拿不到时用 FUSE 附加的调用方 UID 兜底。
///
/// 共享 UID 场景（FUSE UID 与自身相同）按 fuse_pid 归因：此时外部信号与审计
/// 线索已在函数内更新，返回 None 让调用方直接返回。返回值同时携带各来源的
/// 原始 UID 与年龄，供后续「unresolved」诊断日志完整输出归因链。
fn resolve_caller_uid_with_fuse_fallback(hub: &InterceptHub) -> Option<CallerUidResolution> {
    let binder_uid = resolve_caller_uid_by_binder();
    let mut caller_uid = binder_uid;
    let mut caller_source = "binder_uid";
    let mut fuse_uid = -1;
    let mut fuse_age_ms = -1;

    if caller_uid < 0 {
        fuse_uid = hub.get_fuse_caller_uid();
        fuse_age_ms = context::get_fuse_caller_uid_age_ms();
        // SAFETY: getuid 只返回调用进程的真实 UID，无指针参数与副作用。
        let self_uid = unsafe { getuid() } as i32;
        if fuse_uid >= ANDROID_APP_UID_START && (0..=FUSE_CALLER_MAX_AGE_MS).contains(&fuse_age_ms)
        {
            if fuse_uid != self_uid {
                caller_uid = fuse_uid;
                caller_source = "fuse_uid";
            } else {
                // 共享 UID：UID 相同但 PID 不同说明来自同组的另一个进程
                let fuse_pid = context::get_fuse_caller_pid();
                // SAFETY: getpid 只返回调用进程的 PID，无指针参数与副作用。
                let self_pid = unsafe { getpid() } as i32;
                if fuse_pid > 0 && fuse_pid != self_pid {
                    let pkg = resolve_caller_package_by_pid(fuse_pid);
                    if !pkg.is_empty() {
                        context::set_current_caller_from_external_signal(&pkg, fuse_uid);
                        AuditTrail::instance().update_caller_package(&pkg);
                        let count = CALLER_FALLBACK_HIT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                        if should_log_sample(count) {
                            log::debug!(
                                "caller fallback src=fuse_pid pkg={} uid={} fuse_pid={} n={}",
                                pkg,
                                fuse_uid,
                                fuse_pid,
                                count
                            );
                        }
                        context::clear_fuse_caller_uid();
                        return None;
                    }
                }
            }
        } else if fuse_uid >= 0 && (0..=FUSE_CALLER_MAX_AGE_MS).contains(&fuse_age_ms) {
            // 系统 uid（如 system 1000）的 FUSE 请求：uid 区间解析不出包名，
            // 但内核请求头的 pid 就是发起进程。cmdline 为包名形态且与请求
            // uid 匹配时按发起进程归因（2026-10-24 真机：开机创建
            // MIUI/Mirror/MainCache 的系统服务此前只能记录 unknown）。
            let fuse_pid = context::get_fuse_caller_pid();
            // SAFETY: getpid 只返回调用进程的 PID，无指针参数与副作用。
            let self_pid = unsafe { getpid() } as i32;
            if fuse_pid > 0 && fuse_pid != self_pid {
                if let Some((pkg, resolved_uid)) = resolve_system_fuse_identity(fuse_uid, fuse_pid)
                {
                    context::set_current_caller_from_external_signal(&pkg, resolved_uid);
                    AuditTrail::instance().update_caller_package(&pkg);
                    let count = CALLER_FALLBACK_HIT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                    if should_log_sample(count) {
                        log::debug!(
                            "caller fallback src=fuse_pid_system pkg={} uid={} fuse_pid={} n={}",
                            pkg,
                            resolved_uid,
                            fuse_pid,
                            count
                        );
                    }
                    context::clear_fuse_caller_uid();
                    return None;
                }
                // 运维诊断：系统 uid 请求未归因时按 10 秒限频记录发起任务 id 与
                // 解析现场。短命进程/线程在 pid 表快照间隙消亡属已知边界；持久
                // 服务若持续命中此日志则需要排查 daemon pid 表链路。
                static LAST_MISS_LOG_MS: AtomicI64 = AtomicI64::new(-1);
                let now_ms = crate::platform::paths::monotonic_ms();
                let last_log_ms = LAST_MISS_LOG_MS.load(Ordering::Relaxed);
                if last_log_ms < 0
                    || now_ms.saturating_sub(last_log_ms) >= SYSTEM_MISS_LOG_INTERVAL_MS
                {
                    LAST_MISS_LOG_MS.store(now_ms, Ordering::Relaxed);
                    log::warn!(
                        "fuse system pid miss pid={} uid={} proc_pkg={} proc_uid={} table_hint={:?} fuse_age={}",
                        fuse_pid,
                        fuse_uid,
                        resolve_caller_package_by_pid(fuse_pid),
                        read_uid_from_proc_status(fuse_pid),
                        lookup_system_pid_table(fuse_pid),
                        fuse_age_ms
                    );
                }
            }
        } else if fuse_uid >= ANDROID_APP_UID_START && fuse_age_ms > FUSE_CALLER_MAX_AGE_MS {
            let count = CALLER_FUSE_STALE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if should_log_sample(count) {
                log::debug!(
                    "caller skip stale fuse uid={} age_ms={} n={}",
                    fuse_uid,
                    fuse_age_ms,
                    count
                );
            }
        }
        context::clear_fuse_caller_uid();
    }

    Some(CallerUidResolution {
        binder_uid,
        fuse_uid,
        fuse_age_ms,
        caller_uid,
        caller_source,
    })
}

fn should_preserve_unresolved_external_uid(package_name: &str, caller_uid: i32) -> bool {
    caller_uid >= ANDROID_APP_UID_START
        && (policy::is_system_writer_package(package_name)
            || policy::is_saf_native_monitor_bridge_package(package_name))
}

fn try_reuse_recent_external_signal_for_system_writer(
    hub: &InterceptHub,
    package_name: &str,
    caller_uid: i32,
    age_ms: i64,
    from_external_signal: bool,
    reason: &str,
) -> bool {
    if !should_reuse_recent_external_signal_for_system_writer(
        &hub.get_package_name(),
        caller_uid,
        age_ms,
        from_external_signal,
    ) {
        return false;
    }

    let caller_package = if !package_name.is_empty()
        && package_name != hub.get_package_name()
        && !policy::is_system_writer_package(package_name)
    {
        package_name
    } else {
        ""
    };
    context::set_current_caller_from_external_signal(caller_package, caller_uid);
    if !caller_package.is_empty() {
        AuditTrail::instance().update_caller_package(caller_package);
    }

    let count = CALLER_RECENT_REUSE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if should_log_sample(count) {
        log::debug!(
            "caller reuse recent external signal reason={} pkg={} uid={} age_ms={} n={}",
            reason,
            caller_package,
            caller_uid,
            age_ms,
            count
        );
    }
    true
}

fn should_reuse_recent_external_signal_for_system_writer(
    process_package: &str,
    caller_uid: i32,
    age_ms: i64,
    from_external_signal: bool,
) -> bool {
    from_external_signal
        && (policy::is_system_writer_package(process_package)
            || policy::is_saf_native_monitor_bridge_package(process_package))
        && caller_uid >= ANDROID_APP_UID_START
        && (0..=CURRENT_CALLER_MAX_AGE_MS).contains(&age_ms)
}

fn try_reuse_recent_current_caller(
    hub: &InterceptHub,
    package_name: &str,
    caller_uid: i32,
    age_ms: i64,
    reason: &str,
) -> bool {
    if package_name.is_empty()
        || caller_uid < ANDROID_APP_UID_START
        || !(0..=CURRENT_CALLER_MAX_AGE_MS).contains(&age_ms)
        || package_name == hub.get_package_name()
        || policy::is_system_writer_package(package_name)
    {
        return false;
    }

    let count = CALLER_RECENT_REUSE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if should_log_sample(count) {
        log::debug!(
            "caller reuse recent reason={} pkg={} uid={} age_ms={} n={}",
            reason,
            package_name,
            caller_uid,
            age_ms,
            count
        );
    }
    true
}

fn should_reuse_recent_current_caller_for_process(package_name: &str) -> bool {
    !policy::is_system_writer_package(package_name)
        && (!policy::is_file_monitor_bridge_package(package_name)
            || policy::is_saf_native_monitor_bridge_package(package_name))
}

#[inline]
fn should_log_sample(count: u64) -> bool {
    count == 1 || count.is_multiple_of(CALLER_LOG_SAMPLE_STEP)
}

fn resolve_caller_uid_by_binder() -> i32 {
    if !is_likely_binder_thread_cached() {
        return -1;
    }

    let symbols = get_caller_binder_symbols();
    let mut caller_uid = -1;
    if let Some(func) = symbols.ndk_get_calling_uid {
        caller_uid = unsafe { func() } as i32;
    }

    if caller_uid < 0
        && let (Some(self_fn), Some(get_uid_fn)) =
            (symbols.binder_self, symbols.binder_get_calling_uid)
    {
        let ipc_state = unsafe { self_fn() };
        if !ipc_state.is_null() {
            caller_uid = unsafe { get_uid_fn(ipc_state) } as i32;
        }
    }

    if caller_uid < ANDROID_APP_UID_START {
        return -1;
    }

    let self_uid = unsafe { getuid() } as i32;
    if caller_uid == self_uid {
        return -1;
    }

    caller_uid
}

fn resolve_caller_pid_by_binder() -> i32 {
    if !is_likely_binder_thread_cached() {
        return -1;
    }

    let symbols = get_caller_binder_symbols();
    let mut caller_pid = -1;
    if let Some(func) = symbols.ndk_get_calling_pid {
        caller_pid = unsafe { func() };
    }

    if caller_pid <= 0
        && let (Some(self_fn), Some(get_pid_fn)) =
            (symbols.binder_self, symbols.binder_get_calling_pid)
    {
        let ipc_state = unsafe { self_fn() };
        if !ipc_state.is_null() {
            caller_pid = unsafe { get_pid_fn(ipc_state) };
        }
    }

    if caller_pid <= 0 || caller_pid == unsafe { getpid() } as i32 {
        return -1;
    }

    caller_pid
}

// 通过 /proc/PID/cmdline 取包名
fn resolve_caller_package_by_pid(pid: i32) -> String {
    if pid <= 0 {
        return String::new();
    }

    let cmdline_path = format!("/proc/{}/cmdline", pid);
    let mut cmdline = read_single_line_by_syscall(&cmdline_path);
    if cmdline.is_empty() {
        return String::new();
    }

    if let Some(pos) = cmdline.find('\0') {
        cmdline.truncate(pos);
    }

    if let Some(pos) = cmdline.find(':') {
        cmdline.truncate(pos);
    }

    normalize_caller_package_text(&cmdline)
}

/// native 二进制进程的身份提取：argv0 带路径时取 basename（如 adb shell 派生
/// 的 /system/bin/sh → "sh"）。仅供系统 uid 分支使用——共享的
/// resolve_caller_package_by_pid 保持包名语义不变，native 二进制没有包名，
/// basename 是 pid 归因能给出的最诚实身份。
fn resolve_native_identity_by_pid(pid: i32) -> String {
    if pid <= 0 {
        return String::new();
    }

    let cmdline_path = format!("/proc/{}/cmdline", pid);
    let mut cmdline = read_single_line_by_syscall(&cmdline_path);
    if cmdline.is_empty() {
        return String::new();
    }

    if let Some(pos) = cmdline.find('\0') {
        cmdline.truncate(pos);
    }

    if let Some(pos) = cmdline.rfind('/') {
        cmdline.drain(..=pos);
    }

    if let Some(pos) = cmdline.find(':') {
        cmdline.truncate(pos);
    }

    normalize_caller_package_text(&cmdline)
}

fn resolve_caller_package_by_uid(caller_uid: i32, hub: &InterceptHub) -> String {
    if caller_uid < ANDROID_APP_UID_START {
        return String::new();
    }

    let mut packages = policy::get_packages_for_uid(caller_uid);
    if packages.is_empty() && should_refresh_uid_cache_for_process(hub) {
        policy::refresh_shared_uid_cache();
        packages = policy::get_packages_for_uid(caller_uid);
    }
    if packages.is_empty() {
        return String::new();
    }

    packages.sort();
    packages.dedup();

    let mut candidates = Vec::new();
    let self_package = hub.get_package_name();
    for package in packages {
        if package.is_empty() || package == self_package {
            continue;
        }
        if policy::is_system_writer_package(&package) {
            continue;
        }
        candidates.push(package);
    }

    if candidates.is_empty() {
        return String::new();
    }
    if candidates.len() == 1 {
        return candidates.remove(0);
    }

    let config = SettingsHub::instance();
    let mut mapping_candidates = Vec::new();
    let mut fallback_candidates = Vec::new();

    for candidate in candidates {
        let Some(profile) = config.get_resolved_user_profile_snapshot(&candidate, caller_uid)
        else {
            continue;
        };
        if !profile.path_mappings.is_empty() {
            mapping_candidates.push(candidate);
        } else {
            fallback_candidates.push(candidate);
        }
    }

    if mapping_candidates.len() == 1 {
        return mapping_candidates.remove(0);
    }
    if mapping_candidates.is_empty() && fallback_candidates.len() == 1 {
        return fallback_candidates.remove(0);
    }

    String::new()
}

fn is_package_uid_match(package_name: &str, uid: i32, hub: &InterceptHub) -> bool {
    if package_name.is_empty() || uid < ANDROID_APP_UID_START {
        return false;
    }
    if package_name == hub.get_package_name() || policy::is_system_writer_package(package_name) {
        return false;
    }

    let package_uid = if should_refresh_uid_cache_for_process(hub) {
        policy::get_fresh_uid_for_package(package_name)
    } else {
        policy::get_uid_for_package(package_name)
    };
    package_uid == uid
}

fn should_refresh_uid_cache_for_process(hub: &InterceptHub) -> bool {
    let package_name = hub.get_package_name();
    !policy::is_system_writer_package(&package_name)
}

fn is_pid_uid_match(pid: i32, uid: i32) -> bool {
    if pid <= 0 || uid < ANDROID_APP_UID_START {
        return false;
    }
    read_uid_from_proc_status(pid) == uid
}

fn read_uid_from_proc_status(pid: i32) -> i32 {
    let status_path = format!("/proc/{}/status", pid);
    let content = read_file_by_syscall(&status_path, 4096);
    if content.is_empty() {
        return -1;
    }
    for line in content.lines() {
        let Some(rest) = line.strip_prefix("Uid:") else {
            continue;
        };
        return rest
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(-1);
    }
    -1
}

fn is_likely_binder_thread() -> bool {
    let mut name = [0u8; 17];
    let ret = unsafe { prctl(libc::PR_GET_NAME, name.as_mut_ptr() as *mut c_void, 0, 0, 0) };
    if ret != 0 {
        return false;
    }

    let raw = String::from_utf8_lossy(&name);
    raw.starts_with("Binder:")
        || raw.starts_with("binder:")
        || raw.starts_with("HwBinder:")
        || raw.starts_with("hwbinder:")
}

// 直接走 syscall，避免触发自身 Hook
fn read_single_line_by_syscall(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let Ok(c_path) = CString::new(path) else {
        return String::new();
    };

    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
            0,
        ) as c_int
    };
    if fd < 0 {
        return String::new();
    }

    let mut buffer = [0u8; 256];
    let mut total = 0usize;
    while total < buffer.len() - 1 {
        let n = unsafe {
            libc::read(
                fd,
                buffer[total..].as_mut_ptr() as *mut c_void,
                buffer.len() - 1 - total,
            )
        };
        if n <= 0 {
            break;
        }

        total += n as usize;
        if buffer[..total]
            .iter()
            .any(|b| *b == 0 || *b == b'\n' || *b == b'\r')
        {
            break;
        }
    }

    unsafe { libc::close(fd) };
    if total == 0 {
        return String::new();
    }

    let mut end = 0usize;
    while end < total {
        let ch = buffer[end];
        if ch == 0 || ch == b'\n' || ch == b'\r' {
            break;
        }
        end += 1;
    }

    if end == 0 {
        return String::new();
    }

    String::from_utf8_lossy(&buffer[..end]).to_string()
}

fn read_file_by_syscall(path: &str, max_len: usize) -> String {
    if path.is_empty() || max_len == 0 {
        return String::new();
    }

    let Ok(c_path) = CString::new(path) else {
        return String::new();
    };

    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
            0,
        ) as c_int
    };
    if fd < 0 {
        return String::new();
    }

    let mut buffer = vec![0u8; max_len.min(512 * 1024)];
    let mut total = 0usize;
    while total < buffer.len() {
        let n = unsafe {
            libc::read(
                fd,
                buffer[total..].as_mut_ptr() as *mut c_void,
                buffer.len() - total,
            )
        };
        if n <= 0 {
            break;
        }
        total += n as usize;
    }

    unsafe { libc::close(fd) };
    if total == 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buffer[..total]).to_string()
}

fn normalize_caller_package_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let mut value = text.to_string();
    if let Some(pos) = value.find(['\r', '\n', '\t', ' ']) {
        value.truncate(pos);
    }
    if value.is_empty() {
        return String::new();
    }

    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
    {
        return String::new();
    }
    value
}

#[derive(Clone, Copy)]
struct CallerBinderSymbols {
    ndk_get_calling_uid: Option<unsafe extern "C" fn() -> u32>,
    ndk_get_calling_pid: Option<unsafe extern "C" fn() -> i32>,
    binder_self: Option<unsafe extern "C" fn() -> *mut c_void>,
    binder_get_calling_uid: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    binder_get_calling_pid: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
}

fn get_caller_binder_symbols() -> &'static CallerBinderSymbols {
    static SYMBOLS: OnceLock<CallerBinderSymbols> = OnceLock::new();
    SYMBOLS.get_or_init(|| {
        let mut symbols = CallerBinderSymbols {
            ndk_get_calling_uid: None,
            ndk_get_calling_pid: None,
            binder_self: None,
            binder_get_calling_uid: None,
            binder_get_calling_pid: None,
        };

        unsafe {
            if let Some(libbinder_ndk_name) = c_str("libbinder_ndk.so") {
                let libbinder_ndk = dlopen(libbinder_ndk_name.as_ptr(), RTLD_NOW | RTLD_LOCAL);
                if !libbinder_ndk.is_null() {
                    symbols.ndk_get_calling_uid =
                        load_symbol(libbinder_ndk, "AIBinder_getCallingUid");
                    symbols.ndk_get_calling_pid =
                        load_symbol(libbinder_ndk, "AIBinder_getCallingPid");
                }
            }

            if let Some(libbinder_name) = c_str("libbinder.so") {
                let libbinder = dlopen(libbinder_name.as_ptr(), RTLD_NOW | RTLD_LOCAL);
                if !libbinder.is_null() {
                    symbols.binder_self =
                        load_symbol(libbinder, "_ZN7android14IPCThreadState4selfEv");
                    symbols.binder_get_calling_uid =
                        load_symbol(libbinder, "_ZNK7android14IPCThreadState13getCallingUidEv");
                    if symbols.binder_get_calling_uid.is_none() {
                        symbols.binder_get_calling_uid =
                            load_symbol(libbinder, "_ZN7android14IPCThreadState13getCallingUidEv");
                    }
                    symbols.binder_get_calling_pid =
                        load_symbol(libbinder, "_ZNK7android14IPCThreadState13getCallingPidEv");
                    if symbols.binder_get_calling_pid.is_none() {
                        symbols.binder_get_calling_pid =
                            load_symbol(libbinder, "_ZN7android14IPCThreadState13getCallingPidEv");
                    }
                }
            }
        }

        symbols
    })
}

unsafe fn load_symbol<T>(handle: *mut c_void, name: &str) -> Option<T> {
    let c_name = c_str(name)?;
    let symbol = dlsym(handle, c_name.as_ptr());
    if symbol.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&symbol))
    }
}

fn c_str(value: &str) -> Option<CString> {
    CString::new(value).ok()
}

/// daemon 落盘的系统进程 pid 表（tmp/system_pids，"pid uid 包名" 行）读取缓存。
///
/// 表由 root daemon 事件化重建（进程/线程集合变化即刷）；本缓存在 TTL 内复用，
/// 避免高频归因反复读文件。新进程/新线程的首次存储操作可能撞上缓存窗口，
/// 因此 miss 且缓存已具备重读条件时强制重读一次。
struct SystemPidTable {
    loaded_at_ms: i64,
    entries: std::collections::HashMap<i32, (String, i32)>,
}

static SYSTEM_PID_TABLE: OnceLock<Mutex<SystemPidTable>> = OnceLock::new();
const SYSTEM_PID_TABLE_TTL_MS: i64 = 2_000;
/// miss 强制重读的最小缓存年龄：低于它时新表大概率也尚未落盘，直接放弃；
/// 高于它时表文件很可能已被 daemon 刷新，重读一次能接住刚启动进程的首次写入。
const SYSTEM_PID_TABLE_RECHECK_MIN_AGE_MS: i64 = 300;

/// 按任务 id 反查 (包名, uid)：先查缓存，过期后从模块目录经 Zygisk 保留 FD
/// 读取 daemon 写入的表并重建缓存；未命中且缓存已老于重读门槛时再强制刷新
/// 一次。表缺失或不可读返回 None。
fn lookup_system_pid_table(pid: i32) -> Option<(String, i32)> {
    let table_mutex = SYSTEM_PID_TABLE.get_or_init(|| {
        Mutex::new(SystemPidTable {
            loaded_at_ms: -1,
            entries: std::collections::HashMap::new(),
        })
    });
    let mut table = table_mutex.lock().unwrap_or_else(|err| err.into_inner());
    let now_ms = get_monotonic_ms();
    let mut force_reload = table.loaded_at_ms < 0
        || now_ms.saturating_sub(table.loaded_at_ms) >= SYSTEM_PID_TABLE_TTL_MS;
    if let Some(entry) = table.entries.get(&pid) {
        return Some(entry.clone());
    }
    // miss：缓存窗口可能盖住了 daemon 刚刷新的新表，年龄达标就强制重读一次。
    if !force_reload
        && now_ms.saturating_sub(table.loaded_at_ms) >= SYSTEM_PID_TABLE_RECHECK_MIN_AGE_MS
    {
        force_reload = true;
    }
    if force_reload {
        table.entries.clear();
        table.loaded_at_ms = now_ms;
        let content = read_system_pid_table_file();
        for line in content.lines() {
            let mut fields = line.split_whitespace();
            let Some(entry_pid) = fields.next().and_then(|v| v.parse::<i32>().ok()) else {
                continue;
            };
            let Some(uid) = fields.next().and_then(|v| v.parse::<i32>().ok()) else {
                continue;
            };
            let Some(package) = fields.next() else {
                continue;
            };
            table.entries.insert(entry_pid, (package.to_string(), uid));
        }
        if let Some(entry) = table.entries.get(&pid) {
            return Some(entry.clone());
        }
    }
    None
}

fn get_monotonic_ms() -> i64 {
    crate::platform::paths::monotonic_ms()
}

/// 读系统进程 pid 表：hook 进程内经 Zygisk 保留 FD（/proc/self/fd/<fd>/tmp/...）
/// 读取；直接模块路径仅 root 可见，普通 hook 进程不可达时返回空。
fn read_system_pid_table_file() -> String {
    let config_dir = SettingsHub::instance().config_dir();
    let module_dir = config_dir.strip_suffix("/config").unwrap_or(&config_dir);
    let table_path = format!("{module_dir}/tmp/system_pids");
    read_file_by_syscall(&table_path, 512 * 1024)
}

// clearCallingIdentity 之前提前快照 UID，供后续 restore 区间回查
pub fn capture_binder_caller_uid_if_available() {
    let uid = resolve_caller_uid_by_binder();
    if uid >= ANDROID_APP_UID_START {
        context::set_binder_saved_caller_uid(uid);
        let hub = InterceptHub::instance();
        let mut package_name = resolve_caller_package_by_uid(uid, hub);
        if package_name.is_empty() {
            let pid = resolve_caller_pid_by_binder();
            package_name = resolve_caller_package_by_pid(pid);
            if !is_package_uid_match(&package_name, uid, hub) && !is_pid_uid_match(pid, uid) {
                package_name.clear();
            }
        }
        if !package_name.is_empty() {
            context::set_binder_saved_caller_package(&package_name);
        }
    } else {
        context::clear_binder_saved_caller_uid();
    }
}

pub fn capture_java_binder_caller(caller_uid: i32, caller_pid: i32) {
    if caller_uid < ANDROID_APP_UID_START {
        return;
    }

    context::set_binder_saved_caller_uid(caller_uid);
    if let Some(package_name) = resolve_java_caller_package(caller_uid, caller_pid) {
        context::set_binder_saved_caller_package(&package_name);
    }
}

pub fn enter_java_caller_scope(caller_uid: i32, caller_pid: i32) {
    if caller_uid < ANDROID_APP_UID_START {
        context::push_current_caller_scope("", -1);
        return;
    }
    if let Some(package_name) = resolve_java_caller_package(caller_uid, caller_pid) {
        context::push_current_caller_scope(&package_name, caller_uid);
        context::set_binder_saved_caller_uid(caller_uid);
        context::set_binder_saved_caller_package(&package_name);
    } else {
        context::push_current_caller_scope("", -1);
    }
}

pub fn exit_java_caller_scope() {
    context::pop_current_caller_scope();
    if !context::is_current_caller_scope_active() {
        context::clear_binder_saved_caller_uid();
        context::set_binder_identity_cleared(false);
    }
}

fn resolve_java_caller_package(caller_uid: i32, caller_pid: i32) -> Option<String> {
    let hub = InterceptHub::instance();
    if caller_pid > 0 {
        let package_name = resolve_caller_package_by_pid(caller_pid);
        if !package_name.is_empty()
            && (is_package_uid_match(&package_name, caller_uid, hub)
                || is_pid_uid_match(caller_pid, caller_uid))
        {
            return Some(package_name);
        }
    }

    let package_name = resolve_caller_package_by_uid(caller_uid, hub);
    if package_name.is_empty() {
        None
    } else {
        Some(package_name)
    }
}
