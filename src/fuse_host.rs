//! 共享宿主 FUSE 会话（阶段 2 基础设施，B2-a/B2-b）。
//!
//! 目标是把「每应用各自 fork 一个 FUSE 服务」改成「daemon 持有一个持久共享会话，各应用
//! namespace 用 `MS_BIND` 引用」。B2-a 搭起宿主会话骨架，B2-b 实现应用侧接入逻辑，
//! 阶段 1 的按 uid 策略注册通过会话内的控制通道补齐。

use crate::platform::paths;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// 宿主挂载点：放在模块私有目录下，不落在任何存储别名上，避免与应用命名空间发生传播耦合。
const FUSE_HOST_MOUNT_POINT: &str = "/data/adb/modules/storage.redirect.x/tmp/fuse_host";

/// 宿主会话的就绪等待上限（秒）。
const HOST_READY_TIMEOUT_SEC: i64 = 30;
/// 应用接入的就绪等待上限（秒）。
///
/// 接入子进程只做几次本地系统调用（取句柄、mkdir、bind、复核），正常在毫秒级完成；
/// 这里给的时间只用于兜住内核里卡住的 `mount`，避免挂载 worker 被长期阻塞。
const HOST_ATTACH_TIMEOUT_SEC: i64 = 10;
/// 宿主恢复失败后的最短重试间隔，避免每个 reconcile 周期重复 fork。
const HOST_RECOVERY_BACKOFF_MS: u64 = 5_000;
/// 单条策略载荷上限（一次 `send` 不超过它）。
const HOST_CONTROL_PAYLOAD_LIMIT: usize = 32 * 1024;
/// 控制线程栈大小：要容纳与载荷等长的栈缓冲，避免在 fork 之后为读缓冲分配堆内存。
const HOST_CONTROL_STACK_SIZE: usize = 256 * 1024;
/// 策略登记应答的等待上限（毫秒）。
///
/// 登记是接入的前置条件，必须先确认宿主会话真的建好了该 uid 的策略，否则应用会拿到
/// 「未登记即拒绝」的空视图。同一条控制通道被所有应用的挂载 worker 共用，应答因此带
/// uid 标记，读到别人的应答就继续等自己那条。
const HOST_POLICY_ACK_TIMEOUT_MS: i64 = 2_000;
/// 一次登记最多容忍几条不属于自己的应答，避免通道异常时无限空转。
const HOST_POLICY_ACK_SKIP_LIMIT: usize = 8;
/// 应用接入共享宿主会话的**关闭开关**环境变量。
///
/// 共享宿主会话是目标形态，因此**默认开启**：未设置即接入。显式设置成 `0` / `false` /
/// `no` / `off`（大小写不敏感）才关闭，用于在设备上快速退回"每应用一个 scoped 会话"的
/// 旧数据面，不必降级模块。
///
/// 注意它只影响**读到该变量的进程**：daemon 与其 fork 出的挂载 worker 能读到，而
/// `companion_mount` 跑在应用进程内、读不到 daemon 的环境变量。因此它只适合做 daemon 侧
/// 的临时开关；要做全链路开关，需把状态经能力快照之类的共享文件发布出去。
const HOST_ATTACH_ENV: &str = "SRT_FUSE_HOST_ATTACH";

static LAST_RECOVERY_ATTEMPT_MS: AtomicI64 = AtomicI64::new(0);
/// 宿主会话控制端 fd（daemon 侧）；`-1` 表示当前没有可用宿主会话。
static HOST_CONTROL_FD: AtomicI32 = AtomicI32::new(-1);

/// 可替换的全局共享宿主会话句柄。
///
/// 共享宿主子进程可能独立退出；使用可写槽位允许 daemon 在下一轮 reconcile 中
/// 丢弃失效句柄并重建宿主，应用侧读取时仍只拿到当前有效快照。
static FUSE_HOST: OnceLock<RwLock<Option<Arc<FuseHost>>>> = OnceLock::new();

fn host_slot() -> &'static RwLock<Option<Arc<FuseHost>>> {
    FUSE_HOST.get_or_init(|| RwLock::new(None))
}

/// 宿主会话是否仍然可用。
///
/// 除了「pid + 启动时刻」匹配，还必须排除僵尸态：会话线程结束后子进程会退出，若父进程未回收，
/// 它的 `/proc/<pid>` 依旧存在，仅按 pid 判活会把它当成存活会话，自愈便永远不会重建宿主。
fn host_is_alive(host: &FuseHost) -> bool {
    if !crate::platform::is_process_instance_alive(host.child_pid, host.child_start_time_ticks) {
        return false;
    }
    if !process_is_zombie(host.child_pid) {
        return true;
    }
    log::warn!(
        "fuse host session exited on its own child={} source={}",
        host.child_pid,
        host.mount_source
    );
    reap_exited_host_child(host.child_pid);
    false
}

