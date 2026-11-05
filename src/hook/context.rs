use crate::platform::paths::monotonic_ms;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};

// 单次 MediaStore 插入涉及的目录层级有限，用作登记表容量上限
const PROVIDER_PASSTHROUGH_VIRTUAL_DIR_MAX: usize = 16;

/// 跨线程 binder 调用方证据（进程级）。
///
/// MediaProvider 的 binder 线程（`ContentProvider$Transport#insert`）与 FuseDaemon
/// 的处理线程不是同一个线程：Java 侧捕获到的调用方 uid/pid 落在 `thread_local` 里，
/// 而在 FUSE 线程上生成的记录读不到，于是退回本进程自己的 uid —— 真机实测
/// `adb shell` 的 MediaStore insert 在记录里变成 `caller_uid=10217`（MP 自己）、
/// 来源未知，用户看到的"谁创建了这个目录"就答不上来。
///
/// 因此额外保留一份进程级证据，**只用于记录归因**：
/// - 仅在 uid 合法且不等于本进程 uid 时登记（自身的 I/O 不是"外部调用方"）；
/// - 读取限 [`CROSS_THREAD_CALLER_WINDOW_MS`] 窗口，过期不再采信，避免把上一笔
///   请求的调用方贴到随后无关的记录上；
/// - 不参与任何重定向决策（决策仍只认 thread_local 的 `CALLER_UID`）。
const CROSS_THREAD_CALLER_WINDOW_MS: i64 = 1_500;
static CROSS_THREAD_CALLER_UID: AtomicI32 = AtomicI32::new(-1);
static CROSS_THREAD_CALLER_PID: AtomicI32 = AtomicI32::new(-1);
static CROSS_THREAD_CALLER_TS_MS: AtomicI64 = AtomicI64::new(-1);

pub fn remember_cross_thread_caller(uid: i32, pid: i32) {
    if uid < 0 {
        return;
    }
    // SAFETY: getuid() 无参数、无副作用，只读进程真实 uid。
    let self_uid = unsafe { libc::getuid() } as i32;
    if uid == self_uid {
        return;
    }
    CROSS_THREAD_CALLER_UID.store(uid, Ordering::Relaxed);
    CROSS_THREAD_CALLER_PID.store(if pid > 0 { pid } else { -1 }, Ordering::Relaxed);
    CROSS_THREAD_CALLER_TS_MS.store(monotonic_ms(), Ordering::Relaxed);
}

/// 取进程级调用方证据；未登记或已过期返回 `None`。
pub fn cross_thread_caller_in_window() -> Option<(i32, i32)> {
    let ts = CROSS_THREAD_CALLER_TS_MS.load(Ordering::Relaxed);
    if ts < 0 {
        return None;
    }
    let uid = CROSS_THREAD_CALLER_UID.load(Ordering::Relaxed);
    if uid < 0 {
        return None;
    }
    let now = monotonic_ms();
    if now < ts || now - ts > CROSS_THREAD_CALLER_WINDOW_MS {
        return None;
    }
    Some((uid, CROSS_THREAD_CALLER_PID.load(Ordering::Relaxed)))
}

/// 记录用调用方 uid，返回 `(uid, 是否来自跨线程证据)`。
///
/// 与 [`get_effective_caller_uid_for_monitor`] 的区别：后者供归因**线索**使用，
/// 保持原有语义不变；本函数只在"记录会归到本进程自己"时追加跨线程证据，避免
/// 把外部调用方误当成自身 I/O。
pub fn record_caller_uid() -> (i32, bool) {
    // SAFETY: getuid() 无参数、无副作用，只读进程真实 uid。
    let self_uid = unsafe { libc::getuid() } as i32;
    // 直接证据按优先级取：监视槽（binder/fuse 给出的请求方）→ CALLER_UID（决策上下文，
    // 会被 FUSE 请求头填成本进程 uid）→ 当前线程 fsuid。
    let direct = monitor_caller_uid_in_window()
        .or_else(|| {
            let uid = CALLER_UID.with(|cell| cell.get());
            (uid >= 0).then_some(uid)
        })
        .unwrap_or_else(fsuid_from_proc_status);
    if direct != self_uid {
        return (direct, false);
    }
    // 归因会落到本进程自己（MediaProvider 代写）：这才是"外部调用方可能是谁"的
    // 唯一场景，用进程级证据补上。自身 I/O 没有外部调用方，保持原值。
    if let Some((external_uid, _)) = cross_thread_caller_in_window() {
        return (external_uid, true);
    }
    (direct, false)
}

