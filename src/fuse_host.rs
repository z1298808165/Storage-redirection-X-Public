//! 共享宿主 FUSE 会话（阶段 2 基础设施，B2-a/B2-b）。
//!
//! 目标是把「每应用各自 fork 一个 FUSE 服务」改成「daemon 持有一个持久共享会话，各应用
//! namespace 用 `MS_BIND` 引用」。B2-a 搭起宿主会话骨架，B2-b 实现应用侧接入逻辑。

use crate::platform::paths;
use std::ffi::CString;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// 宿主挂载点：放在模块私有目录下，不落在任何存储别名上，避免与应用命名空间发生传播耦合。
const FUSE_HOST_MOUNT_POINT: &str = "/data/adb/modules/storage.redirect.x/tmp/fuse_host";

/// 宿主会话的就绪等待上限（秒）。
const HOST_READY_TIMEOUT_SEC: i64 = 30;
/// 宿主恢复失败后的最短重试间隔，避免每个 reconcile 周期重复 fork。
const HOST_RECOVERY_BACKOFF_MS: u64 = 5_000;

static LAST_RECOVERY_ATTEMPT_MS: AtomicI64 = AtomicI64::new(0);

/// 可替换的全局共享宿主会话句柄。
///
/// 共享宿主子进程可能独立退出；使用可写槽位允许 daemon 在下一轮 reconcile 中
/// 丢弃失效句柄并重建宿主，应用侧读取时仍只拿到当前有效快照。
static FUSE_HOST: OnceLock<RwLock<Option<Arc<FuseHost>>>> = OnceLock::new();

fn host_slot() -> &'static RwLock<Option<Arc<FuseHost>>> {
    FUSE_HOST.get_or_init(|| RwLock::new(None))
}

fn host_is_alive(host: &FuseHost) -> bool {
    crate::platform::is_process_instance_alive(host.child_pid, host.child_start_time_ticks)
}

fn host_is_dead(host: &FuseHost) -> bool {
    !host_is_alive(host)
}

/// 已建立的共享宿主会话句柄。
pub struct FuseHost {
    /// 持有宿主 mount namespace 的子进程 pid。
    pub child_pid: i32,
    /// 子进程启动时刻（clock ticks），用于区分 pid 复用。
    pub child_start_time_ticks: u64,
    /// 宿主挂载点。
    pub mount_point: String,
    /// 宿主会话挂载源（`srx_fuse_host[<pid>]`）。
    pub mount_source: String,
}

impl Drop for FuseHost {
    fn drop(&mut self) {
        if !crate::platform::is_process_instance_alive(self.child_pid, self.child_start_time_ticks)
        {
            return;
        }
        // 宿主会话由子进程持有；停止 daemon 时先终止它，让 FUSE fd 关闭并回收挂载。
        // SAFETY: pid 已用启动时刻校验，避免误杀复用后的其它进程。
        unsafe {
            libc::kill(self.child_pid, libc::SIGTERM);
            libc::waitpid(self.child_pid, std::ptr::null_mut(), 0);
        }
    }
}

fn log_errno(tag: &str) {
    let errno = crate::platform::errno::last();
    log::error!(
        "{} errno={} {}",
        tag,
        errno,
        crate::platform::errno::text(errno)
    );
}

/// 宿主子进程阶段埋点文件。
///
/// 宿主子进程在 fork 之后、任何一行日志写出之前就可能崩溃退出；service.sh 又把 daemon 的
/// stderr 丢到 `/dev/null`，panic 信息不会留下来。写文件不依赖私有日志通道，也不依赖
/// 标准错误，因此作为最后兜底的取证手段：即使父进程只看到"未就绪"，也能从阶段行判断
/// 子进程走到哪一步。
const FUSE_HOST_STAGE_PATH: &str = "/data/adb/modules/storage.redirect.x/tmp/fuse_host.stage";