/// 判断进程是否已变成僵尸（已退出但未被回收）。
///
/// `/proc/<pid>/stat` 的状态字段固定紧跟进程名之后；僵尸为 `Z`。
fn process_is_zombie(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid)) else {
        return false;
    };
    // 进程名可能包含空格与括号，状态字段固定在第 2 个字段之后，因此从最后一个 ')' 起解析。
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return false;
    };
    rest.trim_start().starts_with('Z')
}

/// 回收已自行退出的宿主子进程，不留僵尸占位。
fn reap_exited_host_child(pid: libc::pid_t) {
    let mut status: libc::c_int = 0;
    // SAFETY: pid 是本模块 fork 出的子进程；WNOHANG 只在它已退出时回收，不会阻塞。
    unsafe { libc::waitpid(pid, &mut status as *mut libc::c_int, libc::WNOHANG) };
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

/// 保存宿主会话控制端 fd，并关闭被替换掉的旧 fd（宿主重建时避免 fd 泄漏）。
fn set_host_control_fd(fd: libc::c_int) {
    // 登记要等待应答，因此接收超时必须先设好：挂载 worker 是 fork 出来的，超时设置会随
    // fd 一起继承，等真正登记时再设置就晚了。
    set_socket_recv_timeout_ms(fd, HOST_POLICY_ACK_TIMEOUT_MS);
    let previous = HOST_CONTROL_FD.swap(fd, Ordering::Relaxed);
    if previous >= 0 && previous != fd {
        // SAFETY: previous 是本模块此前保存的控制端 fd，本次替换负责关闭它。
        unsafe { libc::close(previous) };
    }
}

/// 设置 socket 接收超时，避免等待应答时无限阻塞。
fn set_socket_recv_timeout_ms(fd: libc::c_int, timeout_ms: i64) {
    let timeout = libc::timeval {
        tv_sec: timeout_ms / 1000,
        tv_usec: ((timeout_ms % 1000) * 1000) as libc::suseconds_t,
    };
    // SAFETY: timeout 是合法 timeval，fd 是本模块持有的 socket。
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &timeout as *const libc::timeval as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
}

/// 丢弃控制端 fd。宿主会话失效或重建失败时调用，避免后续注册请求写到已死的会话上。
fn close_host_control_fd() {
    let previous = HOST_CONTROL_FD.swap(-1, Ordering::Relaxed);
    if previous >= 0 {
        // SAFETY: previous 是本模块保存的控制端 fd，本次清理负责关闭它。
        unsafe { libc::close(previous) };
    }
}

/// 策略登记应答：`(uid, 表大小)`，表大小为 0 表示宿主会话拒绝了这份策略。
///
/// 应答必须带 uid：控制通道被所有应用的挂载 worker 共用（worker 是 daemon 的 fork，
/// 继承同一个 fd），并发登记时先到的应答未必属于本次调用。只按「收到一包就当成功」
/// 判断，会让某个应用在策略尚未登记的情况下接入共享会话。
fn decode_policy_ack(buffer: &[u8]) -> Option<(i32, i32)> {
    if buffer.len() < 8 {
        return None;
    }
    let uid = i32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
    let size = i32::from_ne_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
    Some((uid, size))
}

/// 等待宿主会话对本次登记的应答。
fn await_policy_ack(fd: libc::c_int, uid: i32) -> bool {
    let mut buffer = [0u8; 8];
    for _ in 0..HOST_POLICY_ACK_SKIP_LIMIT {
        // SAFETY: fd 是本模块持有的控制端 socket，buffer 是本地可写数组。
        let read = unsafe {
            libc::recv(
                fd,
                buffer.as_mut_ptr() as *mut libc::c_void,
                buffer.len(),
                0,
            )
        };
        if read < 0 {
            let errno = crate::platform::errno::last();
            log::warn!(
                "fuse host policy ack unavailable uid={} errno={} {}",
                uid,
                errno,
                crate::platform::errno::text(errno)
            );
            return false;
        }
        let Some((ack_uid, table_size)) = decode_policy_ack(&buffer[..read as usize]) else {
            continue;
        };
        if ack_uid != uid {
            // 属于并发登记的另一次调用：不能代它消费，继续等自己那条。
            log::debug!(
                "fuse host policy ack for other uid={} expected={}",
                ack_uid,
                uid
            );
            continue;
        }
        if table_size <= 0 {
            log::warn!("fuse host policy ack rejected uid={}", uid);
            return false;
        }
        return true;
    }
    log::warn!("fuse host policy ack not received uid={}", uid);
    false
}

/// 把某个应用的策略登记到共享宿主会话（按其 uid 生效）。
///
/// 这是阶段 1「按 uid 策略注册」的 daemon 侧入口：策略在 daemon 侧构造（与 scoped 会话同一份
/// `fuse_config_from_request`），序列化后经控制通道送到持有会话的子进程再建策略。
///
/// 返回 `true` 表示**宿主会话已确认建好该 uid 的策略**（收到应答），而不是"数据已发出"：
/// 接入会让应用的存储根直接由宿主会话承载，策略没登记上去就是整片 ENOENT，因此这里必须
/// 拿应答而不是拿 `send` 的返回值。返回 `false` 时调用方保持 scoped 路径不变。
pub fn register_app_policy(config: &crate::fuse_redirect::FuseRedirectConfig) -> bool {
    let fd = HOST_CONTROL_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return false;
    }
    let Ok(payload) = serde_json::to_vec(config) else {
        log::warn!("fuse host policy encode failed pkg={}", config.package_name);
        return false;
    };
    if payload.len() > HOST_CONTROL_PAYLOAD_LIMIT {
        log::warn!(
            "fuse host policy payload too large pkg={} bytes={} limit={}",
            config.package_name,
            payload.len(),
            HOST_CONTROL_PAYLOAD_LIMIT
        );
        return false;
    }
    // SAFETY: fd 是本模块保存的控制端 socket，payload 在调用期间保持存活。
    let sent = unsafe {
        libc::send(
            fd,
            payload.as_ptr() as *const libc::c_void,
            payload.len(),
            0,
        )
    };
    if sent != payload.len() as isize {
        let errno = crate::platform::errno::last();
        log::warn!(
            "fuse host policy send failed uid={} sent={} bytes={} errno={} {}",
            config.uid,
            sent,
            payload.len(),
            errno,
            crate::platform::errno::text(errno)
        );
        return false;
    }
    await_policy_ack(fd, config.uid)
}