/// 记录用调用方 pid：线程局部证据优先，缺失时（且本次 uid 来自跨线程证据）
/// 回退到同一份跨线程证据里的 pid。
pub fn record_caller_pid(cross_thread: bool) -> i32 {
    let pid = get_effective_caller_pid_for_monitor();
    if pid > 0 {
        return pid;
    }
    if cross_thread && let Some((_, external_pid)) = cross_thread_caller_in_window() {
        return external_pid;
    }
    -1
}

thread_local! {
    static CALLER_PACKAGE: RefCell<String> = const { RefCell::new(String::new()) };
    static CALLER_UID: Cell<i32> = const { Cell::new(-1) };
    static CALLER_TS_MS: Cell<i64> = const { Cell::new(-1) };
    static CALLER_FROM_EXTERNAL_SIGNAL: Cell<bool> = const { Cell::new(false) };
    // 归属来自「映射请求侧路径」推断（而非外部调用方信号）。MediaProvider 的
    // FuseDaemon 在 create/mkdir 之后用原始路径回查 attr 时没有外部调用方信号，
    // 只有这条归属可用；查询回退必须接受它，否则映射重定向后的应答会 ENOENT。
    static CALLER_FROM_MAPPING_REQUEST_OWNER: Cell<bool> = const { Cell::new(false) };
    static CALLER_SCOPE_STACK: RefCell<Vec<(String, i32)>> = const { RefCell::new(Vec::new()) };
    static FUSE_CALLER_UID: Cell<i32> = const { Cell::new(-1) };
    static FUSE_CALLER_UID_TS_MS: Cell<i64> = const { Cell::new(-1) };
    static FUSE_CALLER_PID: Cell<i32> = const { Cell::new(-1) };
    // 监视记录专用的请求方 pid：FUSE 请求头里的 tid 在解析结束时就被清掉
    // （clear_fuse_caller_uid），而记录是在解析之后才写的，所以需要另存一份。
    // 它不参与任何重定向决策，只用于记录里写出 caller_pid，让 daemon（root）
    // 能替 MP 反查读不到 /proc 的特权进程。
    static MONITOR_CALLER_PID: Cell<i32> = const { Cell::new(-1) };
    static MONITOR_CALLER_PID_TS_MS: Cell<i64> = const { Cell::new(-1) };
    /// 监视记录专用请求方 uid：与 CALLER_UID 分开存，理由见 set_monitor_caller_uid。
    static MONITOR_CALLER_UID: Cell<i32> = const { Cell::new(-1) };
    static MONITOR_CALLER_UID_TS_MS: Cell<i64> = const { Cell::new(-1) };
    static BINDER_SAVED_CALLER_UID: Cell<i32> = const { Cell::new(-1) };
    static BINDER_SAVED_CALLER_UID_TS_MS: Cell<i64> = const { Cell::new(-1) };
    static BINDER_SAVED_CALLER_PACKAGE: RefCell<String> = const { RefCell::new(String::new()) };
    static BINDER_IDENTITY_CLEARED: Cell<bool> = const { Cell::new(false) };
    static REENTRY_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PROVIDER_PASSTHROUGH_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PROVIDER_VIRTUAL_SCOPE_DEPTH: Cell<u32> = const { Cell::new(0) };
    // provider 变更期间被拦下的公共目录 -> 沙箱目标，仅用于让查询结果与 mkdir 结果保持一致
    static PROVIDER_PASSTHROUGH_VIRTUAL_DIRS: RefCell<Vec<(String, String)>> =
        const { RefCell::new(Vec::new()) };
    static EXPLICIT_CALLER_DECISION_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PATH_OWNER_INFERENCE_DISABLED_DEPTH: Cell<u32> = const { Cell::new(0) };
}

pub struct ReentryGuard;

impl ReentryGuard {
    pub fn enter() -> Self {
        REENTRY_DEPTH.with(|depth| depth.set(depth.get() + 1));
        ReentryGuard
    }

    pub fn is_reentrant() -> bool {
        REENTRY_DEPTH.with(|depth| depth.get() > 0)
    }
}

impl Drop for ReentryGuard {
    fn drop(&mut self) {
        REENTRY_DEPTH.with(|depth| {
            let current = depth.get();
            if current > 0 {
                depth.set(current - 1);
            }
        });
    }
}

pub struct ExplicitCallerDecisionGuard;

pub fn enter_explicit_caller_decision() -> ExplicitCallerDecisionGuard {
    EXPLICIT_CALLER_DECISION_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
    ExplicitCallerDecisionGuard
}

impl Drop for ExplicitCallerDecisionGuard {
    fn drop(&mut self) {
        EXPLICIT_CALLER_DECISION_DEPTH.with(|depth| {
            let current = depth.get();
            if current > 0 {
                depth.set(current - 1);
            }
        });
    }
}