fn append_bytes(buffer: &mut [u8], len: &mut usize, bytes: &[u8]) {
    for byte in bytes {
        if *len < buffer.len() {
            buffer[*len] = *byte;
            *len += 1;
        }
    }
}

fn append_decimal(buffer: &mut [u8], len: &mut usize, value: u32) {
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    let mut rest = value;
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        rest /= 10;
        count += 1;
        if rest == 0 || count == digits.len() {
            break;
        }
    }
    while count > 0 {
        count -= 1;
        if *len < buffer.len() {
            buffer[*len] = digits[count];
            *len += 1;
        }
    }
}

/// 记录宿主子进程已到达的阶段。
///
/// 全程只用栈缓冲与系统调用：fork 之后不再依赖堆分配，避免父进程其它线程在 fork 瞬间
/// 持有分配器锁时把子进程卡死（那会让本函数本身变成新的故障点）。
pub(crate) fn host_stage(stage: &str) {
    host_stage_bytes(stage.as_bytes());
}

fn host_stage_bytes(stage: &[u8]) {
    let mut buffer = [0u8; 192];
    let mut len = 0usize;
    append_bytes(&mut buffer, &mut len, b"pid=");
    append_decimal(&mut buffer, &mut len, std::process::id());
    append_bytes(&mut buffer, &mut len, b" stage=");
    append_bytes(&mut buffer, &mut len, stage);
    append_bytes(&mut buffer, &mut len, b"\n");
    let Ok(path) = CString::new(FUSE_HOST_STAGE_PATH) else {
        return;
    };
    // SAFETY: path 是合法 NUL 结尾路径；buffer 与 len 一致，fd 在同一块内关闭。
    unsafe {
        let fd = libc::open(
            path.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
            0o644,
        );
        if fd < 0 {
            return;
        }
        libc::write(fd, buffer.as_ptr() as *const libc::c_void, len);
        libc::close(fd);
    }
}

/// 在宿主子进程内安装 panic 钩子。
///
/// release 构建使用 `panic = "abort"`，`catch_unwind` 无法截获 panic，子进程会直接 abort；
/// 而 service.sh 把 daemon 的 stderr 丢到 `/dev/null`，panic 位置不会留下任何记录。
/// 钩子执行在 abort 之前，是唯一还能落盘的时机。
fn install_host_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let mut text = [0u8; 96];
        let mut len = 0usize;
        append_bytes(&mut text, &mut len, b"panic:");
        if let Some(location) = info.location() {
            append_bytes(&mut text, &mut len, location.file().as_bytes());
            append_bytes(&mut text, &mut len, b":");
            append_decimal(&mut text, &mut len, location.line());
        }
        host_stage_bytes(&text[..len]);
    }));
}

/// 共享宿主会话当前是否可以安全承接指定调用方的挂载。
///
/// 宿主会话只持有一份"直通"策略，而按 uid 注册应用策略的通道尚未实现
/// （`PolicyRegistry::by_uid` 目前恒为空，且没有注册入口）。此时若把宿主树 `MS_BIND`
/// 到应用存储根，应用会拿到**纯直通视图**，即完全失去重定向——而且这种回退不会报错，
/// 只会表现为"规则莫名其妙不生效"。
///
/// 因此这里是一道能力闸门，而不是失败兜底：在按 uid 注册落地前，应用接入一律关闭，
/// 继续走已经稳定的 scoped FUSE 路径；宿主会话本身照常建立并保持，供后续接入使用。
pub fn can_attach_app(uid: i32) -> bool {
    let _ = uid;
    false
}

/// 清空阶段文件：每次尝试只保留本轮痕迹，避免多轮重试互相覆盖后无法判断当轮结果。
fn reset_host_stage() {
    let _ = std::fs::write(FUSE_HOST_STAGE_PATH, b"");
}