/// 在宿主会话内启动按 uid 策略控制通道的读取循环。
///
/// 线程随会话存续：控制端被关闭（或会话结束）时 `recv` 返回 0，循环退出。读缓冲放在线程栈上，
/// 不在 fork 之后分配堆内存。
pub(crate) fn spawn_host_control_loop(
    control_sock: libc::c_int,
    policy_table: crate::fuse_redirect::SharedPolicyTable,
) {
    let spawned = std::thread::Builder::new()
        .name("srx_host_policy".to_string())
        .stack_size(HOST_CONTROL_STACK_SIZE)
        .spawn(move || {
            let mut buffer = [0u8; HOST_CONTROL_PAYLOAD_LIMIT];
            loop {
                // SAFETY: control_sock 是本次会话的控制端 fd，buffer 是本地可写数组。
                let read = unsafe {
                    libc::recv(
                        control_sock,
                        buffer.as_mut_ptr() as *mut libc::c_void,
                        buffer.len(),
                        0,
                    )
                };
                if read <= 0 {
                    break;
                }
                let payload = &buffer[..read as usize];
                match serde_json::from_slice::<crate::fuse_redirect::FuseRedirectConfig>(payload) {
                    Ok(config) => {
                        let uid = config.uid;
                        let package_name = config.package_name.clone();
                        let size = policy_table.register(config).unwrap_or(0);
                        if size > 0 {
                            log::info!(
                                "fuse host policy registered uid={} pkg={} table={}",
                                uid,
                                package_name,
                                size
                            );
                        } else {
                            log::warn!(
                                "fuse host policy rejected uid={} pkg={}",
                                uid,
                                package_name
                            );
                        }
                        // 应答必须发出去，否则调用方会一直等到超时再退回 scoped 路径；
                        // 表大小为 0 也是一种明确答复（拒绝），不能省略。
                        let mut ack = [0u8; 8];
                        ack[..4].copy_from_slice(&uid.to_ne_bytes());
                        ack[4..].copy_from_slice(&(size as i32).to_ne_bytes());
                        // SAFETY: control_sock 是本次会话的控制端 fd，ack 是本地数组。
                        let sent = unsafe {
                            libc::send(
                                control_sock,
                                ack.as_ptr() as *const libc::c_void,
                                ack.len(),
                                0,
                            )
                        };
                        if sent != ack.len() as isize {
                            let errno = crate::platform::errno::last();
                            log::warn!(
                                "fuse host policy ack send failed uid={} errno={} {}",
                                uid,
                                errno,
                                crate::platform::errno::text(errno)
                            );
                        }
                    }
                    Err(error) => log::warn!("fuse host policy decode failed err={}", error),
                }
            }
            log::info!("fuse host policy channel closed");
        });
    if spawned.is_err() {
        log::warn!("fuse host policy channel thread unavailable");
        // SAFETY: 线程创建失败时由本函数关闭控制端 fd，避免泄漏。
        unsafe { libc::close(control_sock) };
    }
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

/// 应用接入共享宿主会话是否生效（默认开启，只认显式关闭）。
fn host_attach_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        // 只有明确写成"关闭"的取值才关闭；未设置、空值或其它内容一律按开启处理——
        // 开关的默认方向必须与目标形态一致，否则共享宿主会话永远不会在真实环境里跑起来。
        let configured = std::env::var(HOST_ATTACH_ENV).ok();
        let disabled = configured
            .as_deref()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(false);
        log::info!(
            "fuse host attach switch env={} configured={:?} disabled={} default_on=true",
            HOST_ATTACH_ENV,
            configured,
            disabled
        );
        !disabled
    })
}