pub fn is_explicit_caller_decision_active() -> bool {
    EXPLICIT_CALLER_DECISION_DEPTH.with(|depth| depth.get() > 0)
}

pub struct PathOwnerInferenceGuard;

pub fn enter_path_owner_inference_disabled() -> PathOwnerInferenceGuard {
    PATH_OWNER_INFERENCE_DISABLED_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
    PathOwnerInferenceGuard
}

impl Drop for PathOwnerInferenceGuard {
    fn drop(&mut self) {
        PATH_OWNER_INFERENCE_DISABLED_DEPTH.with(|depth| {
            let current = depth.get();
            if current > 0 {
                depth.set(current - 1);
            }
        });
    }
}

pub fn is_path_owner_inference_disabled() -> bool {
    PATH_OWNER_INFERENCE_DISABLED_DEPTH.with(|depth| depth.get() > 0)
}

pub fn set_current_caller_package(name: &str) {
    CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = name.to_string());
    CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
    CALLER_FROM_MAPPING_REQUEST_OWNER.with(|cell| cell.set(false));
}

pub fn get_current_caller_package() -> String {
    CALLER_PACKAGE.with(|pkg| pkg.borrow().clone())
}

pub fn set_current_caller_uid(uid: i32) {
    CALLER_UID.with(|cell| cell.set(uid));
    CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
    CALLER_FROM_MAPPING_REQUEST_OWNER.with(|cell| cell.set(false));
}

pub fn get_current_caller_uid() -> i32 {
    CALLER_UID.with(|cell| cell.get())
}

/// 监视记录用的有效调用方 uid：CALLER_UID 未设置时回退当前线程 fsuid。
///
/// MediaProvider 的 FuseDaemon 处理每个 FUSE 请求前会 setfsuid(请求方 uid)
/// （AOSP 标准做法，使下层文件以请求者身份创建）。未走 JNI rewrite 路径的
/// 原生 FUSE 请求（未配置应用对 /sdcard 的直接读写）没有 CALLER_UID 上下文，
/// 此时线程 fsuid 恰为触发请求的应用 uid，监视记录据此归因。
///
/// **仅用于监视记录**，不得用于重定向判定：重定向决策必须使用显式登记的
/// 调用方上下文，fsuid 兜底会把"恰好执行 I/O 的请求方"当成重定向调用方。
pub fn get_effective_caller_uid_for_monitor() -> i32 {
    // 监视专用槽优先：它记录的是 binder/fuse 给出的权威请求方 uid（含系统 uid），
    // 而 CALLER_UID 只承载「参与重定向决策」的调用方。
    if let Some(uid) = monitor_caller_uid_in_window() {
        return uid;
    }
    let uid = CALLER_UID.with(|cell| cell.get());
    if uid >= 0 {
        return uid;
    }
    fsuid_from_proc_status()
}

/// 与 [`get_effective_caller_pid_for_monitor`] 同窗口的 uid 读取。
pub fn monitor_caller_uid_in_window() -> Option<i32> {
    const MONITOR_CALLER_UID_WINDOW_MS: i64 = 2_000;
    let uid = MONITOR_CALLER_UID.with(|cell| cell.get());
    if uid < 0 {
        return None;
    }
    let now = monotonic_ms();
    MONITOR_CALLER_UID_TS_MS.with(|cell| {
        let ts = cell.get();
        if ts < 0 || now < ts || now - ts > MONITOR_CALLER_UID_WINDOW_MS {
            return None;
        }
        Some(uid)
    })
}