/// 把阶段文件内容写入日志。
///
/// 宿主失败时父进程只能看到"未就绪"，真正的原因在子进程侧；把阶段行转写到 running.log
/// 后，CI 与设备日志都能直接定位失败阶段，不必再额外取文件。
fn log_host_stage_trace() {
    let Ok(text) = std::fs::read_to_string(FUSE_HOST_STAGE_PATH) else {
        return;
    };
    for line in text.lines().take(12) {
        if !line.is_empty() {
            log::warn!("fuse host stage trace {}", line);
        }
    }
}

/// 回收宿主子进程并返回退出原因描述。
///
/// 退出原因本身也是关键证据：`exit=1` 表示子进程走到了失败返回，`signal=9` 表示它没有
/// 自己退出（由本次强杀收尾，即卡在某个阶段），`signal=11/6` 表示崩溃。回收是强制的，
/// 否则每轮恢复都会留下一个僵尸进程。
fn reap_host_child(pid: libc::pid_t) -> String {
    let mut status: libc::c_int = 0;
    let mut reaped = false;
    // SAFETY: pid 来自本次 fork；先终止再回收，且回收循环带次数上限，不会无限阻塞。
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        for _ in 0..50 {
            if libc::waitpid(pid, &mut status as *mut libc::c_int, libc::WNOHANG) == pid {
                reaped = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    if !reaped {
        return "exit=unreaped".to_string();
    }
    if libc::WIFSIGNALED(status) {
        format!("signal={}", libc::WTERMSIG(status))
    } else if libc::WIFEXITED(status) {
        format!("exit={}", libc::WEXITSTATUS(status))
    } else {
        "exit=unknown".to_string()
    }
}

/// 建立共享宿主会话。失败返回 `None`（不抛出），调用方据此降级而不影响主循环。
pub fn spawn_fuse_host() -> Option<FuseHost> {
    let mut ready_sockets = [0; 2];
    // SAFETY: ready_sockets 是本地数组，长度合法，socketpair 填充两个 fd。
    // 允许英文：系统调用 API 名称保持原样以便检索。
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_DGRAM,
            0,
            ready_sockets.as_mut_ptr(),
        )
    } != 0
    {
        log_errno("fuse host ready socketpair failed");
        return None;
    }

    reset_host_stage();
    crate::logging::prepare_for_fork();
    // SAFETY: fork 前已完成日志/信号准备，匹配现有 daemon 挂载的 fork 模式。
    // 允许英文：fork 是系统调用名称。
    let child = unsafe { libc::fork() };
    if child < 0 {
        log_errno("宿主会话 fork 失败");
        // SAFETY: 两个 fd 均为本次 socketpair 打开，且此处是唯一清理点。
        unsafe {
            libc::close(ready_sockets[0]);
            libc::close(ready_sockets[1]);
        }
        return None;
    }

    if child == 0 {
        host_stage("child_enter");
        // SAFETY: 子进程关闭父端，随后进入常驻入口；该入口只在完成或失败时返回。
        unsafe { libc::close(ready_sockets[0]) };
        let name = b"srx_fuse_host\0";
        // SAFETY: prctl(PR_SET_NAME) 设置线程名称，name 是 NUL 结尾的静态字节串。
        // 允许英文：prctl 是系统调用名称。
        unsafe {
            libc::prctl(libc::PR_SET_NAME, name.as_ptr() as libc::c_ulong, 0, 0, 0);
        }
        // 捕获 panic：子进程的 panic 信息会写进 stderr，而 service.sh 已把它丢弃，
        // 只留下"未就绪"这种无法定位的现象。这里改为落到阶段文件，保留可取证信息。
        install_host_panic_hook();
        let ok = fuse_host_child_main(ready_sockets[1]);
        host_stage(if ok { "child_ok" } else { "child_failed" });
        // SAFETY: _exit 终止子进程，不跑 atexit；子进程用完即退出，无需清理栈。
        // 允许英文：_exit 是系统调用名称。
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // SAFETY: 父进程关闭子端，然后等待就绪结果。
    unsafe { libc::close(ready_sockets[1]) };
    let ready = recv_host_ready(ready_sockets[0]);
    // SAFETY: ready_sockets[0] 是父进程持有的有效 socketpair 端点。
    // 允许英文：close 是系统调用名称。
    unsafe { libc::close(ready_sockets[0]) };
    match ready {
        Some(0) => {}
        Some(code) => {
            let reason = reap_host_child(child);
            log::warn!(
                "fuse host not ready child={} stage={} code={} {}",
                child,
                crate::fuse_redirect::config::host_ready_stage(code),
                code,
                reason
            );
            log_host_stage_trace();
            return None;
        }
        None => {
            let reason = reap_host_child(child);
            log::warn!(
                "fuse host ready unavailable child={} timeout_sec={} {}",
                child,
                HOST_READY_TIMEOUT_SEC,
                reason
            );
            log_host_stage_trace();
            return None;
        }
    }

    let Some(child_start_time_ticks) = crate::platform::process_start_time_ticks(child) else {
        let reason = reap_host_child(child);
        log::warn!(
            "fuse host start time unavailable child={} {}",
            child,
            reason
        );
        return None;
    };
    Some(FuseHost {
        child_pid: child,
        child_start_time_ticks,
        mount_point: FUSE_HOST_MOUNT_POINT.to_string(),
        mount_source: crate::fuse_redirect::config::host_mount_source(child as u32),
    })
}

/// 子进程入口：进入私有 namespace、挂直通 FUSE、设 shared，然后常驻。
fn fuse_host_child_main(ready_sock: libc::c_int) -> bool {
    // 1. 进入私有 mount namespace；unshare 失败则放弃宿主会话。
    host_stage("before_unshare");
    // SAFETY: unshare(CLONE_NEWNS) 创建私有挂载命名空间，不影响父进程或其他线程。
    // 允许英文：unshare 和 CLONE_NEWNS 是 Linux 系统调用和标志常量。
    if unsafe { libc::unshare(libc::CLONE_NEWNS) } != 0 {
        log_errno("fuse host unshare failed");
        host_stage("unshare_failed");
        return false;
    }
    host_stage("unshared");
    // 2. 隔离整棵挂载树，阻断向其它 namespace 传播。
    let Ok(root) = CString::new("/") else {
        host_stage("root_path_invalid");
        return false;
    };
    // SAFETY: root 是合法 NUL 结尾路径，且在调用期间保持存活。
    if unsafe {
        libc::mount(
            std::ptr::null(),
            root.as_ptr(),
            std::ptr::null(),
            (libc::MS_REC | libc::MS_PRIVATE) as libc::c_ulong,
            std::ptr::null(),
        )
    } != 0
    {
        log_errno("fuse host MS_PRIVATE failed");
        host_stage("private_failed");
        return false;
    }
    host_stage("private_ok");

    // 3. 挂直通宿主 FUSE（mount_host_fuse 内部完成 shared propagation 并常驻）。
    crate::fuse_redirect::config::mount_host_fuse(
        passthrough_host_config(),
        FUSE_HOST_MOUNT_POINT,
        Some(ready_sock),
    )
}

/// 构造宿主会话的直通配置：`allowed_real_paths` 覆盖整个存储根，所有路径都落回真实后端，
/// 不做任何重定向。宿主会话在应用接入前不对任何 uid 生效，接入后按 uid 查表。
fn passthrough_host_config() -> crate::fuse_redirect::FuseRedirectConfig {
    let uid = 0;
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    crate::fuse_redirect::FuseRedirectConfig {
        package_name: "srx_fuse_host".to_string(),
        app_pid: std::process::id() as i32,
        app_start_time_ticks: crate::platform::process_start_time_ticks(std::process::id() as i32),
        uid,
        app_data_dir: String::new(),
        redirect_target: storage_root.clone(),
        // 策略内部仍以共享存储根作为虚拟 FUSE 根；实际宿主挂载点由 mount_host_fuse
        // 单独传入模块私有目录，避免把宿主目录误当成存储别名参与路径决策。
        mount_root: Some(storage_root.clone()),
        real_root_override: None,
        is_file_monitor_enabled: false,
        allowed_real_paths: vec![storage_root],
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
        is_mapping_mode_only: false,
        // 宿主会话是直通会话：`redirect_target` 取存储根本身，策略侧据此跳过重定向根推导。
        is_passthrough_host: true,
    }
}

/// 阻塞等待宿主子进程的 ready 结果，带超时。
///
/// 返回 `Some(0)` 表示宿主已就绪；`Some(负值)` 是子进程报告的具体失败阶段；
/// `None` 表示超时或对端未按约定回包（子进程在回包前就退出时会走到这里）。
fn recv_host_ready(sock: libc::c_int) -> Option<i32> {
    let timeout = libc::timeval {
        tv_sec: HOST_READY_TIMEOUT_SEC,
        tv_usec: 0,
    };
    // SAFETY: timeout 是合法 timeval，sock 是有效的 socket fd。
    unsafe {
        libc::setsockopt(
            sock,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &timeout as *const libc::timeval as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
    let mut result: i32 = -1;
    // SAFETY: result 是本地可写 i32，recv 填充最多 4 字节。
    let n = unsafe {
        libc::recv(
            sock,
            &mut result as *mut i32 as *mut libc::c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    if n == std::mem::size_of::<i32>() as isize {
        Some(result)
    } else {
        // n=0 表示对端已关闭（子进程在回包前就退出）；n<0 才是等待超时或 socket 错误。
        // 这两种情况的修复方向完全不同，必须区分开。
        let errno = crate::platform::errno::last();
        log::warn!(
            "fuse host ready recv anomaly n={} errno={} {}",
            n,
            errno,
            crate::platform::errno::text(errno)
        );
        None
    }
}

/// 设置全局宿主会话句柄（daemon 启动时调用一次）。
pub fn set_global(host: FuseHost) {
    if let Ok(mut slot) = host_slot().write() {
        // 以当前宿主句柄替换旧快照，供两条挂载路径读取。
        *slot = Some(Arc::new(host));
    }
}

pub fn clear_if_dead() -> bool {
    host_slot()
        .write()
        .ok()
        .map(|mut s| {
            // 仅当槽位里的宿主句柄确实失效时才清除，避免误伤正在使用的会话。
            let dead = s.as_ref().is_some_and(|host| host_is_dead(host));
            if dead {
                *s = None;
            }
            dead
        })
        .unwrap_or(false)
}

/// 获取当前仍存活的宿主会话句柄。
pub fn get_fuse_host() -> Option<Arc<FuseHost>> {
    let slot = host_slot().read().ok()?;
    let host = slot.as_ref()?.clone();
    if crate::platform::is_process_instance_alive(host.child_pid, host.child_start_time_ticks) {
        Some(host)
    } else {
        None
    }
}

/// 确保共享宿主存在；宿主死亡时最多由当前 reconcile 调用方重建一次。
pub fn ensure_global() -> bool {
    if get_fuse_host().is_some() {
        return true;
    }
    let _ = clear_if_dead();
    let now = crate::platform::paths::monotonic_ms();
    let last = LAST_RECOVERY_ATTEMPT_MS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < HOST_RECOVERY_BACKOFF_MS as i64 {
        return false;
    }
    LAST_RECOVERY_ATTEMPT_MS.store(now, Ordering::Relaxed);
    let Some(host) = spawn_fuse_host() else {
        log::warn!(
            "fuse host recovery failed, scoped path remains active backoff_ms={}",
            HOST_RECOVERY_BACKOFF_MS
        );
        return false;
    };
    let pid = host.child_pid;
    let source = host.mount_source.clone();
    set_global(host);
    log::info!("fuse host recovered child={} source={}", pid, source);
    true
}