/// 共享宿主会话当前是否可以安全承接指定调用方的挂载。
///
/// 这是能力闸门加语义约束，两层都必须满足：
///
/// 1. **能力**：宿主会话按 uid 提供策略，登记通道与拒绝回退都已就位，接入**默认开启**；
///    [`HOST_ATTACH_ENV`] 只用于显式关闭（kill switch），便于在设备上快速退回旧数据面。
/// 2. **语义**：宿主会话的虚拟根固定是"该 uid 的整个存储视图根"，因为策略是按 uid 注册的
///    （一个 uid 一份规则，虚拟根只能是整根）。所以宿主挂载只能落在存储视图根上；落在更深
///    的 scoped 根（混合规则的子目录）时，内核会把该子目录下的请求按整根解析，命中的是与本
///    应用规则无关的真实路径——读错内容却完全不报错。这类目标一律拒绝接入，继续走 scoped。
///
/// 两条都不满足时返回 false 都不是失败：调用方保持既有的 scoped FUSE 路径。
pub fn can_attach_app(uid: i32, mount_root: &str) -> bool {
    if !host_attach_enabled() {
        return false;
    }
    if mount_root.is_empty() {
        return false;
    }
    let user_id = crate::platform::user_id_from_uid(uid);
    let view_root = paths::storage_user_root_for_user(user_id);
    paths::normalize_syntax(mount_root) == paths::normalize_syntax(&view_root)
}

/// 某个挂载源是否属于**当前仍在服务**的共享宿主会话。
///
/// 宿主会话会被自愈逻辑重建（新的 pid、新的挂载源）。旧会话留下的挂载层虽然仍带着
/// `srx_fuse_host[...]` 前缀、看上去"是本模块的"，但它服务的那条 FUSE 连接已经断了，
/// 继续当作有效层保留只会让应用访问永远 ENOTCONN。
pub fn is_current_host_source(source: &str) -> bool {
    get_fuse_host().is_some_and(|host| host.mount_source == source)
}

/// 应用接入留下的挂载层是否已经过期。
///
/// "过期"指这层来自共享宿主会话、但不再是当前仍在服务的那个会话：它既是本模块的层
/// （归属判定会认），又已经失去后端。清理与恢复流程据此决定不保留、按重挂处理。
pub fn is_stale_host_source(source: &str) -> bool {
    crate::fuse_redirect::config::is_host_mount_source(source) && !is_current_host_source(source)
}

/// 一次成功的应用接入。
pub struct HostAttach {
    /// 已经落在应用命名空间里的挂载点。
    pub target: String,
    /// 承载该挂载的宿主会话身份。
    ///
    /// 调用方记录进挂载状态时必须与 scoped 子进程区分：宿主会话是跨应用共享的，
    /// 按 pid 终止它会把其它应用一起打掉。
    pub host_pid: i32,
    pub host_start_time_ticks: u64,
}