/// 从 /proc/self/status 的 `Uid:` 行读取当前线程 fsuid（该行四列依次为
/// 真实/有效/保存/文件系统 uid，fsuid 为第 4 列）。
///
/// **为什么不用 setfsuid(-1) 惯用法**：MediaProvider 等应用进程运行在 zygote
/// seccomp 沙箱内，`setfsuid`（arm64 syscall 151）不在白名单，调用即 SIGSYS
/// 杀死进程（2026-10-30 真机实测，MP 崩溃循环、存储 volume 不可用）。
/// **为什么不找 `FsUid:` 行**：status 没有 FsUid 字段（2026-10-30 duchamp
/// 实测仅 `Uid:` 行），fsuid 是 `Uid:` 行的第 4 列。
/// 改用 raw `SYS_openat` 直读 /proc/self/status：不经 PLT，不会被自身 hook
/// 拦截产生递归记录。
fn fsuid_from_proc_status() -> i32 {
    const STATUS_PATH: &[u8] = b"/proc/self/status\0";
    const READ_BUF_LEN: usize = 4096;
    // 失败只告警一次：定位用，避免刷屏。
    static FSUID_DIAG_WARNED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    // SAFETY: AT_FDCWD + NUL 结尾合法路径；raw syscall 不经 PLT，避免被自身
    // libc hook 拦截形成递归。读取失败返回 -1，由调用方按"归因不可用"处理。
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat,
            libc::AT_FDCWD,
            STATUS_PATH.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        ) as libc::c_int
    };
    if fd < 0 {
        if !FSUID_DIAG_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let errno = crate::platform::errno::last();
            log::warn!(
                "monitor fsuid /proc/self/status open failed errno={errno} {}",
                crate::platform::errno::text(errno)
            );
        }
        return -1;
    }
    let mut buf = [0u8; READ_BUF_LEN];
    // SAFETY: fd 为刚打开的合法描述符，buf 容量充足，单次读不满整个文件。
    let read_n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, READ_BUF_LEN) };
    // SAFETY: fd 为本函数打开的合法描述符，仅关闭一次。
    unsafe {
        libc::close(fd);
    }
    if read_n <= 0 {
        if !FSUID_DIAG_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            log::warn!("monitor fsuid /proc/self/status read failed n={read_n}");
        }
        return -1;
    }
    let text = &buf[..(read_n as usize).min(READ_BUF_LEN)];
    for line in text.split(|b| *b == b'\n') {
        if let Some(rest) = line.strip_prefix(b"Uid:") {
            // Uid: real effective saved fs —— fsuid 为第 4 列；status 用制表符
            // 分隔列，仅按空格切分会让四列粘成一列（真机实测）。
            let fields: Vec<&[u8]> = rest
                .split(|b| *b == b' ' || *b == b'\t')
                .filter(|f| !f.is_empty())
                .collect();
            return fields
                .get(3)
                .and_then(|fs| std::str::from_utf8(fs).ok())
                .and_then(|fs| fs.trim().parse().ok())
                .unwrap_or_else(|| {
                    if !FSUID_DIAG_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        log::warn!(
                            "monitor fsuid parse failed line={}",
                            String::from_utf8_lossy(line)
                        );
                    }
                    -1
                });
        }
    }
    if !FSUID_DIAG_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        log::warn!(
            "monitor fsuid Uid line missing: {}",
            String::from_utf8_lossy(&text[..text.len().min(256)])
        );
    }
    -1
}

pub fn get_current_caller_age_ms() -> i64 {
    let now = monotonic_ms();
    CALLER_TS_MS.with(|cell| {
        let ts = cell.get();
        if ts < 0 || now < ts {
            return -1;
        }
        now - ts
    })
}

pub fn clear_current_caller() {
    CALLER_PACKAGE.with(|pkg| pkg.borrow_mut().clear());
    CALLER_UID.with(|cell| cell.set(-1));
    CALLER_TS_MS.with(|cell| cell.set(-1));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
    CALLER_FROM_MAPPING_REQUEST_OWNER.with(|cell| cell.set(false));
}

pub fn set_current_caller_from_external_signal(package_name: &str, uid: i32) {
    CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = package_name.to_string());
    CALLER_UID.with(|cell| cell.set(uid));
    CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(true));
}

pub fn is_current_caller_from_external_signal() -> bool {
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.get())
}

/// 归属来自映射请求侧路径推断时，登记调用方并标记来源。
///
/// 与外部调用方信号互斥：外部信号代表「确有应用在发起本次调用」，本信号代表
/// 「本次调用的路径属于某应用的映射请求侧」。查询回退两条都接受，写路径决策
/// 仍按原有 external 语义区分。
pub fn set_current_caller_from_mapping_request_owner(package_name: &str, uid: i32) {
    CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = package_name.to_string());
    CALLER_UID.with(|cell| cell.set(uid));
    CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
    CALLER_FROM_MAPPING_REQUEST_OWNER.with(|cell| cell.set(true));
}

pub fn is_current_caller_from_mapping_request_owner() -> bool {
    CALLER_FROM_MAPPING_REQUEST_OWNER.with(|cell| cell.get())
}

pub fn push_current_caller_scope(package_name: &str, uid: i32) {
    let previous_package = get_current_caller_package();
    let previous_uid = get_current_caller_uid();
    CALLER_SCOPE_STACK.with(|stack| {
        stack.borrow_mut().push((previous_package, previous_uid));
    });
    CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = package_name.to_string());
    CALLER_UID.with(|cell| cell.set(uid));
    CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
    CALLER_FROM_MAPPING_REQUEST_OWNER.with(|cell| cell.set(false));
}

