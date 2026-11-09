use super::context;
use super::runtime;
use super::stats::InterceptHub;
use crate::config::SettingsHub;
use crate::monitor::AuditTrail;
use crate::platform::paths;
use crate::redirect::policy;
use libc::{RTLD_LOCAL, RTLD_NOW, c_char, c_void, dlopen, dlsym, getpid, getuid, prctl};
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
    // 同理复位「监视记录用请求方 uid/pid」：本次请求若拿不到 FUSE 头 pid，记录里就
    // 不该出现上一次请求的 uid/pid（daemon 会据此反查出错误源头）。
    //
    // 但**处在 Java 调用方作用域内时不能复位**：MediaProvider 的 provider 入口会把
    // binder 给出的调用方 uid/pid 先落进这两个槽（含系统 uid），随后该调用内部的
    // 文件操作还会走一遍 FUSE 侧入口；若无条件复位，Java 侧刚捕获的证据立刻被抹掉，
    // 记录退回 MediaProvider 自己的 uid（真机实测：shell 的 MediaStore insert 因此
    // 记成 caller_uid=10217、identity 未知）。
    if !context::is_current_caller_scope_active() {
        context::set_monitor_caller_pid(-1);
        context::set_monitor_caller_uid(-1);
    }
    // MediaProvider 的文件操作持续经过这里：借调用节流探测晚加载的
    // FUSE 引擎 libfuse.so 并补装 read 补丁（/dev/fuse 请求的真实读取方，晚于 hook
    // 安装）。仅媒体提供器进程探测，其它进程一次字符串比较即短路。
    if hub.with_package_name(policy::is_media_provider_package) {
        super::ops::read::ensure_fuse_engine_read_patch();
    }

    // 这几个「上一次的调用方」快照只在**解析失败**的回退路径上才用得到，而它们里的
    // 包名是一次 `String` 分配。这条函数每被拦截一次文件操作就调用一次（媒体扫描时
    // 每秒数千次），把分配推迟到真正需要它的分支即可，语义完全不变。
    let previous_uid = hub.get_current_caller_uid();
    let previous_age_ms = context::get_current_caller_age_ms();
    let previous_from_external_signal = context::is_current_caller_from_external_signal();

    let can_reuse_recent_caller =
        hub.with_package_name(should_reuse_recent_current_caller_for_process);

    if !has_caller_signal(hub) {
        // 只有走到这里才需要上一次的包名。
        let previous_package = hub.get_current_caller_package();
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
            } else if let Some((resolved_pkg, _)) =
                resolve_system_fuse_identity(caller_uid, caller_pid)
            {
                // 命令行不是包名形态：这一 pid 的进程身份只能由 pid → /proc 反查给出
                // （native 名 + uid 唯一归属时换算成包名）。与 FUSE 请求头那条路径共用
                // 同一实现，避免两处口径漂移。
                package_name = resolved_pkg;
                caller_source = "pid_proc";
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
            // 系统 uid 请求（root / system 二进制）解析不出包名时，内核 FUSE 头的
            // uid 仍是权威请求方；清成 -1 会让记录侧回退 fsuid，把「mediaprovider
            // 自己」写成请求方（实测 caller_uid=10217）。头部有 uid 就保留它，
            // 配合 caller_pid 一起落进记录，由 daemon 侧继续反查。
            if caller_source != "fuse_uid_system_fallback" {
                caller_uid = -1;
            }
        }
    }

    // 系统 uid 且只剩头部 uid：不回退到 fsuid，直接把「权威 uid + pid」挂到
    // 当前调用方上下文，让本条记录写出 caller_uid/caller_pid 而不是 MP 自身。
    if package_name.is_empty() && caller_source == "fuse_uid_system_fallback" && caller_uid >= 0 {
        context::set_current_caller_from_external_signal("", caller_uid);
        let count = CALLER_UNRESOLVED_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if should_log_sample(count) {
            log::debug!(
                "caller unresolved uid kept proc={} uid={} fuse_pid={} fuse_age_ms={} n={}",
                hub.get_package_name(),
                caller_uid,
                caller_pid,
                fuse_age_ms,
                count
            );
        }
        return;
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
    if can_reuse_recent_caller {
        let previous_package = hub.get_current_caller_package();
        if try_reuse_recent_current_caller(
            hub,
            &previous_package,
            previous_uid,
            previous_age_ms,
            "unresolved",
        ) {
            return;
        }
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
    // 共享 uid 下 pid 复用仍可能保持 uid 不变，因此周期快照不能证明当前任务身份。
    // 实时 root 查询是权威证据；表仅用于诊断快照是否曾见过该 pid。
    let table_seen = lookup_system_pid_table(fuse_pid).is_some();
    if let Some((remote_pkg, remote_uid)) =
        crate::daemon_identity_query::query_process_identity(fuse_pid, fuse_uid)
        && remote_uid == fuse_uid
    {
        return Some((remote_pkg, remote_uid));
    }
    let mut pkg = resolve_caller_package_by_pid(fuse_pid);
    if pkg.is_empty() {
        pkg = resolve_native_identity_by_pid(fuse_pid);
    }
    // 本地 /proc 读不到（MP 域读 platform 应用的 cmdline 被 SELinux 拒绝）时同步问
    // daemon：它是 root，读得到；而且这条证据与记录侧用的是同一份内核请求头 pid，
    // 因此决策与事后取证不会各说一套。**这不是按规则反推源头**——答案来自 /proc。
    let needs_remote_lookup = pkg.is_empty() || !pkg.contains('.');
    if needs_remote_lookup
        && let Some((remote_pkg, _)) =
            crate::daemon_identity_query::query_process_identity(fuse_pid, fuse_uid)
    {
        // daemon 侧还会按 packages.list 把「native 名 + 唯一 uid」收敛成包名；多包 uid
        // 不猜，仍回 native 名。包名形态才覆盖本地结果，否则保留本地证据。
        if pkg.is_empty() || remote_pkg.contains('.') {
            pkg = remote_pkg;
        }
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
        // 即使随后解析失败，pid 本身仍是这次请求的权威证据：写进记录后由
        // daemon（root）反查，MP 域读不到的 /proc 才不会白丢。
        context::set_monitor_caller_pid(request_pid);
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
        // FUSE 头的 tid 是「哪个进程发起了这次请求」的最精确证据，比 uid 反查
        // 可靠得多（同一 uid 可能对应多个包，uid 段还可能被 RRO 资源包占位）。
        // 解析结束时 fuse 槽会被清空，所以另存一份给监视记录用；具体反查交给
        // daemon（root）：MP 域读不到 root / 特权 helper 的 /proc cmdline。
        if fuse_uid >= 0 && (0..=FUSE_CALLER_MAX_AGE_MS).contains(&fuse_age_ms) {
            let header_pid = context::get_fuse_caller_pid();
            if header_pid > 0 {
                context::set_monitor_caller_pid(header_pid);
            }
        }
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
                // pid 表未命中时回退到裸 uid：内核 FUSE 头的 uid 由内核按请求方
                // fsuid 填写，仍是权威请求方（2026-10-30 A17 实测：shell 写入此前
                // 被归因为 MediaProvider 自身 10218）；优于 resolve 调用方随后的
                // fsuid 兜底。
                caller_uid = fuse_uid;
                caller_source = "fuse_uid_system_fallback";
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
//
// 先按原有语义取 argv[0]（包名形态时直接返回）；argv[0] 不是包名时（root helper
// 常见形态 `mtio daemon @bin.mt.plus-2967573 0`），再扫描完整 argv 里的包名形态
// token。真实发起者由内核 FUSE 头的 pid 指定，pid 对应的命令行是比 uid 更精确的
// 证据：同一个 root helper 可能以任意 uid 运行（真机：MT 管理器 helper 以
// fsuid=10000 写入，而 uid 10000 被某个 RRO 资源包占用，只按 uid 反查必然归错）。
fn resolve_caller_package_by_pid(pid: i32) -> String {
    if pid <= 0 {
        return String::new();
    }

    let path = format!("/proc/{}/cmdline", pid);
    // hook 侧只能用裸系统调用读文件：本进程装着 PLT hook，std::fs 会走被拦截的
    // open/read，既可能递归也会污染监视记录（见 read_file_by_syscall 注释）。
    let raw = read_file_by_syscall(&path, 4096);
    if raw.is_empty() {
        return String::new();
    }

    // 包名解析与 daemon 侧共用同一实现：优先 uid 表命中，其次 `@<pkg>-<ver>`
    // 形态，子进程名会截断到父包（HyperOS 的 `pkg.remote`）。
    if let Some(package) = crate::platform::proc_identity::parse_package_from_cmdline(&raw) {
        return package;
    }

    // 兜底保持包名语义：argv0 是包名（Java 进程）才返回；native 二进制（`sh`、
    // 下载器 helper、`/data/local/tmp/xxx`）没有包名，这里返回空，交由调用方按 pid
    // 继续解析。此前把 basename 当包名返回，规则必然查不到，写入就被当成「未配置
    // 应用」放行到真实存储（2026-11-05 真机 issue #2 的泄漏路径之一）。
    let mut head = raw.split('\0').next().unwrap_or("").to_string();
    if let Some(pos) = head.find(':') {
        head.truncate(pos);
    }
    let head = normalize_caller_package_text(&head);
    if head.contains('.') {
        head
    } else {
        String::new()
    }
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
        if policy::is_system_writer_package(&package) || policy::is_overlay_or_rro_package(&package)
        {
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
    // 裸系统调用直读见 `platform::proc_direct`：这里只取首行，故给一个足够容纳
    // 单行身份的最大读取量，超出部分由内核截断、不影响首行结果。
    let Some(text) = crate::platform::proc_direct::read_proc_file(path, 256) else {
        return String::new();
    };
    text.split(['\0', '\n', '\r'])
        .next()
        .unwrap_or_default()
        .to_string()
}

fn read_file_by_syscall(path: &str, max_len: usize) -> String {
    crate::platform::proc_direct::read_proc_file(path, max_len).unwrap_or_default()
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
/// 表由 root daemon 周期性重建（进程/线程集合变化即刷）；缓存只在 miss 且**文件身份
/// 变化**时重读，既不会被「过期」触发无谓重读，也不会错过 daemon 的每一次刷新。
struct SystemPidTable {
    /// 最近一次成功载入时的文件身份；`None` 表示还没读成功过。
    loaded_identity: Option<crate::platform::proc_direct::FileIdentity>,
    /// 最近一次 stat 的时刻，用于给 miss 风暴下的 stat 本身设上限。
    last_stat_ms: i64,
    /// 最近一次整表重读的时刻。仅当 stat 不可用（拿不到文件身份）时用作时间兜底，
    /// 否则可能退化成「每次 miss 都重读 512 KiB」。
    last_reload_ms: i64,
    entries: std::collections::HashMap<i32, (String, i32)>,
}

static SYSTEM_PID_TABLE: OnceLock<Mutex<SystemPidTable>> = OnceLock::new();
/// miss 路径上最短的 stat 间隔。
///
/// 命中路径完全不碰文件（与旧实现一致），所以这里只约束「未命中时先看一眼文件有没有
/// 变」这一条路径：200ms 足够把同一批 miss 合并成一次 stat，又不会让刚启动进程的
/// 首次写入等太久（真正决定延迟的是 daemon 的产表节奏）。
const SYSTEM_PID_TABLE_STAT_FLOOR_MS: i64 = 200;
/// stat 不可用时的重读下限。
///
/// 文件身份是「内容有没有变」的等价判据（daemon 走 tmp + rename，inode 必变）；只有在
/// stat 本身失败（表文件尚不存在、权限异常）时才会失去这个判据，此时退化成时间节流，
/// 避免每次 miss 都把整张表读一遍。
const SYSTEM_PID_TABLE_TIME_FALLBACK_MS: i64 = 2_000;

/// 按任务 id 反查 (包名, uid)。
///
/// 语义与旧实现一致：**命中直接返回**（不做任何 IO），未命中才考虑刷新缓存。区别只在
/// 刷新判据——旧实现是「按时间重读整张表」，现在是「按文件身份重读」：
///
/// 旧实现每次 miss 且距上次重读 ≥300ms 就把 512 KiB 的表读回来、解析近两万行、每行
/// 做一次 `String` 分配。开机与装应用的进程风暴期，miss 会把这件事放大到每秒数次，
/// 而读到的内容绝大多数时候与内存里一模一样。表由 daemon 通过 tmp + rename 原子替换，
/// inode 必然变化，因此 stat 一次就能判定是否需要重读，精度不变、读取量降到接近零。
pub(crate) fn lookup_system_pid_table(pid: i32) -> Option<(String, i32)> {
    let table_mutex = SYSTEM_PID_TABLE.get_or_init(|| {
        Mutex::new(SystemPidTable {
            loaded_identity: None,
            last_stat_ms: i64::MIN / 2,
            last_reload_ms: i64::MIN / 2,
            entries: std::collections::HashMap::new(),
        })
    });
    let mut table = table_mutex.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(entry) = table.entries.get(&pid) {
        return Some(entry.clone());
    }
    let now_ms = get_monotonic_ms();
    // stat 节流对**所有** miss 生效（包括「一次都没读成功过」的情形）：表文件缺席时
    // 每次 miss 都 stat 一遍同样是被流量放大的白工，而首次尝试不会被挡——初始的
    // `last_stat_ms` 是极小值。
    if now_ms.saturating_sub(table.last_stat_ms) < SYSTEM_PID_TABLE_STAT_FLOOR_MS {
        return None;
    }
    table.last_stat_ms = now_ms;
    let identity = crate::platform::proc_direct::stat_file_identity(&system_pid_table_path());
    if identity.is_some() && identity == table.loaded_identity {
        // 文件没变：缓存仍然有效，本次 miss 就是「表里确实还没有这个 pid」。
        // 调用方会退回按需询问（`daemon_identity_query`），结果同样来自 /proc。
        return None;
    }
    if identity.is_none()
        && table.loaded_identity.is_none()
        && now_ms.saturating_sub(table.last_reload_ms) < SYSTEM_PID_TABLE_TIME_FALLBACK_MS
    {
        // 拿不到文件身份（表文件还不存在等）：没有廉价的「变没变」判据，只能用时间节流。
        return None;
    }
    // 先读内容再决定是否记录文件身份：读失败/读到空内容时**不能**把 identity 记成
    // 「已载入」，否则下一次 miss 会认为「文件没变、缓存有效」而永远不再重读，
    // 整张表会一直保持为空（直到 daemon 下次真的改写文件）。
    let content = read_system_pid_table_file();
    if content.is_empty() {
        return None;
    }
    table.entries.clear();
    table.loaded_identity = identity;
    table.last_reload_ms = now_ms;
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
    table.entries.get(&pid).cloned()
}

fn get_monotonic_ms() -> i64 {
    crate::platform::paths::monotonic_ms()
}

/// 读系统进程 pid 表：hook 进程内经 Zygisk 保留 FD（/proc/self/fd/<fd>/tmp/...）
/// 读取；直接模块路径仅 root 可见，普通 hook 进程不可达时返回空。
fn read_system_pid_table_file() -> String {
    read_file_by_syscall(&system_pid_table_path(), 512 * 1024)
}

fn system_pid_table_path() -> String {
    let config_dir = SettingsHub::instance().config_dir();
    let module_dir = config_dir.strip_suffix("/config").unwrap_or(&config_dir);
    format!("{module_dir}/tmp/system_pids")
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
    if caller_uid < 0 {
        return;
    }
    // 系统 uid（< 应用起始值）的 binder 调用方此前被整体丢弃：ColorOS/HyperOS 上大量
    // 系统应用与共享组跑在 uid 1000/1001，它们在 MediaProvider 入口因此一条证据都留不下，
    // 记录里只剩 MP 自己的 uid。这里只写「监视记录专用」槽（uid + pid），不进
    // CALLER_UID / saved caller 的应用分支，重定向决策语义不变。
    context::set_monitor_caller_uid(caller_uid);
    if caller_pid > 0 {
        context::set_monitor_caller_pid(caller_pid);
    }
    // 同一份证据再存一份进程级副本：MediaProvider 的 binder 线程与 FuseDaemon 的
    // 处理线程不同，thread_local 到不了 FUSE 记录（见 context::remember_cross_thread_caller）。
    context::remember_cross_thread_caller(caller_uid, caller_pid);
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
        // 作用域仍按「无调用方」入栈（CALLER_UID 参与决策，不能塞系统 uid），
        // 但把 uid/pid 交给监视记录槽，记录才能写出真实请求方。
        if caller_uid >= 0 {
            context::set_monitor_caller_uid(caller_uid);
        }
        if caller_pid > 0 {
            context::set_monitor_caller_pid(caller_pid);
        }
        context::remember_cross_thread_caller(caller_uid, caller_pid);
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
        // 系统 uid 的命中也必须按当前 /proc 身份验证，不能直接采信周期 pid 表中的旧包名。
        if caller_uid < ANDROID_APP_UID_START {
            if let Some((package, resolved_uid)) =
                crate::daemon_identity_query::query_process_identity(caller_pid, caller_uid)
                && resolved_uid == caller_uid
            {
                return Some(package);
            }
            return None;
        }
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