/// 把应用的存储视图接入共享宿主会话。
///
/// 要点有两个，都是"看起来能跑、实际不生效"的类型：
///
/// 1. 挂载必须**出现在应用自己的 mount namespace 里**。宿主会话建立时对整棵树做过
///    `MS_REC|MS_PRIVATE`，应用命名空间看不到宿主挂载点；直接在宿主命名空间里挂载只会
///    落在宿主自己的树上，应用视图毫无变化。
/// 2. 搬运必须用 `open_tree(OPEN_TREE_CLONE)` + `move_mount`，不能指望 `mount(MS_BIND)`：
///    后者的源挂载必须属于调用方当前命名空间，跨命名空间时直接 `EINVAL`（真机实测）。
///
/// 因此子进程的顺序是：进宿主命名空间克隆游离挂载 → 切回应用命名空间附着 → 复核应用视图里
/// 该目标的挂载源就是本次会话的挂载源。复核不是可选项：这类错误不会有任何其它症状。
///
/// 子进程在附着成功后立即退出：挂载归 mount namespace 所有，不随创建它的进程消失，
/// 而常驻只会白占一个进程并拖住应用命名空间的引用计数（应用退出后残留挂载）。
pub fn attach_app_to_host(host: &FuseHost, target_root: &str) -> Option<HostAttach> {
    if target_root.is_empty() {
        return None;
    }
    let mut ready_sockets = [0; 2];
    // SAFETY: ready_sockets 是本地数组，长度合法，socketpair 填充两个 fd。
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_DGRAM,
            0,
            ready_sockets.as_mut_ptr(),
        )
    } != 0
    {
        log_errno("fuse host attach socketpair failed");
        return None;
    }

    crate::logging::prepare_for_fork();
    // SAFETY: fork 前已完成日志准备；子进程只做系统调用后退出。
    let child = unsafe { libc::fork() };
    if child < 0 {
        log_errno("fuse host attach fork failed");
        // SAFETY: 两个 fd 均为本次 socketpair 打开，此处是唯一清理点。
        unsafe {
            libc::close(ready_sockets[0]);
            libc::close(ready_sockets[1]);
        }
        return None;
    }

    if child == 0 {
        // SAFETY: 子进程关闭父端 fd；该入口只在完成或失败时返回。
        unsafe { libc::close(ready_sockets[0]) };
        let name = b"srx_hostbind\0";
        // SAFETY: prctl(PR_SET_NAME) 设置线程名称，name 是 NUL 结尾的静态字节串。
        unsafe {
            libc::prctl(libc::PR_SET_NAME, name.as_ptr() as libc::c_ulong, 0, 0, 0);
        }
        let ok = host_attach_child_main(host, target_root, ready_sockets[1]);
        host_stage(if ok { "attach_ok" } else { "attach_failed" });
        // SAFETY: _exit 终止子进程，不跑 atexit。
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // SAFETY: 父进程关闭子端 fd 后等待就绪。
    unsafe { libc::close(ready_sockets[1]) };
    let ready = recv_host_ready(ready_sockets[0], HOST_ATTACH_TIMEOUT_SEC);
    // SAFETY: ready_sockets[0] 是父进程持有的有效端点。
    unsafe { libc::close(ready_sockets[0]) };
    // 子进程正常会立刻退出，因此这里以阻塞回收为准；异常路径改用强制回收，两者都不能留下僵尸。
    let ok = match ready {
        Some(0) => {
            reap_attach_child(child);
            true
        }
        Some(code) => {
            let reason = reap_host_child(child);
            log::warn!(
                "fuse host attach not ready child={} code={} stage={} {}",
                child,
                code,
                crate::fuse_redirect::config::host_ready_stage(code),
                reason
            );
            false
        }
        None => {
            let reason = reap_host_child(child);
            log::warn!(
                "fuse host attach unavailable child={} timeout_sec={} {}",
                child,
                HOST_ATTACH_TIMEOUT_SEC,
                reason
            );
            false
        }
    };
    if !ok {
        log_host_stage_trace();
        return None;
    }
    Some(HostAttach {
        target: paths::normalize_syntax(target_root),
        host_pid: host.child_pid,
        host_start_time_ticks: host.child_start_time_ticks,
    })
}