pub fn pop_current_caller_scope() {
    let previous = CALLER_SCOPE_STACK.with(|stack| stack.borrow_mut().pop());
    if let Some((package_name, uid)) = previous {
        CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = package_name);
        CALLER_UID.with(|cell| cell.set(uid));
        if uid >= 0 {
            CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
        } else {
            CALLER_TS_MS.with(|cell| cell.set(-1));
        }
        CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
        CALLER_FROM_MAPPING_REQUEST_OWNER.with(|cell| cell.set(false));
    } else {
        clear_current_caller();
    }
}

pub fn is_current_caller_scope_active() -> bool {
    CALLER_SCOPE_STACK.with(|stack| !stack.borrow().is_empty())
}

pub fn set_fuse_caller_uid(uid: i32) {
    FUSE_CALLER_UID.with(|cell| cell.set(uid));
    FUSE_CALLER_UID_TS_MS.with(|cell| cell.set(monotonic_ms()));
}

pub fn set_fuse_caller_pid(pid: i32) {
    FUSE_CALLER_PID.with(|cell| cell.set(pid));
}

pub fn get_fuse_caller_uid() -> i32 {
    FUSE_CALLER_UID.with(|cell| cell.get())
}

pub fn get_fuse_caller_pid() -> i32 {
    FUSE_CALLER_PID.with(|cell| cell.get())
}

// 未缓存返回 -1
pub fn get_fuse_caller_uid_age_ms() -> i64 {
    let now = monotonic_ms();
    FUSE_CALLER_UID_TS_MS.with(|cell| {
        let ts = cell.get();
        if ts < 0 || now < ts {
            return -1;
        }
        now - ts
    })
}

/// 记下本次请求的请求方 pid（内核 FUSE 头给出的 tid），供监视记录写出
/// `caller_pid=`。每个请求开始时由 `update_caller_package_for_current_thread`
/// 复位，所以不会串到下一个请求；pid <= 0 视为「本次没有 pid 证据」。
pub fn set_monitor_caller_pid(pid: i32) {
    MONITOR_CALLER_PID.with(|cell| cell.set(pid));
    MONITOR_CALLER_PID_TS_MS.with(|cell| cell.set(if pid > 0 { monotonic_ms() } else { -1 }));
}

/// 记录「这次请求的请求方 uid」，专供监视记录使用。
///
/// 为什么不写进 `CALLER_UID`：后者参与重定向决策（多处按 uid 区间早退），把系统 uid
/// 塞进去等于改变决策输入；而记录只需要如实写出「谁发起了这次写入」。ColorOS/HyperOS
/// 上大量系统应用与共享组跑在 uid 1000/1001（< 应用起始值），此前这些 binder 调用
/// 被整体丢弃，记录里只剩 MediaProvider 自己的 uid。
pub fn set_monitor_caller_uid(uid: i32) {
    MONITOR_CALLER_UID.with(|cell| cell.set(uid));
    MONITOR_CALLER_UID_TS_MS.with(|cell| cell.set(if uid >= 0 { monotonic_ms() } else { -1 }));
}

/// 监视记录用请求方 pid：超过窗口一律视为无证据，避免把上一次请求的 pid
/// 贴到随后无 pid 的记录上（那会让 daemon 反查出错误的源头）。
pub fn get_effective_caller_pid_for_monitor() -> i32 {
    const MONITOR_CALLER_PID_WINDOW_MS: i64 = 2_000;
    let pid = MONITOR_CALLER_PID.with(|cell| cell.get());
    if pid <= 0 {
        return -1;
    }
    let now = monotonic_ms();
    MONITOR_CALLER_PID_TS_MS.with(|cell| {
        let ts = cell.get();
        if ts < 0 || now < ts || now - ts > MONITOR_CALLER_PID_WINDOW_MS {
            return -1;
        }
        pid
    })
}

// 线程复用前必清，避免残留 UID/PID 串到后续请求
pub fn clear_fuse_caller_uid() {
    FUSE_CALLER_UID.with(|cell| cell.set(-1));
    FUSE_CALLER_UID_TS_MS.with(|cell| cell.set(-1));
    FUSE_CALLER_PID.with(|cell| cell.set(-1));
}

/// 系统 uid FUSE 请求的跨线程接力槽。
///
/// MediaProvider 侧读 /dev/fuse 的线程（libappfuse 桥）与处理请求、执行后端
/// mkdir/open 的线程可能分离：thread-local 的 FUSE 请求头信号无法跨线程传递
/// （2026-10-24 真机：系统服务开机创建 MIUI/Mirror 时 fuse_uid=-1）。读线程
/// 捕获到系统 uid（<10000）的请求头时写入本槽，处理线程在归因落空时按时间
/// 窗口取用。只对系统 uid 入槽：应用 uid 的请求已有 binder 与路径台账归因，
/// 全局单槽在应用 IO 并发下会串味，收益不抵风险。
///
/// 取用资格按产品边界收窄：仅公共路径的写形态操作（经
/// caller::update_caller_for_mutation 标记）允许取槽——MP 的高频读操作与
/// 私有路径写会把待归因的公共路径写请求的槽内容提前消费掉（2026-10-24
/// 真机：mkdir 槽内容被并发 stat 秒抢，监视记录全部 unknown）。
struct SystemFuseRequestSlot {
    uid: i32,
    pid: i32,
    at_ms: i64,
}

static SYSTEM_FUSE_REQUEST: std::sync::Mutex<SystemFuseRequestSlot> =
    std::sync::Mutex::new(SystemFuseRequestSlot {
        uid: -1,
        pid: -1,
        at_ms: -1,
    });

pub fn set_system_fuse_request(uid: i32, pid: i32) {
    if uid < 0 || pid <= 0 {
        return;
    }
    let mut slot = SYSTEM_FUSE_REQUEST
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    slot.uid = uid;
    slot.pid = pid;
    slot.at_ms = monotonic_ms();
}

/// 取窗口内的系统 uid FUSE 请求；取用即清，避免同一次请求被重复消费。
pub fn take_recent_system_fuse_request(max_age_ms: i64) -> Option<(i32, i32)> {
    let mut slot = SYSTEM_FUSE_REQUEST
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if slot.uid < 0 || slot.pid <= 0 {
        return None;
    }
    let now = monotonic_ms();
    let age = now.saturating_sub(slot.at_ms);
    let (uid, pid) = (slot.uid, slot.pid);
    slot.uid = -1;
    slot.pid = -1;
    slot.at_ms = -1;
    if age >= 0 && age <= max_age_ms {
        return Some((uid, pid));
    }
    None
}

// 在 clearCallingIdentity 之前调用，保存真实调用方 UID
pub fn set_binder_saved_caller_uid(uid: i32) {
    BINDER_SAVED_CALLER_UID.with(|cell| cell.set(uid));
    BINDER_SAVED_CALLER_UID_TS_MS.with(|cell| cell.set(monotonic_ms()));
    BINDER_SAVED_CALLER_PACKAGE.with(|pkg| pkg.borrow_mut().clear());
}

pub fn set_binder_saved_caller_package(package_name: &str) {
    BINDER_SAVED_CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = package_name.to_string());
    BINDER_SAVED_CALLER_UID_TS_MS.with(|cell| cell.set(monotonic_ms()));
}

pub fn get_binder_saved_caller_uid() -> i32 {
    BINDER_SAVED_CALLER_UID.with(|cell| cell.get())
}

pub fn get_binder_saved_caller_package() -> String {
    BINDER_SAVED_CALLER_PACKAGE.with(|pkg| pkg.borrow().clone())
}

// 未缓存返回 -1
pub fn get_binder_saved_caller_uid_age_ms() -> i64 {
    let now = monotonic_ms();
    BINDER_SAVED_CALLER_UID_TS_MS.with(|cell| {
        let ts = cell.get();
        if ts < 0 || now < ts {
            return -1;
        }
        now - ts
    })
}

// 跨请求前必清，避免误用历史 UID
pub fn clear_binder_saved_caller_uid() {
    BINDER_SAVED_CALLER_UID.with(|cell| cell.set(-1));
    BINDER_SAVED_CALLER_UID_TS_MS.with(|cell| cell.set(-1));
    BINDER_SAVED_CALLER_PACKAGE.with(|pkg| pkg.borrow_mut().clear());
}

pub fn enter_provider_passthrough() {
    PROVIDER_PASSTHROUGH_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
}

pub fn exit_provider_passthrough() -> Vec<(String, String)> {
    PROVIDER_PASSTHROUGH_DEPTH.with(|depth| {
        let current = depth.get();
        if current <= 1 {
            depth.set(0);
            let virtual_dirs = take_provider_virtual_dirs_if_idle();
            if !is_current_caller_scope_active() {
                clear_current_caller();
                clear_binder_saved_caller_uid();
                set_binder_identity_cleared(false);
            }
            virtual_dirs
        } else {
            depth.set(current - 1);
            Vec::new()
        }
    })
}

pub fn is_provider_passthrough_active() -> bool {
    PROVIDER_PASSTHROUGH_DEPTH.with(|depth| depth.get() > 0)
}

pub fn enter_provider_virtual_scope() {
    PROVIDER_VIRTUAL_SCOPE_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
}