/// 回收已自行退出的接入子进程。
fn reap_attach_child(pid: libc::pid_t) {
    let mut status: libc::c_int = 0;
    for _ in 0..50 {
        // SAFETY: pid 是本次 fork 出的子进程，已确认就绪，回收不会阻塞。
        if unsafe { libc::waitpid(pid, &mut status as *mut libc::c_int, libc::WNOHANG) } == pid {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    reap_host_child(pid);
}

/// `open_tree(2)` 的 `OPEN_TREE_CLONE`：克隆出一个游离（未附着到任何命名空间）的挂载。
const OPEN_TREE_CLONE: libc::c_uint = 0x0000_0001;
/// `move_mount(2)` 的 `MOVE_MOUNT_F_EMPTY_PATH`：源由 fd 指定（游离挂载没有路径）。
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 0x0000_0004;

/// 接入子进程入口：宿主命名空间克隆游离挂载 → 应用命名空间附着 → 复核。
///
/// **不能用 `mount(MS_BIND)` 跨命名空间绑定**。`mount(2)` 的 bind 要求源挂载属于**调用方当前的
/// mount namespace**（内核 `do_loopback`/`check_mnt`），把源换成 `/proc/self/fd/<n>` 也绕不过去
/// ——真机实测直接返回 `EINVAL`。跨命名空间搬运挂载是 `open_tree(OPEN_TREE_CLONE)` +
/// `move_mount` 这对 API 的用途：前者在**源命名空间**里克隆出一个不附着于任何命名空间的挂载
/// （fd 携带），后者在**目标命名空间**里把它附着到目标路径。克隆与原挂载共享同一个 superblock，
/// 因此 FUSE 请求仍然全部回到同一个宿主会话。
fn host_attach_child_main(host: &FuseHost, target_root: &str, ready_sock: libc::c_int) -> bool {
    // 1. 先钉住当前（应用）命名空间：克隆要在宿主命名空间做，句柄是唯一的回头路。
    let Ok(c_app_ns) = CString::new("/proc/self/ns/mnt") else {
        return false;
    };
    // SAFETY: c_app_ns 是 NUL 结尾的合法路径。
    let app_ns = unsafe { libc::open(c_app_ns.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if app_ns < 0 {
        log_errno("fuse host attach app ns open failed");
        return false;
    }
    // SAFETY: app_ns 是本函数打开的有效 fd，只在切回时使用。
    let app_ns = UniqueFd::new(app_ns);

    // 2. 进入宿主命名空间做克隆。
    let host_ns_path = format!("/proc/{}/ns/mnt", host.child_pid);
    let Ok(c_host_ns) = CString::new(host_ns_path) else {
        return false;
    };
    // SAFETY: c_host_ns 是 NUL 结尾的合法路径。
    let host_ns = unsafe { libc::open(c_host_ns.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if host_ns < 0 {
        log_errno("fuse host attach host ns open failed");
        return false;
    }
    // SAFETY: host_ns 是本函数打开的有效 namespace fd。
    let host_ns = UniqueFd::new(host_ns);
    // SAFETY: host_ns 是有效的 mount namespace fd。
    if unsafe { libc::setns(host_ns.get(), libc::CLONE_NEWNS) } != 0 {
        log_errno("fuse host attach setns host failed");
        return false;
    }

    let Ok(c_mount_point) = CString::new(host.mount_point.as_str()) else {
        return false;
    };
    let tree_fd = open_detached_mount(&c_mount_point);

    // 3. 无论克隆成败都必须切回应用命名空间：后续 mkdir 与附着都必须在应用视图里发生。
    // SAFETY: app_ns 是本子进程进入宿主命名空间前钉住的自身命名空间 fd。
    if unsafe { libc::setns(app_ns.get(), libc::CLONE_NEWNS) } != 0 {
        log_errno("fuse host attach setns app failed");
        return false;
    }
    if tree_fd < 0 {
        let errno = crate::platform::errno::last();
        log::warn!(
            "fuse host attach open_tree failed mp={} errno={} {}",
            host.mount_point,
            errno,
            crate::platform::errno::text(errno)
        );
        return false;
    }
    // SAFETY: tree_fd 是 open_tree 返回的有效 fd。
    let tree_fd = UniqueFd::new(tree_fd);

    // 4. 目标目录：与应用挂载路径同源，允许已存在。
    let Ok(c_target) = CString::new(target_root) else {
        return false;
    };
    // SAFETY: c_target 是 NUL 结尾的合法路径；失败只可能是已存在（后续附着会给出结论）。
    unsafe {
        libc::mkdir(c_target.as_ptr(), 0o755);
    }

    // 5. 把游离挂载附着到应用视图里的目标路径，并立刻切断传播关系。
    if !move_detached_mount(tree_fd.get(), &c_target) {
        return false;
    }
    if !make_mount_private(&c_target) {
        return false;
    }

    // 6. 复核。挂载源必须就是本次会话的挂载源：只靠系统调用返回 0 无法区分
    //    "挂在应用视图里" 与 "挂在了别处"，而误判成接入成功会让应用静默失去重定向。
    let Some(live) = crate::mount_ledger::topmost_live_mount(0, target_root) else {
        log::warn!("fuse host attach not visible target={}", target_root);
        return false;
    };
    if live.source != host.mount_source {
        log::warn!(
            "fuse host attach source mismatch target={} expected={} actual={} fs={}",
            target_root,
            host.mount_source,
            live.source,
            live.fs_type
        );
        return false;
    }

    // 7. 通知父进程，随后自行退出。
    let ready: i32 = 0;
    // SAFETY: ready_sock 是本次接入的有效端点，ready 是栈变量。
    let sent = unsafe {
        libc::send(
            ready_sock,
            &ready as *const i32 as *const libc::c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    if sent != std::mem::size_of::<i32>() as isize {
        log_errno("fuse host attach send ready failed");
        return false;
    }
    true
}

/// 在源命名空间里克隆出一个游离挂载，返回其 fd（失败返回负值）。
fn open_detached_mount(path: &CStr) -> libc::c_int {
    // SAFETY: open_tree 的参数与内核一致：dfd 为 AT_FDCWD，path 是 NUL 结尾路径，
    // 只克隆、不附着，因此不会改动任何命名空间。
    unsafe {
        libc::syscall(
            libc::SYS_open_tree,
            libc::AT_FDCWD,
            path.as_ptr(),
            OPEN_TREE_CLONE,
        ) as libc::c_int
    }
}

/// 把**本命名空间内**的挂载点改为私有，切断与其它命名空间的传播关系。
///
/// 宿主挂载点在宿主会话里开了 shared，克隆出来的这份会带着同一个 peer group（`mountinfo`
/// 里的 `shared:N`）。保持共享会很危险：peer group 的成员之间会互相传播挂载/卸载事件，
/// 于是任一应用在存储视图下新建或摘除挂载都会传播到其它应用和宿主命名空间——既跨应用干扰，
/// 也与"卸载一个应用的注入不影响其它应用"的设计前提直接冲突。scoped 会话的挂载本来就是
/// private，这里改私有后两条路径语义一致。
fn make_mount_private(target: &CStr) -> bool {
    // SAFETY: 改传播属性只需要目标路径，source/fs_type/data 均为 null；只动传播属性，
    // 不改动任何文件内容，也不会摘除挂载。
    let result = unsafe {
        libc::mount(
            std::ptr::null(),
            target.as_ptr(),
            std::ptr::null(),
            (libc::MS_REC | libc::MS_PRIVATE) as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if result != 0 {
        log_errno("fuse host attach make private failed");
        return false;
    }
    true
}

/// 把游离挂载附着到目标命名空间的路径上。
fn move_detached_mount(tree_fd: libc::c_int, target: &CStr) -> bool {
    // 空字符串是 `MOVE_MOUNT_F_EMPTY_PATH` 要求的占位：源由 fd 而非路径给出。
    let empty = c"";
    // SAFETY: tree_fd 是 open_tree 返回的 fd，target 是 NUL 结尾路径且在本调用期间保持存活。
    let result = unsafe {
        libc::syscall(
            libc::SYS_move_mount,
            tree_fd,
            empty.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            MOVE_MOUNT_F_EMPTY_PATH,
        )
    };
    if result != 0 {
        log_errno("fuse host attach move_mount failed");
        return false;
    }
    true
}

/// 只负责关闭 fd 的小包装：命名空间与挂载点句柄在多条失败分支上都要关闭。
struct UniqueFd(libc::c_int);

impl UniqueFd {
    fn new(fd: libc::c_int) -> Self {
        Self(fd)
    }

    fn get(&self) -> libc::c_int {
        self.0
    }
}

impl Drop for UniqueFd {
    fn drop(&mut self) {
        // SAFETY: fd 由本包装独占持有，Drop 是唯一关闭点。
        unsafe { libc::close(self.0) };
    }
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

    // 控制通道：daemon 侧把应用策略按 uid 送进会话；与就绪通道分开，避免和 ready 回包互相干扰。
    let mut control_sockets = [0; 2];
    // SAFETY: control_sockets 是本地数组，长度合法，socketpair 填充两个 fd。
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_DGRAM,
            0,
            control_sockets.as_mut_ptr(),
        )
    } != 0
    {
        log_errno("fuse host control socketpair failed");
        // SAFETY: 就绪通道的两个 fd 均为本次打开，此处是唯一清理点。
        unsafe {
            libc::close(ready_sockets[0]);
            libc::close(ready_sockets[1]);
        }
        return None;
    }

    reset_host_stage();
    crate::logging::prepare_for_fork();
    // SAFETY: fork 前已完成日志/信号准备，匹配现有 daemon 挂载的 fork 模式。
    // 允许英文：fork 是系统调用名称。
    let child = unsafe { libc::fork() };
    if child < 0 {
        log_errno("宿主会话 fork 失败");
        // SAFETY: 四个 fd 均为本次 socketpair 打开，此处是唯一清理点。
        unsafe {
            libc::close(ready_sockets[0]);
            libc::close(ready_sockets[1]);
            libc::close(control_sockets[0]);
            libc::close(control_sockets[1]);
        }
        return None;
    }

    if child == 0 {
        host_stage("child_enter");
        // SAFETY: 子进程关闭父端两个 fd（就绪端与控制端）；该入口只在完成或失败时返回。
        unsafe {
            libc::close(ready_sockets[0]);
            libc::close(control_sockets[0]);
        }
        let name = b"srx_fuse_host\0";
        // SAFETY: prctl(PR_SET_NAME) 设置线程名称，name 是 NUL 结尾的静态字节串。
        // 允许英文：prctl 是系统调用名称。
        unsafe {
            libc::prctl(libc::PR_SET_NAME, name.as_ptr() as libc::c_ulong, 0, 0, 0);
        }
        // 捕获 panic：子进程的 panic 信息会写进 stderr，而 service.sh 已把它丢弃，
        // 只留下"未就绪"这种无法定位的现象。这里改为落到阶段文件，保留可取证信息。
        install_host_panic_hook();
        let ok = fuse_host_child_main(ready_sockets[1], control_sockets[1]);
        host_stage(if ok { "child_ok" } else { "child_failed" });
        // SAFETY: _exit 终止子进程，不跑 atexit；子进程用完即退出，无需清理栈。
        // 允许英文：_exit 是系统调用名称。
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // SAFETY: 父进程关闭两个子端 fd 后等待就绪；两个 fd 均为本次 socketpair 打开。
    unsafe {
        libc::close(ready_sockets[1]);
        libc::close(control_sockets[1]);
    }
    let ready = recv_host_ready(ready_sockets[0], HOST_READY_TIMEOUT_SEC);
    // SAFETY: ready_sockets[0] 是父进程持有的有效 socketpair 端点。
    // 允许英文：close 是系统调用名称。
    unsafe { libc::close(ready_sockets[0]) };
    match ready {
        Some(0) => {}
        Some(code) => {
            let reason = reap_host_child(child);
            // SAFETY: 子进程已回收，控制端不再有对端，由本次清理关闭。
            unsafe { libc::close(control_sockets[0]) };
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
            // SAFETY: 子进程已回收，控制端不再有对端，由本次清理关闭。
            unsafe { libc::close(control_sockets[0]) };
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
        // SAFETY: 子进程已回收，控制端不再有对端，由本次清理关闭。
        unsafe { libc::close(control_sockets[0]) };
        log::warn!(
            "fuse host start time unavailable child={} {}",
            child,
            reason
        );
        return None;
    };
    // 会话可用后才发布控制端：此前任何注册请求都没有真实会话可服务。
    set_host_control_fd(control_sockets[0]);
    Some(FuseHost {
        child_pid: child,
        child_start_time_ticks,
        mount_point: FUSE_HOST_MOUNT_POINT.to_string(),
        mount_source: crate::fuse_redirect::config::host_mount_source(child as u32),
    })
}

/// 子进程入口：进入私有 namespace、挂直通 FUSE、设 shared，然后常驻。
fn fuse_host_child_main(ready_sock: libc::c_int, control_sock: libc::c_int) -> bool {
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
        Some(control_sock),
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

/// 阻塞等待子进程的 ready 结果，带超时。
///
/// 返回 `Some(0)` 表示就绪；`Some(负值)` 是子进程报告的具体失败阶段；
/// `None` 表示超时或对端未按约定回包（子进程在回包前就退出时会走到这里）。
fn recv_host_ready(sock: libc::c_int, timeout_sec: i64) -> Option<i32> {
    let timeout = libc::timeval {
        tv_sec: timeout_sec,
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
    let cleared = host_slot()
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
        .unwrap_or(false);
    if cleared {
        // 会话已死，控制端不再有对端；一并丢弃，避免后续注册请求写到死会话。
        close_host_control_fd();
    }
    cleared
}

/// 获取当前仍存活的宿主会话句柄。
pub fn get_fuse_host() -> Option<Arc<FuseHost>> {
    let slot = host_slot().read().ok()?;
    let host = slot.as_ref()?.clone();
    // 只交出仍然可用的会话：僵尸态由 `host_is_alive` 一并排除并回收。
    if host_is_alive(&host) {
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

/// 等待宿主会话就绪的预算与失败冷却。
///
/// 宿主建立实测在亚秒到两秒级完成；预算 6 秒给足余量。等待发生在 daemon 主循环的
/// 挂载处理里，**必须有界**：无界等待会把整个 reconcile 循环卡死，开机时几十个应用
/// 的挂载全部排队。同理，一次等待超时说明宿主在此环境里建不起来（能力缺失、内核
/// 拒绝），继续让后续请求逐个空等只会放大延迟——30 秒冷却内的请求立即走回退路径，
/// 冷却到期后再允许一次完整等待，宿主恢复后自然接上。
const HOST_WAIT_BUDGET_MS: u64 = 6_000;
const HOST_WAIT_FAIL_COOLDOWN_MS: i64 = 30_000;
const HOST_WAIT_POLL_INTERVAL_MS: i64 = 250;
static LAST_HOST_WAIT_FAIL_MS: AtomicI64 = AtomicI64::new(0);

/// 有界等待共享宿主会话就绪，供 Auto 模式挂载规划在宿主未就绪时调用。
///
/// 返回 `false` 的三种情形调用方处理完全一致（保持按目录 scoped 旧规划）：
/// 接入被显式关闭（立即返回，不等待）；等待预算内宿主仍未建立；距上次等待
/// 超时不足一个冷却周期（避免请求风暴逐个空等）。
///
/// 为什么是"等待"而不是"先按旧规划挂上、事后迁移"：迁移必须在应用命名空间里
/// 先卸旧 scoped 层、再挂新层，中间应用对该路径的访问会**穿透到真实存储**——
/// 这是 fail-open 窗口。而等待发生在挂载应答返回之前，应用进程尚未恢复运行、
/// 不会产生 I/O，一次挂载到位，不存在中间态。
pub fn wait_for_host_session() -> bool {
    if !host_attach_enabled() {
        return false;
    }
    if get_fuse_host().is_some() {
        return true;
    }
    let now = crate::platform::paths::monotonic_ms();
    let last_fail = LAST_HOST_WAIT_FAIL_MS.load(Ordering::Relaxed);
    if last_fail != 0 && now.saturating_sub(last_fail) < HOST_WAIT_FAIL_COOLDOWN_MS {
        return false;
    }
    let deadline = now.saturating_add(HOST_WAIT_BUDGET_MS as i64);
    loop {
        // ensure_global 内部带退避；预算内它通常第一次调用就能把宿主建起来。
        if get_fuse_host().is_some() {
            return true;
        }
        let _ = ensure_global();
        if get_fuse_host().is_some() {
            return true;
        }
        let current = crate::platform::paths::monotonic_ms();
        if current >= deadline {
            LAST_HOST_WAIT_FAIL_MS.store(current, Ordering::Relaxed);
            log::warn!(
                "fuse host wait timeout budget_ms={} fallback=scoped_planning",
                HOST_WAIT_BUDGET_MS
            );
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(
            HOST_WAIT_POLL_INTERVAL_MS as u64,
        ));
    }
}