pub fn exit_provider_virtual_scope() -> Vec<(String, String)> {
    PROVIDER_VIRTUAL_SCOPE_DEPTH.with(|depth| {
        let current = depth.get();
        if current <= 1 {
            depth.set(0);
            take_provider_virtual_dirs_if_idle()
        } else {
            depth.set(current - 1);
            Vec::new()
        }
    })
}

pub fn is_provider_virtual_scope_active() -> bool {
    PROVIDER_VIRTUAL_SCOPE_DEPTH.with(|depth| depth.get() > 0)
}

fn take_provider_virtual_dirs_if_idle() -> Vec<(String, String)> {
    if is_provider_passthrough_active() || is_provider_virtual_scope_active() {
        return Vec::new();
    }
    PROVIDER_PASSTHROUGH_VIRTUAL_DIRS.with(|dirs| std::mem::take(&mut *dirs.borrow_mut()))
}

// 登记键统一折算到 /storage/emulated 路径空间：mkdir 的判定路径可能是 /data/media 形式，
// 而后续查询按归一化路径查表，两侧不一致会导致查不到已登记的目录。
fn virtual_dir_key(path: &str) -> String {
    crate::platform::paths::data_media_to_storage_path(path)
}

// 记录一个「未在公共目录落地、实际重定向到沙箱」的目录，供同一直通窗口内的查询使用
pub fn remember_provider_passthrough_virtual_dir(public_path: &str, sandbox_path: &str) {
    if public_path.is_empty() || sandbox_path.is_empty() {
        return;
    }
    let key = virtual_dir_key(public_path);
    PROVIDER_PASSTHROUGH_VIRTUAL_DIRS.with(|dirs| {
        let mut dirs = dirs.borrow_mut();
        if dirs.iter().any(|(public, _)| public == &key) {
            return;
        }
        // 单次插入涉及的目录层级有限，设上限避免异常场景下无界增长
        if dirs.len() >= PROVIDER_PASSTHROUGH_VIRTUAL_DIR_MAX {
            dirs.remove(0);
        }
        dirs.push((key, sandbox_path.to_string()));
    })
}

// 虚拟目录对应的沙箱目标，仅用于目录内容列举：列出沙箱内容才与应用可见状态一致。
pub fn provider_passthrough_virtual_dir_target(public_path: &str) -> Option<String> {
    if public_path.is_empty() {
        return None;
    }
    let key = virtual_dir_key(public_path);
    PROVIDER_PASSTHROUGH_VIRTUAL_DIRS.with(|dirs| {
        dirs.borrow()
            .iter()
            .find(|(public, _)| public == &key)
            .map(|(_, sandbox)| sandbox.clone())
    })
}

// 该公共目录是否属于本次直通窗口内被拦下、实际未在公共路径落地的目录
pub fn is_provider_passthrough_virtual_dir(public_path: &str) -> bool {
    if public_path.is_empty() {
        return false;
    }
    let key = virtual_dir_key(public_path);
    PROVIDER_PASSTHROUGH_VIRTUAL_DIRS.with(|dirs| dirs.borrow().iter().any(|(p, _)| p == &key))
}

// 返回 Provider 变更期内登记的沙箱目标。调用方必须继续检查目标文件本身，不能把
// 「虚拟目录存在」误当成「目录内任意候选文件都存在」。
pub fn provider_passthrough_virtual_query_target(path: &str) -> Option<String> {
    if (!is_provider_passthrough_active() && !is_provider_virtual_scope_active()) || path.is_empty()
    {
        return None;
    }
    let key = virtual_dir_key(path);
    PROVIDER_PASSTHROUGH_VIRTUAL_DIRS.with(|dirs| {
        let dirs = dirs.borrow();
        let (public, sandbox) = dirs
            .iter()
            .filter(|(public, _)| key == *public || key.starts_with(&format!("{public}/")))
            .max_by_key(|(public, _)| public.len())?;
        let suffix = key.strip_prefix(public)?.trim_start_matches('/');
        if suffix.is_empty() {
            Some(sandbox.clone())
        } else {
            Some(format!("{sandbox}/{suffix}"))
        }
    })
}

// 返回 Provider 变更期内应按虚拟目录回答查询的目录路径。MediaStore 会先创建公共父目录，
// 随后对 .pending-* 文件执行存在性检查；两者必须落到同一份线程内登记状态。
pub fn provider_passthrough_virtual_query_dir(path: &str) -> Option<String> {
    if (!is_provider_passthrough_active() && !is_provider_virtual_scope_active()) || path.is_empty()
    {
        return None;
    }
    if is_provider_passthrough_virtual_dir(path) {
        return Some(path.to_string());
    }
    crate::platform::paths::media_store_pending_display_path(path)?;
    let parent = crate::platform::paths::parent(path);
    if !parent.is_empty() && is_provider_passthrough_virtual_dir(&parent) {
        Some(parent)
    } else {
        None
    }
}

// ── 进程级「映射请求路径 → 映射目标」短时登记 ─────────────────────────────
//
// 系统写者把 mkdir/create 落到映射目标之后，会用**原请求路径**回查 attr、做
// realpath 校验；而请求路径在真实存储里并不存在（它只是应用视图里的 bind
// 挂载点），这些回查一律 ENOENT，FUSE 最终把 ENOENT 回给调用方。2026-10-26
// A17 场景 39 实证：DownloadProvider 的 mkdir 拿到 -1/ENOENT，映射目标里的
// 目录却已由 MediaProvider 建好。
//
// 线程内的 PROVIDER_PASSTHROUGH_VIRTUAL_DIRS 覆盖不到这条链路：FuseDaemon 的
// 写请求与后续回查落在不同线程上（binder 线程池 + FUSE 读线程）。这里登记到
// 进程级并带 TTL，只用于回答查询，不改变落盘位置。
const PROCESS_MAPPED_DIR_TTL_MS: i64 = 8000;
// 单个下载/插入流程涉及的目录层级有限，设上限避免异常场景下无界增长
const PROCESS_MAPPED_DIR_MAX: usize = 64;

struct ProcessMappedDir {
    request: String,
    target: String,
    at_ms: i64,
}

static PROCESS_MAPPED_DIRS: std::sync::Mutex<Vec<ProcessMappedDir>> =
    std::sync::Mutex::new(Vec::new());

/// 登记「本进程内刚把某请求侧目录重定向到了映射目标」。
///
/// 键统一折算到 /storage/emulated 路径空间，与查询侧 `virtual_dir_key` 一致。
pub fn remember_process_mapped_dir(request_path: &str, target_path: &str) {
    if request_path.is_empty() || target_path.is_empty() {
        return;
    }
    let request = virtual_dir_key(request_path);
    let target = crate::platform::paths::data_media_to_storage_path(target_path);
    let now = monotonic_ms();
    let mut dirs = PROCESS_MAPPED_DIRS
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    dirs.retain(|dir| now.saturating_sub(dir.at_ms) <= PROCESS_MAPPED_DIR_TTL_MS);
    if let Some(existing) = dirs.iter_mut().find(|dir| dir.request == request) {
        existing.target = target;
        existing.at_ms = now;
        return;
    }
    if dirs.len() >= PROCESS_MAPPED_DIR_MAX {
        dirs.remove(0);
    }
    dirs.push(ProcessMappedDir {
        request,
        target,
        at_ms: now,
    });
}

/// 登记项对应的映射目标（含子路径派生），过期项在读取时就地清理。
///
/// 只做纯查询：命中不代表该路径在真实存储里存在，调用方仍须自行验证目标。
pub fn process_mapped_dir_target(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let key = virtual_dir_key(path);
    let now = monotonic_ms();
    let mut dirs = PROCESS_MAPPED_DIRS
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    dirs.retain(|dir| now.saturating_sub(dir.at_ms) <= PROCESS_MAPPED_DIR_TTL_MS);
    let (request, target) = dirs
        .iter()
        .filter(|dir| key == dir.request || key.starts_with(&format!("{}/", dir.request)))
        .max_by_key(|dir| dir.request.len())
        .map(|dir| (dir.request.clone(), dir.target.clone()))?;
    let suffix = key.strip_prefix(&request)?.trim_start_matches('/');
    if suffix.is_empty() {
        Some(target)
    } else {
        Some(format!("{target}/{suffix}"))
    }
}

/// 当前有效登记条数（过期项就地清理）。
///
/// 只用于诊断：查询回退未命中时，`0` 说明「同进程内从没登记过这次映射」，非 `0`
/// 则说明登记存在但路径形式对不上——这两种情况在修复动作上完全不同。
pub fn process_mapped_dir_count() -> usize {
    let now = monotonic_ms();
    let mut dirs = PROCESS_MAPPED_DIRS
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    dirs.retain(|dir| now.saturating_sub(dir.at_ms) <= PROCESS_MAPPED_DIR_TTL_MS);
    dirs.len()
}

pub fn set_binder_identity_cleared(cleared: bool) {
    BINDER_IDENTITY_CLEARED.with(|cell| cell.set(cleared));
}

// 处于 clearCallingIdentity 后、restoreCallingIdentity 前的区间
pub fn is_binder_identity_cleared() -> bool {
    BINDER_IDENTITY_CLEARED.with(|cell| cell.get())
}
