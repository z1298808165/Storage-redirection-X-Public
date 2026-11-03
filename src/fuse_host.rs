//! 共享宿主 FUSE 会话（阶段 2 基础设施，B2-a/B2-b）。
//!
//! 目标是把「每应用各自 fork 一个 FUSE 服务」改成「daemon 持有一个持久共享会话，各应用
//! namespace 用 `MS_BIND` 引用」。B2-a 搭起宿主会话骨架，B2-b 实现应用侧接入逻辑，
//! 阶段 1 的按 uid 策略注册通过会话内的控制通道补齐。

// quality-allow(lint-suppression): register_app_policy 只在 daemon 目标被调用，
// lib 目标重导出它会被判为未使用导入，因此保留定向豁免。
#[allow(unused_imports)]
pub use crate::fuse_host_control::register_app_policy;
pub(crate) use crate::fuse_host_control::spawn_host_control_loop;
use crate::fuse_host_control::{close_host_control_fd, set_host_control_fd};
use crate::platform::paths;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// 宿主挂载点：放在模块私有目录下，不落在任何存储别名上，避免与应用命名空间发生传播耦合。
const FUSE_HOST_MOUNT_POINT: &str = crate::platform::module_paths::FUSE_HOST_MOUNT_DIR;

/// 宿主会话的就绪等待上限（秒）。
const HOST_READY_TIMEOUT_SEC: i64 = 30;
/// 应用接入的就绪等待上限（秒）。
///
/// 接入子进程只做几次本地系统调用（取句柄、mkdir、bind、复核），正常在毫秒级完成；
/// 这里给的时间只用于兜住内核里卡住的 `mount`，避免挂载 worker 被长期阻塞。
pub(crate) const HOST_ATTACH_TIMEOUT_SEC: i64 = 10;
/// 宿主恢复失败后的最短重试间隔，避免每个 reconcile 周期重复 fork。
const HOST_RECOVERY_BACKOFF_MS: u64 = 5_000;
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
static HOST_PIDFD: AtomicI32 = AtomicI32::new(-1);
static HOST_GENERATION: AtomicU64 = AtomicU64::new(0);
/// 可替换的全局共享宿主会话句柄。
///
/// 共享宿主子进程可能独立退出；使用可写槽位允许 daemon 在下一轮 reconcile 中
/// 丢弃失效句柄并重建宿主，应用侧读取时仍只拿到当前有效快照。
static FUSE_HOST: OnceLock<RwLock<Option<Arc<FuseHost>>>> = OnceLock::new();

pub(crate) fn host_slot() -> &'static RwLock<Option<Arc<FuseHost>>> {
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
    if process_is_zombie(host.child_pid) {
        log::warn!(
            "fuse host session exited on its own child={} source={}",
            host.child_pid,
            host.mount_source
        );
        reap_exited_host_child(host.child_pid);
        return false;
    }
    true
}

/// 终止连接已死的假活宿主并尽力回收。
///
/// 宿主进程在 FUSE 连接失效后可能长期存活（会话线程未感知连接死亡），只清句柄会让
/// 它带着死挂载常驻，`/proc/<pid>/ns/mnt` 也仍可进入。SIGKILL 后按僵尸回收；进程若
/// 短暂未退出，留给下一轮判活兜底（僵尸态检测已覆盖该形态）。
fn terminate_host_process(pid: libc::pid_t) {
    // SAFETY: pid 是本模块 fork 出的宿主子进程，调用方已凭 pid + 启动时刻确认其身份；
    // SIGKILL 只作用于该进程，不影响其它会话。
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let mut status: libc::c_int = 0;
    for _ in 0..10 {
        // SAFETY: pid 是本模块 fork 出的子进程；WNOHANG 只在它已退出时回收，不会阻塞。
        if unsafe { libc::waitpid(pid, &mut status as *mut libc::c_int, libc::WNOHANG) } == pid {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
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

/// 宿主会话的跨进程视图。
///
/// daemon 持有 [`FuseHost`] 原始句柄；companion（Zygisk companion 进程）与 daemon
/// 没有共享内存，只能通过快照文件发现会话。视图字段与 [`FuseHost`] 的宿主身份字段
/// 一一对应，因此接入逻辑（克隆/附着/复核）对两个进程共用同一份实现。
#[derive(Clone)]
pub struct HostSessionView {
    /// 持有宿主 mount namespace 的子进程 pid。
    pub child_pid: i32,
    /// 子进程启动时刻（clock ticks），用于区分 pid 复用。
    pub child_start_time_ticks: u64,
    /// 宿主挂载点。
    pub mount_point: String,
    /// 宿主会话挂载源（`srx_fuse_host[<pid>]`）。
    pub mount_source: String,
    /// 宿主会话中每个 uid 当前已登记策略的指纹。
    ///
    /// 未登记 uid 的请求会被 fail-closed 拒绝（`deny_all` → ENOENT），因此接入前
    /// 必须确认本应用的 uid 已在列——companion 没有控制通道，登记由 daemon 预先完成。
    pub policy_fingerprints: std::collections::BTreeMap<u32, u64>,
}

impl From<&FuseHost> for HostSessionView {
    fn from(host: &FuseHost) -> Self {
        let policy_fingerprints = registered_policy_snapshot();
        Self {
            child_pid: host.child_pid,
            child_start_time_ticks: host.child_start_time_ticks,
            mount_point: host.mount_point.clone(),
            mount_source: host.mount_source.clone(),
            policy_fingerprints,
        }
    }
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
const FUSE_HOST_STAGE_PATH: &str = crate::platform::module_paths::FUSE_HOST_STAGE_FILE;
/// 阶段轨迹文件的大小上限。轨迹只用于排障，不能随接入风暴无界追加（真机单次 boot 已
/// 268KB）；超过上限时清空后继续写，保留最近阶段而不是整段历史。
const FUSE_HOST_STAGE_MAX_BYTES: i64 = 256 * 1024;

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
    // SAFETY: path 是合法 NUL 结尾路径；buffer 与 len 一致，fd 在同一块内关闭；
    // lseek/ftruncate 只作用于刚打开的本文件 fd。
    unsafe {
        let fd = libc::open(
            path.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
            0o644,
        );
        if fd < 0 {
            return;
        }
        if libc::lseek(fd, 0, libc::SEEK_END) >= FUSE_HOST_STAGE_MAX_BYTES {
            libc::ftruncate(fd, 0);
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
///
/// 这是共享宿主会话的唯一总闸：宿主会话建立、应用接入、媒体视图绑定与外部 abort
/// 兜底监控全部以它为前提。任一缺失都会让「关闭接入」变成假关闭——宿主照样建立、
/// 照样换代，只是没有应用接入它，而换代本身仍会给已附着命名空间留死挂载层。
pub(crate) fn host_attach_enabled() -> bool {
    if !host_attach_env_enabled() {
        return false;
    }
    // 落盘开关每次都读：设备上 daemon 由 service.sh 直接拉起、没有注入环境变量的
    // 位置，现场排障只能靠创建文件。文件与 env 语义完全等价，出现即关闭宿主的全部
    // 使用路径（不只是应用接入）。
    if std::path::Path::new(crate::platform::module_paths::HOST_ATTACH_DISABLE_FILE).exists() {
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::Relaxed) {
            log::warn!(
                "fuse host disabled by marker file={} effect=no-host-session namespace-bind-only",
                crate::platform::module_paths::HOST_ATTACH_DISABLE_FILE
            );
        }
        return false;
    }
    true
}

/// 环境变量侧的开关判定（一次性读取并留档）。
fn host_attach_env_enabled() -> bool {
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

/// 探测宿主自身挂载点是否仍然可达。
///
/// attach 上报的 ENOTCONN 有两类来源：宿主 FUSE endpoint 已死（进程假活、连接已断），
/// 或目标应用 namespace 里残留的死挂载层（路径解析穿过它时报错，宿主本身健康）。
/// 宿主挂载点位于 daemon/companion 自身可访问的模块目录内，对它发起一次**必然穿透到
/// FUSE 连接**的探测，是区分两类故障最直接的探针。两类故障的处理完全不同：前者必须
/// 撤销快照并终止假活宿主；后者杀宿主只会触发"恢复→再被杀"的循环，把能力快照的
/// 退避越推越深。
///
/// 探测不能只 `stat` 挂载点本身：根 dentry 的属性缓存在连接死亡后仍然有效，僵死的
/// 宿主会因此被误判存活（真机实证：duchamp 2026-10-22 13:22，MediaProvider 重启后宿主
/// media 视图连接死亡，`endpoint=alive` 误判使 attach 反复 ENOTCONN，直到宿主进程
/// 自己退出才恢复）。这里改为 lookup 一个**必然不存在**的探测名——该操作无法走任何
/// 缓存，必须由宿主 FUSE 连接真实应答：连接健康时返回 ENOENT，连接僵死时返回
/// ENOTCONN/EIO 或超时。探测名意外存在同样视为存活（连接应答了请求即证明健康）。
fn host_endpoint_alive(view: &HostSessionView) -> bool {
    let Ok(c_ns_path) = CString::new(format!("/proc/{}/ns/mnt", view.child_pid)) else {
        return false;
    };
    let Ok(c_path) = CString::new(format!("{}/.srx_endpoint_probe", view.mount_point)) else {
        return false;
    };

    // 宿主 FUSE 路径绝不能在 daemon 本体内探测：连接僵死时这次 lookup 会把调用者自己
    // 卡进不可中断的 dentry 查找（真机实证 duchamp：daemon 停在 d_alloc_parallel →
    // newfstatat → KernelSU sucompat 的 D 态，随后整机被迫重启）。子进程是一次性的，
    // 父进程保持可响应，仍能按会话状态自愈。
    crate::logging::prepare_for_fork();
    // SAFETY: fork 出的子进程只用 libc/setns/stat 并以 _exit 退出，不触碰 Rust 运行时状态。
    let child = unsafe { libc::fork() };
    if child < 0 {
        return false;
    }
    if child == 0 {
        // SAFETY: 两个 C 串在整个子进程调用序列内保持有效。
        unsafe {
            let ns_fd = libc::open(c_ns_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            if ns_fd < 0 || libc::setns(ns_fd, libc::CLONE_NEWNS) != 0 {
                libc::_exit(2);
            }
            libc::close(ns_fd);
            let mut buf = std::mem::MaybeUninit::<libc::stat>::uninit();
            let result = libc::stat(c_path.as_ptr(), buf.as_mut_ptr());
            if result == 0 {
                libc::unlink(c_path.as_ptr());
                libc::_exit(0);
            }
            libc::_exit(if crate::platform::errno::last() == libc::ENOENT {
                0
            } else {
                1
            });
        }
    }

    wait_child_exit(child, 5_000) == Some(0)
}

/// 有界等待 fork 子进程退出，返回退出码；超时、被信号终止或 fork 异常一律返回 `None`。
///
/// 子进程可能停在不可中断的 D 态（FUSE 挂死时探测里的 stat 就会这样），所以不能阻塞
/// waitpid：那会让调用线程陪着一起卡死，正是要避免的失效模式。这里到点即 SIGKILL，
/// 再交给 `reap_bounded` 做有界回收。等待的唯一事件源是内核的进程退出，不是对状态的
/// 反复查询。
fn wait_child_exit(child: i32, timeout_ms: i64) -> Option<i32> {
    let deadline = paths::monotonic_ms() + timeout_ms;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: child 是本进程 fork 出的子进程；WNOHANG 不阻塞。
        let waited = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if waited == child {
            return if libc::WIFEXITED(status) {
                Some(libc::WEXITSTATUS(status))
            } else {
                None
            };
        }
        if waited < 0 {
            return None;
        }
        if paths::monotonic_ms() >= deadline {
            // SAFETY: child 已确认仍在运行；SIGKILL 只作用于它。
            unsafe { libc::kill(child, libc::SIGKILL) };
            reap_bounded(child, "probe");
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
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
pub fn attach_app_to_host(
    view: &HostSessionView,
    target_root: &str,
    timeout_sec: i64,
) -> Option<HostAttach> {
    if target_root.is_empty() {
        return None;
    }
    // 接入子进程要对 target_root 做 open_tree + move_mount，这条路径遍历应用命名空间里的
    // FUSE 挂载；共享会话只有一条连接，开机风暴时 fuse_get_req 可能把子进程卡在 D 态，
    // 只有 SIGKILL 能打断。因此等待窗口必须由调用方（挂载子进程）按自己的剩余预算给出，
    // 而不是固定 10 秒——固定值比父进程的等待预算还长时，挂载子进程必然先被判卡死杀掉。
    let timeout_sec = timeout_sec.clamp(1, HOST_ATTACH_TIMEOUT_SEC);
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
        // SOCK_DGRAM 对端退出没有 EOF：所有正常失败都必须显式回包，避免
        // 已退出的接入子进程仍让父进程空等 10 秒，挤占后续应用的挂载窗口。
        let result = host_attach_child_main(view, target_root);
        let ok = send_host_attach_result(ready_sockets[1], result) && result == 0;
        host_stage(if ok { "attach_ok" } else { "attach_failed" });
        // SAFETY: _exit 终止子进程，不跑 atexit。
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // SAFETY: 父进程关闭子端 fd 后等待就绪。
    unsafe { libc::close(ready_sockets[1]) };
    let ready = recv_host_ready(ready_sockets[0], timeout_sec);
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
            if code == -libc::ENOTCONN {
                // 接入子进程在目标路径确认可达后仍收到 move_mount ENOTCONN。ENOTCONN 有两类
                // 来源：宿主 FUSE endpoint 已死（进程假活），或目标应用 namespace 里残留的
                // 死挂载层（路径解析穿过它时报错；"dead layer invisible" 形态挂在祖先路径上，
                // mountinfo 中没有该精确路径的条目，接入子进程既看不见也不敢清理）。
                // 杀宿主前必须先探测宿主自己的挂载点：宿主健康时故障在目标侧，杀掉它只会
                // 触发"恢复→再被同样的接入失败杀掉"的循环，并把能力快照的退避越推越深。
                log::warn!(
                    "fuse host attach reports dead conn child={} host={} source={}",
                    child,
                    view.child_pid,
                    view.mount_source
                );
                if host_endpoint_alive(view) {
                    log::warn!(
                        "fuse host attach target-side dead layer host={} target={} host_endpoint=alive keep_session=true",
                        view.child_pid,
                        target_root
                    );
                    // 宿主存活并正在服务其它应用，本身就是设备 FUSE 能力可用的铁证。
                    // 该应用自身的 namespace 脏（目标侧死层）不代表设备能力故障：若不在此
                    // 复位，脏应用回退 scoped fork 的失败会按设备级失败累积，多个脏应用
                    // 连续失败即把整机推入 Unavailable+退避；退避期间一切 FUSE 尝试被
                    // 关闭、没有成功可以复位，窗口粘住直到退避到期，期间启动的应用全部
                    // 被迫 namespace 回退。
                    crate::fuse_redirect::config::record_fuse_capability_result(
                        true,
                        "host_alive_target_dirty",
                        "srx_fuse_host",
                    );
                } else {
                    // endpoint 已确认失效时立即撤销同代际快照并终止假活宿主，自愈通道在
                    // 下一轮重建新会话；本进程若无权终止（或已退出），僵尸态/实例判活兜底。
                    revoke_host_snapshot(
                        "attach_endpoint_unreachable",
                        view.child_pid,
                        view.child_start_time_ticks,
                    );
                    terminate_host_process(view.child_pid);
                }
            }
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
                timeout_sec,
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
        host_pid: view.child_pid,
        host_start_time_ticks: view.child_start_time_ticks,
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
fn host_attach_child_main(view: &HostSessionView, target_root: &str) -> i32 {
    // 1. 先钉住当前（应用）命名空间：克隆要在宿主命名空间做，句柄是唯一的回头路。
    let Ok(c_app_ns) = CString::new("/proc/self/ns/mnt") else {
        return -1;
    };
    // SAFETY: c_app_ns 是 NUL 结尾的合法路径。
    let app_ns = unsafe { libc::open(c_app_ns.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if app_ns < 0 {
        log_errno("fuse host attach app ns open failed");
        return -1;
    }
    // SAFETY: app_ns 是本函数打开的有效 fd，只在切回时使用。
    let app_ns = UniqueFd::new(app_ns);

    // 2. 进入宿主命名空间做克隆。
    let host_ns_path = format!("/proc/{}/ns/mnt", view.child_pid);
    let Ok(c_host_ns) = CString::new(host_ns_path) else {
        return -1;
    };
    // SAFETY: c_host_ns 是 NUL 结尾的合法路径。
    let host_ns = unsafe { libc::open(c_host_ns.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if host_ns < 0 {
        log_errno("fuse host attach host ns open failed");
        return -1;
    }
    // SAFETY: host_ns 是本函数打开的有效 namespace fd。
    let host_ns = UniqueFd::new(host_ns);
    // SAFETY: host_ns 是有效的 mount namespace fd。
    if unsafe { libc::setns(host_ns.get(), libc::CLONE_NEWNS) } != 0 {
        log_errno("fuse host attach setns host failed");
        return -1;
    }

    let Ok(c_mount_point) = CString::new(view.mount_point.as_str()) else {
        return -1;
    };
    let tree_fd = open_detached_mount(&c_mount_point);

    // 3. 无论克隆成败都必须切回应用命名空间：后续 mkdir 与附着都必须在应用视图里发生。
    // SAFETY: app_ns 是本子进程进入宿主命名空间前钉住的自身命名空间 fd。
    if unsafe { libc::setns(app_ns.get(), libc::CLONE_NEWNS) } != 0 {
        log_errno("fuse host attach setns app failed");
        return -1;
    }
    if tree_fd < 0 {
        let errno = crate::platform::errno::last();
        log::warn!(
            "fuse host attach open_tree failed mp={} errno={} {}",
            view.mount_point,
            errno,
            crate::platform::errno::text(errno)
        );
        return -1;
    }
    // SAFETY: tree_fd 是 open_tree 返回的有效 fd。
    let tree_fd = UniqueFd::new(tree_fd);

    // 3.5 目标死层清理：宿主/会话换代后，上一代挂载仍残留在应用命名空间里——接入时
    //     `make_mount_private` 已切断传播，新会话的挂载无法传播覆盖它。死 FUSE 层会让
    //     目标路径解析在 `move_mount` 之前就返回 ENOTCONN（内核 `do_move_mount` 本身
    //     不产生该错误，它只能来自穿过死连接的路径解析），接入从此永远失败。返回值供
    //     move_mount 失败时区分「目标有死层」与「目标已确认可达、ENOTCONN 只能来自
    //     宿主连接」两类故障。
    let target_had_dead_layers = clear_dead_srx_layers_at_target(target_root);

    // 4. 目标目录：与应用挂载路径同源，允许已存在。
    let Ok(c_target) = CString::new(target_root) else {
        return -1;
    };
    // SAFETY: c_target 是 NUL 结尾的合法路径；失败只可能是已存在（后续附着会给出结论）。
    unsafe {
        libc::mkdir(c_target.as_ptr(), 0o755);
    }

    // 4.5 传播隔离：接入发生在应用**真实**命名空间里，`/storage/...` 的系统挂载处于
    //     共享传播组（vold 依赖它向各应用传播存储变化）。附着事件本身会沿传播组外播
    //     ——未配置应用的命名空间也会被同一视图盖住（duchamp 实测：termux、vtools 等
    //     未登记应用的 /storage/emulated/0 与三个 /mnt/... 别名位置出现 srx_fuse_host，
    //     策略 deny_all 令未登记应用视图变空目录、写全被拒）。move_mount 之后再私有化
    //     只能切断新挂载自身的传播组关系，救不回附着瞬间已经外播的拷贝，因此必须
    //     **先**私有化目标。
    //     仅当目标是挂载点才需要隔离：无存储授权应用的 /storage/emulated/0 只是
    //     应用命名空间根树上的普通目录（mountinfo 无记录），附着不产生外播；此时
    //     跳过私有化（对普通目录发 MS_PRIVATE 会 EINVAL，2026-10-29 真机实测）。
    //     私有化失败即放弃本次接入：宁可本应用暂时没有视图，也不把泄漏扩散给其它应用。
    if !crate::mount_ledger::live_mounts_at(0, target_root).is_empty()
        && !make_mount_private(&c_target)
    {
        log::warn!(
            "fuse host attach target privatize failed target={}",
            target_root
        );
        return -1;
    }

    // 5. 把游离挂载附着到目标路径，并立刻切断传播关系。
    if let Err(errno) = move_detached_mount(tree_fd.get(), &c_target) {
        // 补记本次接入所用的宿主身份与 errno，用于区分「宿主换代后视图过期」与
        // 「目标路径残留死挂载」两类故障。
        log::warn!(
            "fuse host attach move_mount detail host_pid={} source={} target={} errno={} {}",
            view.child_pid,
            view.mount_source,
            target_root,
            errno,
            crate::platform::errno::text(errno)
        );
        // 保持 ENOTCONN 的既有分类：目标侧仍有死层时不得误杀健康宿主；
        // 只有原先会回报 -ENOTCONN 的路径保留该错误码，其余失败回报通用失败。
        return if !target_had_dead_layers && errno == libc::ENOTCONN {
            -errno
        } else {
            -1
        };
    }
    if !make_mount_private(&c_target) {
        return -1;
    }

    // 6. 复核。挂载源必须就是本次会话的挂载源：只靠系统调用返回 0 无法区分
    //    "挂在应用视图里" 与 "挂在了别处"，而误判成接入成功会让应用静默失去重定向。
    let Some(live) = crate::mount_ledger::topmost_live_mount(0, target_root) else {
        log::warn!("fuse host attach not visible target={}", target_root);
        return -1;
    };
    if live.source != view.mount_source {
        log::warn!(
            "fuse host attach source mismatch target={} expected={} actual={} fs={}",
            target_root,
            view.mount_source,
            live.source,
            live.fs_type
        );
        return -1;
    }

    // 7. 覆盖存储别名族。主目标之外的同义存储出口（`/mnt/user/<user>/emulated/<user>` 等）
    //    仍指向系统 MediaProvider FUSE（pass_through 甚至直通 f2fs 后端），走别名写入的
    //    组件会完全绕开宿主策略落到真实存储。失败不回滚：主目标已成功，别名只补覆盖面。
    attach_host_view_to_storage_aliases(target_root, view);

    0
}

/// 解析存储视图根路径里的用户号（`/storage/emulated/<user>` → `Some(user)`）。
fn storage_user_id_from_view_root(target_root: &str) -> Option<i32> {
    target_root
        .strip_prefix("/storage/emulated/")
        .filter(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|rest| rest.parse::<i32>().ok())
}

/// 判断某个存储别名是否属于 **f2fs 直连后端**（系统进程做下层 I/O 的通道）。
///
/// 这类别名不是应用可见视图，而是挂载链的最底层：`/data/media/<user>` 是用户存储的
/// f2fs 真身，`/mnt/pass_through/<owner>/emulated[/<user>]` 是系统给 MediaProvider 等
/// 特权进程准备的直通出口，两者最终都落在同一个 f2fs 挂载上。
///
/// **不能把宿主 FUSE 视图 bind 到这些路径上**：系统进程（尤其是 MediaProvider 的
/// FuseDaemon）处理 FUSE 请求时要回到下层真实文件系统做 I/O，一旦下层被模块的 FUSE
/// 视图占住，这次下层 I/O 就会经内核 VFS 重新进入模块自己的 FUSE 连接，请求只能由
/// 同一批工作线程应答——线程互相等待、连接整体挂死。这与
/// `MountPlanner::bind_own_private_restore` 只覆盖 `/storage/` 别名是同一条结论。
fn is_f2fs_direct_backend_alias(alias_root: &str, user_id: i32) -> bool {
    if paths::is_same_or_child(alias_root, &paths::data_media_user_root_for_user(user_id)) {
        return true;
    }
    let user = user_id.to_string();
    let pass_through = [
        format!("/mnt/pass_through/{user}/emulated"),
        format!("/mnt/pass_through/emulated/{user}"),
    ];
    pass_through
        .iter()
        .any(|root| paths::is_same_or_child(alias_root, root))
}

/// 把宿主视图同步覆盖到存储别名族（collapsed hybrid FUSE 形态的收尾步骤）。
///
/// 共享宿主接入默认只把视图根挂到主存储根（`/storage/emulated/<user>`）。但同一棵存储树
/// 在应用命名空间里还有一整族等价别名（`/mnt/user/<user>/emulated/<user>`、
/// `/mnt/installer/...`、`/mnt/androidwritable/...`），它们仍指向系统 MediaProvider FUSE，
/// 走别名写入的组件（真机实证：QQ connect SDK 的 xlog 经别名写 `Tencent/msflogs`，
/// 2026-10-22 在 `/data/media/0/Tencent` 真实落盘且零监视记录）会完全绕开宿主策略。
/// 命名空间 bind 后端经 `mount::alias::bind_mount_with_storage_aliases` 覆盖同一族别名；
/// 宿主接入此前漏掉这一步。
///
/// 语义与 `mount::alias` 对齐：
/// - 不存在的别名目标（lstat 失败，如 `/mnt/runtime/*`）跳过；
/// - 解析后与主目标同真实路径的符号别名（`/storage/self/primary`、
///   `/storage/emulated/legacy`）去重跳过；
/// - **f2fs 直连后端别名不覆盖**：见 [`is_f2fs_direct_backend_alias`]；
/// - **别名目标 bind 前先私有化**：别名与主目标同处系统共享传播组，附着事件会沿
///   传播组外播到未配置应用的命名空间（2026-10-29 duchamp 实测），必须先切断；
/// - 别名上残留的本模块死层先摘除（与主目标同一套 [`clear_dead_srx_layers_at_target`]）；
/// - 单个别名失败只告警不回滚：主目标已成功，别名失败不能让整个接入作废。
fn attach_host_view_to_storage_aliases(target_root: &str, view: &HostSessionView) {
    let Some(user_id) = storage_user_id_from_view_root(target_root) else {
        // 非视图根目标（按目录 scoped 根）不展开别名：scoped 会话的别名覆盖仍由
        // 命名空间 bind 路径承担，这里不改变既有语义。
        return;
    };
    let Ok(primary_real) = std::fs::canonicalize(target_root) else {
        return;
    };
    for alias_root in paths::storage_alias_roots_for_user(user_id) {
        if alias_root == target_root {
            continue;
        }
        if is_f2fs_direct_backend_alias(&alias_root, user_id) {
            continue;
        }
        // 存在性检查用 lstat 语义：符号别名（primary/legacy）即便悬空也进入去重判定，
        // 不在这里对存储 FUSE 发起真实 I/O。
        if std::fs::symlink_metadata(&alias_root).is_err() {
            continue;
        }
        let Ok(alias_real) = std::fs::canonicalize(&alias_root) else {
            continue;
        };
        if alias_real == primary_real {
            continue;
        }
        // 幂等：顶层已是当前会话的视图就不再叠加（宿主换代重接入时同一会话源不重复 bind）。
        if let Some(live) = crate::mount_ledger::topmost_live_mount(0, &alias_root)
            && live.source == view.mount_source
        {
            continue;
        }
        // 传播隔离：别名目标与主目标同处系统共享传播组，bind 事件会沿传播组外播到
        // 未配置应用的命名空间（duchamp 实测见主目标 4.5 注释）。bind 之后再私有化
        // 救不回已外播的拷贝，必须先私有化目标；仅当别名当前是挂载点才需要（普通
        // 目录形态无外播路径）。私有化失败时跳过该别名：宁缺勿滥，决不往共享目标
        // 上叠加视图。
        let Ok(c_alias) = CString::new(alias_root.as_str()) else {
            continue;
        };
        if !crate::mount_ledger::live_mounts_at(0, &alias_root).is_empty()
            && !make_mount_private(&c_alias)
        {
            log::warn!(
                "fuse host attach alias privatize failed target={} errno={} {}",
                alias_root,
                crate::platform::errno::last(),
                crate::platform::errno::text(crate::platform::errno::last())
            );
            continue;
        }
        clear_dead_srx_layers_at_target(&alias_root);
        let Ok(c_source) = CString::new(target_root) else {
            continue;
        };
        // SAFETY: 源与目标都是 NUL 结尾的合法路径；同一命名空间内的 bind 不跨命名空间
        // （跨命名空间 MS_BIND 会 EINVAL，见 host_attach_child_main 的说明），此时主目标
        // 挂载已属于应用命名空间，bind 合法。
        if unsafe {
            libc::mount(
                c_source.as_ptr(),
                c_alias.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND as libc::c_ulong,
                std::ptr::null(),
            )
        } != 0
        {
            log::warn!(
                "fuse host attach alias bind failed target={} errno={} {}",
                alias_root,
                crate::platform::errno::last(),
                crate::platform::errno::text(crate::platform::errno::last())
            );
            continue;
        }
        if !make_mount_private(&c_alias) {
            continue;
        }
        log::info!(
            "fuse host attach alias bound target={} host={}",
            alias_root,
            view.child_pid
        );
    }
}

/// 接入子进程统一回报结果，成功与失败均只发送一次。
fn send_host_attach_result(ready_sock: libc::c_int, result: i32) -> bool {
    // SAFETY: ready_sock 是本次接入的有效端点，result 是调用期间存活的栈变量。
    let sent = unsafe {
        libc::send(
            ready_sock,
            &result as *const i32 as *const libc::c_void,
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
///
/// 失败时返回捕获到的 errno：errno 必须在写日志等后续系统调用可能改写它之前取出，
/// 调用方据此区分死连接（ENOTCONN）与其它附着失败。
fn move_detached_mount(tree_fd: libc::c_int, target: &CStr) -> Result<(), i32> {
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
        let errno = crate::platform::errno::last();
        log::error!(
            "fuse host attach move_mount failed errno={} {}",
            errno,
            crate::platform::errno::text(errno)
        );
        return Err(errno);
    }
    Ok(())
}

/// 目标路径上单次接入最多摘除的死层层数。
///
/// 宿主/会话连续多次换代且每代都给同一应用留过死层的极端情况下层数会更多，8 层足够
/// 覆盖自愈语义；超限时保留现场并让本次接入按既有失败路径处理，避免无限循环。
const MAX_DEAD_TARGET_LAYERS: usize = 8;

/// 接入前摘除目标路径上残留的本模块死挂载层。
///
/// 上一代宿主/会话死亡后，它的挂载层仍残留在应用命名空间里：接入时 `make_mount_private`
/// 已切断传播，新会话的挂载无法传播覆盖它。死 FUSE 层会让目标路径的解析在 `move_mount`
/// 之前就返回 ENOTCONN——内核 `do_move_mount` 本身不产生该错误，它只能来自穿过死连接的
/// 路径解析——接入从此永远失败（场景 27 连续五轮复现：换代前接入过的应用全挂，全新
/// 应用正常）。
///
/// 摘除条件同时要求：`stat` 穿到死连接（ENOTCONN/EIO——顶层挂载存活时 stat 走顶层、
/// 不会命中底下的死层），且最顶层挂载确为本模块会话 FUSE（scoped 或宿主，前缀判定
/// 统一走 `is_srx_session_fuse`）。系统挂载与尚且可达的挂载一概不碰。
///
/// 返回是否摘除过死层，供 `move_mount` 失败时区分目标侧与源侧故障。
fn clear_dead_srx_layers_at_target(target: &str) -> bool {
    let mut removed = 0;
    while removed < MAX_DEAD_TARGET_LAYERS {
        let errno = match std::fs::metadata(target) {
            // 目标路径可达：无死层，或死层已在前几轮摘净。
            Ok(_) => return removed > 0,
            Err(error) => error.raw_os_error().unwrap_or_default(),
        };
        if errno != libc::ENOTCONN && errno != libc::EIO {
            // ENOENT 等交给后续 mkdir 与 move_mount 的既有逻辑处理。
            return removed > 0;
        }
        let Some(live) = crate::mount_ledger::topmost_live_mount(0, target) else {
            log::warn!(
                "fuse host attach dead layer invisible target={} errno={}",
                target,
                errno
            );
            return removed > 0;
        };
        if !crate::fuse_redirect::config::is_srx_session_fuse(&live.fs_type, &live.source) {
            log::warn!(
                "fuse host attach dead layer not ours target={} fs={} source={} errno={}",
                target,
                live.fs_type,
                live.source,
                errno
            );
            return removed > 0;
        }

        // 失效的系统 FUSE 主别名本身可能无法作为 umount2 的解析路径（返回 ENOTCONN），
        // 但同一应用命名空间通常还保留 /mnt/user、/mnt/pass_through 等系统别名。它们
        // 指向同一用户存储视图，且不穿过失效的 /storage/emulated/0 dentry；先验证别名
        // 顶层挂载仍是本模块，再从可达别名执行 MNT_DETACH，避免按路径盲卸系统挂载。
        let mut detach_targets = vec![target.to_string()];
        if let Some(relative) = storage_alias_relative_path(target) {
            for alias_root in crate::platform::paths::storage_alias_roots_for_user(0) {
                let alias = if relative.is_empty() {
                    alias_root
                } else {
                    format!("{alias_root}/{relative}")
                };
                if alias != target && !detach_targets.iter().any(|path| path == &alias) {
                    detach_targets.push(alias);
                }
            }
        }
        let mut detached_target = None;
        for candidate in &detach_targets {
            let Some(candidate_live) = crate::mount_ledger::topmost_live_mount(0, candidate) else {
                continue;
            };
            if !crate::fuse_redirect::config::is_srx_session_fuse(
                &candidate_live.fs_type,
                &candidate_live.source,
            ) {
                continue;
            }
            let Ok(c_candidate) = CString::new(candidate.as_str()) else {
                continue;
            };
            // SAFETY: c_candidate 是 NUL 结尾的合法路径；MNT_DETACH 只标记已确认归属
            // 的本模块挂载层，不会向 FUSE endpoint 发起文件 I/O。
            if unsafe { libc::umount2(c_candidate.as_ptr(), libc::MNT_DETACH) } == 0 {
                detached_target = Some(candidate.clone());
                break;
            }
        }
        let Some(detached_target) = detached_target else {
            log_errno("fuse host attach dead layer detach failed");
            dump_dead_target_forensics(target, &detach_targets);
            return removed > 0;
        };
        removed += 1;
        log::warn!(
            "fuse host attach dead layer detached target={} source={} errno={}",
            detached_target,
            live.source,
            errno
        );
    }
    log::warn!(
        "fuse host attach dead layer removal limit target={} layers={}",
        target,
        removed
    );
    removed > 0
}

fn storage_alias_relative_path(target: &str) -> Option<String> {
    let root = crate::platform::paths::storage_user_root_for_user(0);
    let relative = target.strip_prefix(&root)?;
    Some(relative.trim_start_matches('/').to_string())
}

/// 死层清理失败取证：打印目标路径的真实文件形态与当前命名空间的 FUSE/存储
/// 挂载条目。用户设备曾出现「挂载点变成 0B 文件」的形态，但缺少应用命名空间
/// 的 mountinfo 无法确诊——取证失败的原因（EINVAL 与哪条挂载对应）。
fn dump_dead_target_forensics(target: &str, detach_targets: &[String]) {
    let stat_err = match std::fs::metadata(target) {
        Ok(metadata) => format!("ok dir={} size={}", metadata.is_dir(), metadata.len()),
        Err(error) => format!("errno={}", error.raw_os_error().unwrap_or_default()),
    };
    log::error!(
        "fuse host attach dead layer forensics target={} stat={stat_err}",
        target
    );
    for candidate in detach_targets {
        log::error!(
            "fuse host attach dead layer forensics candidate={} stat-errno={}",
            candidate,
            std::fs::metadata(candidate)
                .err()
                .and_then(|error| error.raw_os_error())
                .map_or_else(|| "ok".to_string(), |code| code.to_string())
        );
    }
    if let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") {
        let target_tail = target.rsplit('/').next().unwrap_or_default();
        for line in mountinfo.lines() {
            let is_relevant = line.contains("fuse")
                || line.contains(target)
                || (target_tail.len() > 2 && line.contains(target_tail));
            if is_relevant {
                log::error!("fuse host attach dead layer forensics mountinfo {line}");
            }
        }
    }
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

/// 摘除传播进宿主命名空间的宿主 FUSE 副本（/data/media/0）——事件触发、单次执行。
///
/// 应用接入把宿主挂载 move_mount 到应用命名空间的 /storage/emulated/0 后，
/// AOSP 存储 bind 链的 shared 传播组（用户设备实证 master:104）会把同一挂载传播
/// 回宿主命名空间的 /data/media/0——该路径是宿主数据面 backend 的直连 f2fs
/// 真身，被覆盖后任何 backend stat 的路径解析都会命中自家连接的 dentry 重验证，
/// 请求发回自己的连接、无人应答，全部 worker 死锁（用户设备两次停摆同源，慢调用
/// 探针实证卡点）。
///
/// 触发时机由 daemon 在每次 attach 成功（move_mount 完成、传播已发生）后经控制
/// 通道通知，事件精确覆盖副本出现的唯一窗口；数据面本身已由 f2fs 直连锚点
/// 解耦（见 paths_roots 的 MEDIA_REAL_ANCHOR_ROOT），摘除只承担挂载表卫生。
///
/// 只摘确认归属本模块会话的挂载（is_srx_session_fuse），系统挂载与底层 f2fs 一概
/// 不碰；摘除同时会让已挂起的重验证请求完成。摘除或无可摘都留运行日志。
pub(crate) fn unpropagate_media_realm() {
    // 数据面 backend 全部位于主用户的 /data/media/0 之下；多用户副本
    // （/data/media/999 等）不参与宿主数据面，出现时由后续需求再扩展。
    let target = "/data/media/0";
    let Some(live) = crate::mount_ledger::topmost_live_mount(0, target) else {
        log::debug!(
            "fuse host media realm unpropagate skip target={} no-mount",
            target
        );
        return;
    };
    if !crate::fuse_redirect::config::is_srx_session_fuse(&live.fs_type, &live.source) {
        log::warn!(
            "fuse host media realm unpropagate not-ours target={} fs={} source={}",
            target,
            live.fs_type,
            live.source
        );
        return;
    }
    let Ok(c_target) = std::ffi::CString::new(target) else {
        return;
    };
    // SAFETY: c_target 是 NUL 结尾的合法路径；MNT_DETACH 只摘已确认归属本模块
    // 会话的传播副本，不发起任何文件 I/O。
    if unsafe { libc::umount2(c_target.as_ptr(), libc::MNT_DETACH) } == 0 {
        log::warn!(
            "fuse host media realm unpropagated target={} mount_id={} source={}",
            target,
            live.mount_id,
            live.source
        );
    } else {
        log_errno("fuse host media realm unpropagate failed");
    }
}

/// 宿主数据面 f2fs 直连锚点：bind /data/media/0 到模块私有目录（private，不参与
/// 传播组）。必须在任何应用接入之前建立——此时数据面仍属真实块设备文件系统实例；
/// 之后路径被覆盖也不影响经锚点的解析（见 paths_roots 的 MEDIA_REAL_ANCHOR_ROOT
/// 文档）。
///
/// 用户设备换代现场（gen 恢复）实证：上一代死会话的 FUSE 残留仍盖在 /data/media/0
/// 顶层，此时若直接拒绝建立锚点，数据面将永久走 FUSE 视图、停摆循环复发。因此
/// 顶层若确认归属本模块会话（is_srx_session_fuse），先 MNT_DETACH 摘除残留再复查
/// ——本函数运行于 mount_host_fuse 之前，此刻存在的任何 srx 会话 FUSE 都属于已
/// 死亡的上一代，摘除安全。只有顶层仍是非本模块的 FUSE/模拟视图（无法判定归属，
/// 如异构 OEM 变体）才拒绝建立并回退；无锚点时数据面依赖看门狗换代兜底，因此
/// 宁可摘残留也不把污染路径搬进锚点，但也不轻易放弃锚点。
///
/// `pub(crate)`：宿主会话运行期按周期校验锚点挂载仍在，发现丢失时由会话循环
/// 调用本函数就地重建（gen4 冻结现场实证锚点 bind 会静默消失，根因为 MS_BIND
/// 复制共享传播组成员时新 bind 加入同一 peer 组、系统侧 umount 传播连带卸载，
/// 已由 bind 后 MS_PRIVATE 私有化根治；重建仅作最后防线）。重建
/// 语义与启动期一致：摘本模块残留、拒绝污染覆盖、bind f2fs 真身）。
static MEDIA_REAL_ANCHOR_IDENTITY: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();

fn path_object_identity(path: &str) -> Option<(u64, u64)> {
    let path = std::ffi::CString::new(path).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: path 是 NUL 结尾字符串，stat 指向可写结构；仅查询对象元数据。
    if unsafe { libc::stat(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: stat 成功后内核已完整初始化结构。
    let stat = unsafe { stat.assume_init() };
    // Android 的 64 位目标上 `st_dev`/`st_ino` 本身就是 `u64`；这里显式标注返回类型，
    // 不在表达式里重复 cast（clippy::unnecessary_cast）。
    Some((stat.st_dev, stat.st_ino))
}

/// 验证锚点仍是原始存储目录的私有 f2fs bind，而不是路径上任意一个挂载。
pub(crate) fn media_real_anchor_is_valid() -> bool {
    let Some(anchor) = crate::platform::paths::media_real_anchor_root() else {
        return false;
    };
    let Some(expected_identity) = MEDIA_REAL_ANCHOR_IDENTITY.get().copied() else {
        return false;
    };
    if path_object_identity(anchor) != Some(expected_identity) {
        return false;
    }
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let mut entries = Vec::new();
    for line in mountinfo.lines() {
        let Some(entry) = crate::platform::mountinfo::parse_entry(line) else {
            continue;
        };
        if crate::platform::mountinfo::unescape_field(entry.target) != anchor {
            continue;
        }
        let Some((before_sep, _)) = line.split_once(" - ") else {
            continue;
        };
        let private = before_sep.split_whitespace().skip(6).all(|field| {
            !field.starts_with("shared:")
                && !field.starts_with("master:")
                && !field.starts_with("propagate_from:")
        });
        entries.push((
            entry.mount_id,
            entry.parent_mount_id,
            crate::platform::mountinfo::unescape_field(entry.root),
            entry.fs_type.to_string(),
            private,
        ));
    }
    let Some(top) =
        crate::platform::mountinfo::topmost_by_stack(&entries, |entry| entry.0, |entry| entry.1)
    else {
        return false;
    };
    let direct_fs = is_direct_userdata_fs(&top.3);
    direct_fs && top.4
}

/// 用户数据区直连内核文件系统的判据。
///
/// 锚点要的是 `/data/media` 背后的真实内核文件系统：主流真机是 f2fs，模拟器与部分
/// 设备是 ext4。这里只排除「视图类」文件系统（FUSE 与 sdcardfs），不再写死 f2fs——
/// 写死会让 ext4 设备与模拟器在锚点校验处直接拒绝启动宿主（CI Android 15 实测：
/// `media anchor verification failed fs=ext4` → `startup refused`）。
fn is_direct_userdata_fs(fs_type: &str) -> bool {
    !fs_type.is_empty() && !fs_type.contains("fuse") && fs_type != "sdcardfs"
}

/// 建立并验证宿主数据面私有锚点。
///
/// 返回 `true` 表示锚点已挂载且已脱离传播组；返回 `false` 时调用方必须拒绝
/// 启动宿主 FUSE，不能回退到已知会触发自再入死锁的 `/data/media/<user>` 直连。
pub(crate) fn establish_media_real_anchor() -> bool {
    let user_id = 0;
    let source = crate::platform::paths::data_media_user_root_for_user(user_id);
    let mut residue_count = 0u32;
    while let Some(live) = crate::mount_ledger::topmost_live_mount(0, &source) {
        if crate::fuse_redirect::config::is_srx_session_fuse(&live.fs_type, &live.source) {
            residue_count += 1;
            if residue_count > 8 {
                log::warn!(
                    "fuse host media anchor refuse target={} reason=residue-stack count={residue_count}",
                    source
                );
                return false;
            }
            let Ok(c_target) = std::ffi::CString::new(source.as_str()) else {
                return false;
            };
            // SAFETY: c_target 是 NUL 结尾的合法路径；仅摘除已确认属于本模块的残留挂载。
            if unsafe { libc::umount2(c_target.as_ptr(), libc::MNT_DETACH) } == 0 {
                log::warn!(
                    "fuse host media anchor residue detached target={} mount_id={} source={} seq={residue_count}",
                    source,
                    live.mount_id,
                    live.source
                );
                continue;
            }
            log_errno("fuse host media anchor residue detach failed");
            return false;
        }
        let poisoned = live.fs_type.contains("fuse") || live.fs_type.contains("sdcardfs");
        if poisoned {
            log::warn!(
                "fuse host media anchor refuse target={} fs={} source={}",
                source,
                live.fs_type,
                live.source
            );
            return false;
        }
        break;
    }
    let anchor = format!(
        "{}/real_media_anchor/{user_id}",
        crate::platform::module_paths::MODULE_DIR
    );
    if let Err(error) = std::fs::create_dir_all(&anchor) {
        log::warn!("fuse host media anchor mkdir failed path={anchor} err={error}");
        return false;
    }
    let (Ok(c_source), Ok(c_anchor)) = (
        std::ffi::CString::new(source.as_str()),
        std::ffi::CString::new(anchor.as_str()),
    ) else {
        return false;
    };
    // SAFETY: 两个路径都是 NUL 结尾的合法字符串；MS_BIND 只建立挂载关系。
    if unsafe {
        libc::mount(
            c_source.as_ptr(),
            c_anchor.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        )
    } != 0
    {
        log_errno("fuse host media anchor bind failed");
        return false;
    }
    // SAFETY: c_anchor 是 NUL 结尾的合法路径；MS_PRIVATE 使锚点脱离所有传播组。
    if unsafe {
        libc::mount(
            std::ptr::null(),
            c_anchor.as_ptr(),
            std::ptr::null(),
            libc::MS_PRIVATE,
            std::ptr::null(),
        )
    } != 0
    {
        log_errno("fuse host media anchor privatize failed");
        return false;
    }
    let Some(source_identity) = path_object_identity(&source) else {
        log::error!("fuse host media anchor verification failed source={source} reason=stat");
        return false;
    };
    let Some(anchor_identity) = path_object_identity(&anchor) else {
        log::error!("fuse host media anchor verification failed anchor={anchor} reason=stat");
        return false;
    };
    if anchor_identity != source_identity {
        log::error!(
            "fuse host media anchor verification failed anchor={} source={} anchor_identity={:?} source_identity={:?}",
            anchor,
            source,
            anchor_identity,
            source_identity
        );
        return false;
    }
    let Some(mounted) = crate::mount_ledger::topmost_live_mount(0, &anchor) else {
        log::error!("fuse host media anchor verification failed anchor={anchor} reason=missing");
        return false;
    };
    if !is_direct_userdata_fs(&mounted.fs_type) {
        log::error!(
            "fuse host media anchor verification failed anchor={} fs={} root={} source={}",
            anchor,
            mounted.fs_type,
            mounted.root,
            mounted.source
        );
        return false;
    }
    let _ = MEDIA_REAL_ANCHOR_IDENTITY.set(anchor_identity);
    crate::platform::paths::set_media_real_anchor_root(anchor.clone());
    if !media_real_anchor_is_valid() {
        log::error!(
            "fuse host media anchor verification failed anchor={anchor} reason=not-private"
        );
        return false;
    }
    // 锚点建立成功是一次性正常事件：留档到独立取证文件，运行日志按信息级记录，
    // 避免以错误级别出现让现场误判为失败（详细日志关闭时不写运行日志）。
    crate::fuse_redirect::config::forensics_record(&format!(
        "fuse host media anchor ready source={source} anchor={anchor} private fs={} source={} root={}",
        mounted.fs_type, mounted.source, mounted.root
    ));
    log::info!(
        "fuse host media anchor ready anchor={anchor} fs={}",
        mounted.fs_type
    );
    true
}

/// 子进程入口：进入私有 namespace、收敛为 slave、挂直通 FUSE、设 shared，然后常驻。
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
    // 2. 收敛为 slave：只接收入向挂载事件，不向外传播。
    //
    // 这里必须用 `MS_SLAVE` 而不是 `MS_PRIVATE`。`MS_PRIVATE` 会同时切断两个方向，
    // 于是 init 命名空间在宿主会话建立之后创建的挂载永远进不来。HyperOS 的应用商店
    // 在每次下载时都会把 `/data/apk-tmp/<pkg>/download` bind 到
    // `/data/media/0/Android/data/com.xiaomi.market/files/.siexternal`，而宿主后端根
    // 直连 f2fs（`private_real_root`），一旦错过这个 bind，宿主读到的是 bind 遮蔽
    // 之前的陈旧目录：真机实测那里残留着若干已卸载应用的下载暂存目录
    // （`com.kylin.read`、`com.taobao.idlefish` 等），商店校验阶段读不到自己刚下载的
    // APK，表现为 `failed as error=3` 且不重试。
    //
    // 出向隔离不依赖这一步：宿主挂载点在 `mount_host_fuse` 里显式设 shared（只与自身
    // peer 组互通），应用接入时 `make_mount_private` 又把搬运过去的游离挂载改私有，
    // 因此宿主与应用的挂载事件不会互相污染。
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
            (libc::MS_REC | libc::MS_SLAVE) as libc::c_ulong,
            std::ptr::null(),
        )
    } != 0
    {
        log_errno("fuse host MS_SLAVE failed");
        host_stage("slave_failed");
        return false;
    }
    host_stage("slave_ok");

    if !establish_media_real_anchor() {
        log::error!("fuse host startup refused: media anchor unavailable");
        host_stage("anchor_failed");
        return false;
    }

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
        is_monitor_only: false,
        // 宿主会话是直通会话：`redirect_target` 取存储根本身，策略侧据此跳过重定向根推导。
        is_passthrough_host: true,
        namespace_policy_only: false,
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
    // SAFETY: SYS_pidfd_open 只按 pid 创建内核 fd，第三个实参 0u32 是无标志常量，
    // 不接触调用方内存；返回值经 as 转换为 c_int，失败时为负 errno。
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, host.child_pid, 0u32) as libc::c_int };
    let old_pidfd = HOST_PIDFD.swap(pidfd, Ordering::AcqRel);
    if old_pidfd >= 0 {
        // SAFETY: old_pidfd 是此前由 pidfd_open 创建并原子换出的有效 fd；swap 之后
        // 本进程不再持有任何引用，close 只释放该 fd，不会与其它线程竞争。
        unsafe { libc::close(old_pidfd) };
    }
    HOST_GENERATION.fetch_add(1, Ordering::AcqRel);
    if let Ok(mut reg) = HOST_REGISTRY.lock() {
        *reg = Some((host.child_pid, std::collections::BTreeMap::new()));
    }
    let view = HostSessionView::from(&host);
    if let Ok(mut slot) = host_slot().write() {
        // 以当前宿主句柄替换旧快照，供两条挂载路径读取。
        // quality-allow(chinese-language): 下方为解引用赋值代码行，会被块注释模式误读为文本，并非英文自然语言。
        *slot = Some(Arc::new(host));
    }
    // 新会话身份发布到快照文件：companion 进程没有本进程的内存状态，
    // 只能靠文件发现宿主会话。uid 策略指纹表从空开始，随后随登记递增。
    // quality-allow(chinese-language): 下方固定字段名属于跨进程快照协议。
    publish_host_snapshot(&view);
    // 宿主会话建立即 FUSE 能力可用的最强证据：会话本身持有活的 FUSE 挂载并完成策略
    // 注册。MediaProvider 重启触发的宿主换代期间，各应用的接入失败会快速累积设备级
    // 失败计数与指数退避；不在新会话就绪时立即复位，换代窗口后启动的应用会继续按
    // unavailable 走 namespace 回退，即使宿主已经健康存活。
    crate::fuse_redirect::config::record_fuse_capability_result(
        true,
        "host_session_ready",
        "srx_fuse_host",
    );
    // daemon 自身的 mount namespace 直通登记：宿主换代后旧登记随之失效，每代会话
    // 就绪时重新登记一次。失败不阻断（宿主刚就绪，控制通道理论上可用；失败只损失
    // uid=0 请求的直通优化，保持原有 namespace miss 回退语义）。
    crate::fuse_host_control::register_daemon_namespace_passthrough();
}

/// daemon 侧登记台账：当前宿主会话 pid + 已登记 uid 集合。
///
/// 快照文件是唯一跨进程事实源，内存台账只是它的写入缓存；宿主会话重建时整体替换。
static HOST_REGISTRY: Mutex<Option<(i32, std::collections::BTreeMap<u32, u64>)>> = Mutex::new(None);
static HOST_SNAPSHOT_LOCK: Mutex<()> = Mutex::new(());

fn registered_policy_snapshot() -> std::collections::BTreeMap<u32, u64> {
    HOST_REGISTRY
        .lock()
        .map(|registry| {
            registry
                .as_ref()
                .map(|(_, policies)| policies.clone())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// 原子重写宿主会话快照。companion 只读，必须看到自洽内容：先写临时文件再 rename。
fn revoke_host_snapshot(reason: &str, child_pid: i32, child_start_time_ticks: u64) {
    let _guard = HOST_SNAPSHOT_LOCK.lock().ok();
    let path = std::path::Path::new(crate::platform::module_paths::FUSE_HOST_SNAPSHOT_FILE);
    let expected_pid = format!("pid={child_pid}");
    let expected_start = format!("start_ticks={child_start_time_ticks}");
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let matches_identity = content.lines().any(|line| line == expected_pid)
        && content.lines().any(|line| line == expected_start);
    if matches_identity {
        let _ = std::fs::remove_file(path);
        log::warn!(
            "fuse host snapshot revoked reason={} pid={} start_ticks={}",
            reason,
            child_pid,
            child_start_time_ticks
        );
    }
}

fn publish_host_snapshot(view: &HostSessionView) {
    let _guard = HOST_SNAPSHOT_LOCK.lock().ok();
    use std::io::Write;
    let path = std::path::Path::new(crate::platform::module_paths::FUSE_HOST_SNAPSHOT_FILE);
    let tmp = path.with_extension("snapshot.tmp");
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let content = format!(
        "boot_id={}\npid={}\nstart_ticks={}\nmount_point={}\nmount_source={}\npolicies={}\n",
        boot_id,
        view.child_pid,
        view.child_start_time_ticks,
        view.mount_point,
        view.mount_source,
        view.policy_fingerprints
            .iter()
            .map(|(uid, fingerprint)| format!("{uid}:{fingerprint}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let write_ok = (|| {
        let mut file = std::fs::File::create(&tmp).ok()?;
        file.write_all(content.as_bytes()).ok()?;
        file.sync_all().ok()?;
        Some(())
    })()
    .is_some();
    if !write_ok {
        log::warn!("fuse host snapshot write failed path={}", path.display());
        return;
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        log::warn!(
            "fuse host snapshot rename failed path={} err={}",
            path.display(),
            error
        );
    }
}

/// 登记成功后更新该 UID 对应的策略指纹。
pub(crate) fn record_registered_policy(uid: u32, policy_fingerprint: u64) {
    let mut need_publish = false;
    if let Ok(mut reg) = HOST_REGISTRY.lock()
        && let Some((_, policies)) = reg.as_mut()
    {
        need_publish = policies.get(&uid) != Some(&policy_fingerprint);
        policies.insert(uid, policy_fingerprint);
    }
    if need_publish && let Some(host) = get_fuse_host() {
        publish_host_snapshot(&HostSessionView::from(host.as_ref()));
        log::info!(
            "fuse host snapshot updated uid={} policy_fingerprint={:x}",
            uid,
            policy_fingerprint
        );
    }
}

/// 读取并校验宿主会话快照（companion 侧发现入口）。
///
/// 快照缺失、来自其它开机（boot_id 不符）或宿主进程已死时返回 `None`——
/// 三种情形调用方处理一致：继续 scoped 规划。
pub fn read_host_session_view() -> Option<HostSessionView> {
    let content =
        std::fs::read_to_string(crate::platform::module_paths::FUSE_HOST_SNAPSHOT_FILE).ok()?;
    let mut boot_id = String::new();
    let mut child_pid = 0i32;
    let mut child_start_time_ticks = 0u64;
    let mut mount_point = String::new();
    let mut mount_source = String::new();
    let mut policy_fingerprints = std::collections::BTreeMap::new();
    for line in content.lines() {
        let (key, value) = line.split_once('=')?;
        match key {
            "boot_id" => boot_id = value.to_string(),
            "pid" => child_pid = value.parse().ok()?,
            "start_ticks" => child_start_time_ticks = value.parse().ok()?,
            "mount_point" => mount_point = value.to_string(),
            "mount_source" => mount_source = value.to_string(),
            "policies" => {
                for entry in value.split(',').filter(|value| !value.is_empty()) {
                    let (uid, fingerprint) = entry.split_once(':')?;
                    policy_fingerprints.insert(uid.parse().ok()?, fingerprint.parse().ok()?);
                }
            }
            _ => {}
        }
    }
    let current_boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if boot_id.is_empty() || boot_id != current_boot_id {
        return None;
    }
    if !crate::platform::is_process_instance_alive(child_pid, child_start_time_ticks) {
        revoke_host_snapshot("process_dead", child_pid, child_start_time_ticks);
        return None;
    }
    // 宿主进程可能仍存活，但 MediaProvider 重启后其 FUSE endpoint 已经失效；仅凭
    // pid/start_ticks 会把旧快照继续交给 companion，最终在 move_mount 处得到 ENOTCONN。
    // 私有宿主挂载点是最便宜且不依赖应用 namespace 的健康探针，失效时先撤销快照，
    // 让新接入回退 scoped，等 daemon 重建并重新发布新代际后再接入共享宿主。
    if let Err(error) = std::fs::metadata(&mount_point) {
        let errno = error.raw_os_error();
        if errno == Some(libc::ENOTCONN) || errno == Some(libc::EIO) || errno == Some(libc::ENOENT)
        {
            revoke_host_snapshot("endpoint_unreachable", child_pid, child_start_time_ticks);
            return None;
        }
    }
    Some(HostSessionView {
        child_pid,
        child_start_time_ticks,
        mount_point,
        mount_source,
        policy_fingerprints,
    })
}

/// 等待宿主快照文件发生变化。
///
/// daemon 通过临时文件 rename 原子发布快照，监听其父目录才能同时覆盖新建、替换和轮换；
/// inotify 不可用时保留有界的短暂 sleep 作为兼容 fallback。调用方每次返回后都会重新读取
/// 并校验快照，因此事件丢失不会改变 fail-closed 语义。
fn wait_for_host_snapshot_change(timeout_ms: i64, ready: impl Fn() -> bool) {
    let snapshot = crate::platform::module_paths::FUSE_HOST_SNAPSHOT_FILE;
    let Some(waiter) = crate::platform::inotify::FileChangeWaiter::new(&[snapshot]) else {
        std::thread::sleep(std::time::Duration::from_millis(timeout_ms.max(1) as u64));
        return;
    };
    // 建立 watch 之后必须复核就绪条件：调用方上次读取与 inotify_add_watch 之间发生的
    // 发布事件会落在 watch 建立之前，不复核会把本可立即完成的登记变成一轮全额等待。
    if ready() {
        return;
    }
    let deadline = crate::platform::paths::monotonic_ms().saturating_add(timeout_ms.max(1));
    let _ = waiter.wait_until(deadline);
}

/// companion 侧有界等待：直到指定 uid 的当前策略已被 daemon 预登记进宿主会话。
///
/// 预登记由 daemon 周期 reconcile 与配置/控制事件唤醒共同完成，等待预算必须覆盖
/// 事件驱动的最坏情形。等待的是"当前策略就绪"而不是"会话存在"或 UID 曾登记——
/// 旧策略会把配置切换后的请求路由到错误后端。超时或处于失败冷却期返回 `None`，
/// 调用方回退旧 scoped 规划。
pub fn wait_for_host_session_view(uid: i32, policy_fingerprint: u64) -> Option<HostSessionView> {
    if !host_attach_enabled() {
        return None;
    }
    if let Some(view) = read_host_session_view()
        && view.policy_fingerprints.get(&(uid as u32)) == Some(&policy_fingerprint)
    {
        return Some(view);
    }
    let now = crate::platform::paths::monotonic_ms();
    let last_fail = LAST_HOST_WAIT_FAIL_MS.load(Ordering::Relaxed);
    if last_fail != 0 && now.saturating_sub(last_fail) < HOST_WAIT_FAIL_COOLDOWN_MS {
        return None;
    }
    let deadline = now.saturating_add(HOST_WAIT_BUDGET_MS as i64);
    loop {
        if let Some(view) = read_host_session_view()
            && view.policy_fingerprints.get(&(uid as u32)) == Some(&policy_fingerprint)
        {
            return Some(view);
        }
        let current = crate::platform::paths::monotonic_ms();
        if current >= deadline {
            LAST_HOST_WAIT_FAIL_MS.store(current, Ordering::Relaxed);
            let snapshot_fingerprint = read_host_session_view()
                .and_then(|view| view.policy_fingerprints.get(&(uid as u32)).copied());
            log::warn!(
                "fuse host view wait timeout uid={} budget_ms={} request_fingerprint={:016x} snapshot_fingerprint={:?} fallback=scoped_planning",
                uid,
                HOST_WAIT_BUDGET_MS,
                policy_fingerprint,
                snapshot_fingerprint
            );
            return None;
        }
        let remaining = deadline.saturating_sub(current);
        wait_for_host_snapshot_change(remaining, || {
            read_host_session_view()
                .and_then(|view| view.policy_fingerprints.get(&(uid as u32)).copied())
                == Some(policy_fingerprint)
        });
    }
}

pub fn clear_if_dead() -> bool {
    clear_if_dead_with_reason("health_check")
}

pub(crate) fn clear_if_dead_with_reason(reason: &str) -> bool {
    let mut dead_pid = None;
    let cleared = host_slot()
        .write()
        .ok()
        .map(|mut s| {
            // 仅当槽位里的宿主句柄确实失效时才清除，避免误伤正在使用的会话。
            let dead = s.as_ref().is_some_and(|host| {
                let dead = host_is_dead(host);
                if dead {
                    dead_pid = Some(host.child_pid);
                }
                dead
            });
            if dead {
                *s = None;
            }
            dead
        })
        .unwrap_or(false);
    if cleared {
        // 会话已死，控制端不再有对端；一并丢弃，避免后续注册请求写到死会话。
        close_host_control_fd();
        let old_pidfd = HOST_PIDFD.swap(-1, Ordering::AcqRel);
        if old_pidfd >= 0 {
            // SAFETY: old_pidfd 是此前由 pidfd_open 创建并原子换出的有效 fd；swap 之后
            // 本进程不再有任何引用，close 只释放该 fd，不会与其它线程竞争。
            unsafe { libc::close(old_pidfd) };
        }
        HOST_GENERATION.fetch_add(1, Ordering::AcqRel);
        log::warn!(
            "fuse host invalidated reason={} child={} generation={}",
            reason,
            dead_pid.unwrap_or(-1),
            HOST_GENERATION.load(Ordering::Acquire)
        );
    }
    cleared
}

/// 返回宿主代际；宿主每次重建或失效都会变化。
pub fn generation() -> u64 {
    HOST_GENERATION.load(Ordering::Acquire)
}

/// 返回宿主 pidfd，供 daemon 事件循环等待宿主退出。
pub fn pidfd() -> i32 {
    HOST_PIDFD.load(Ordering::Acquire)
}

/// 返回 daemon 当前持有的宿主身份，用于识别应用状态文件里的旧会话。
pub fn current_identity() -> Option<(i32, u64)> {
    let host = host_slot().read().ok()?.as_ref()?.clone();
    host_is_alive(&host).then_some((host.child_pid, host.child_start_time_ticks))
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
///
/// 接入被显式关闭时**不建立宿主**：这条闸门必须覆盖宿主的全部使用路径，否则关闭
/// 接入只是「没有应用接入」，宿主本身仍会建立、仍会被换代，而每次换代都会给已经
/// 附着的命名空间留下死挂载层——设备侧用它做 A/B 归因时，看到的现象与开启时没有
/// 差别，也就无法定位宿主是否为问题的来源。
pub fn ensure_global() -> bool {
    if !host_attach_enabled() {
        return false;
    }
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
        log::info!(
            "fuse host wait skipped cooldown_ms={} last_fail_ms_ago={}",
            HOST_WAIT_FAIL_COOLDOWN_MS,
            now.saturating_sub(last_fail)
        );
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
        let remaining = deadline.saturating_sub(current);
        wait_for_host_snapshot_change(remaining, || get_fuse_host().is_some());
    }
}

// ===== 外部 abort 兜底监控（daemon 侧，最后一道解冻能力）=====
//
// 背景（2026-10-29 真机冻结实证）：宿主内停摆看门狗的探测链路自身可能被冻住
// （探针陷入 D 态、回收路径阻塞），confirmed/dump/abort 全部无法发生，全系统
// 冻死直到强制断电。daemon 是独立进程、不服务任何 FUSE 请求，由它保留兜底：
//
// 触发（宿主心跳给出嫌疑）→ 验证（daemon 亲自进宿主命名空间取证）→ abort。
// 三段缺一不可：心跳只是嫌疑，直接 abort 会误杀慢而活的会话；验证必须同时
// 满足「waiting 持续 > 0」与「端到端探测超时」两条硬证据。
//
// 宿主内看门狗先于本监控触发（阈值更短），因此本路径只在看门狗失效时才可能
// 走到 abort——这正是它的设计定位。

/// 心跳轮询间隔。
const EXTERNAL_ABORT_POLL_MS: u64 = 2_000;
/// 心跳陈旧阈值：宿主心跳线程每 2 秒覆写一次，超过该值未更新说明宿主进程整体
/// 冻结（所有线程含非 FUSE 线程都停了）或已退出（退出场景由 pid 活性检查排除）。
const EXTERNAL_ABORT_BEAT_STALE_MS: i64 = 20_000;
/// 无完成时长阈值：心跳里 last_completion 距今超过该值且 waiting 积压，视为
/// 停摆嫌疑。必须大于宿主内看门狗的全链路延迟（6s+10s+4s），保证看门狗健康时
/// 本监控永远轮不到动手。
const EXTERNAL_ABORT_NO_COMPLETION_MS: i64 = 25_000;
/// 验证子进程两次 waiting 采样的间隔。
const EXTERNAL_ABORT_VERIFY_SAMPLE_MS: u64 = 5_000;
/// 验证子进程端到端探测的超时。
const EXTERNAL_ABORT_PROBE_TIMEOUT_MS: u64 = 12_000;
/// 嫌疑验证的最小间隔：alive 结论下按该节流复验，避免冻结期间高频 fork。
const EXTERNAL_ABORT_VERIFY_THROTTLE_MS: i64 = 60_000;
/// abort 之后的冷却窗：换代重建需要时间，期间心跳必然异常，禁止重复动手。
const EXTERNAL_ABORT_COOLDOWN_MS: i64 = 120_000;

/// 验证子进程退出码：探测应答（alive）。
const VERIFY_EXIT_ANSWERED: i32 = 0;
/// 验证子进程退出码：waiting 清零（alive）。
const VERIFY_EXIT_WAITING_CLEARED: i32 = 2;
/// 验证子进程退出码：命名空间/sysfs 不可达（证据不足，放弃）。
const VERIFY_EXIT_UNAVAILABLE: i32 = 3;

/// 宿主心跳内容（解析自 [`crate::platform::module_paths::FUSE_HOST_HEARTBEAT_FILE`]）。
#[derive(Debug)]
struct HostHeartbeat {
    boot_id: String,
    host_pid: i32,
    conn: i32,
    waiting: u64,
    last_completion_ms: i64,
    wall_ms: i64,
}

/// 外部 abort 兜底验证结论。
#[derive(Debug, Eq, PartialEq)]
enum ExternalVerdict {
    /// 有 alive 证据（探测应答 / waiting 清零），不动手。
    Alive(&'static str),
    /// 硬证据齐了：waiting 持续积压 + 端到端探测超时（含探针 SIGKILL 后仍不退出）。
    Stalled,
    /// 证据不足（命名空间进不去、sysfs 消失等），本轮放弃。
    Unavailable(&'static str),
}

/// 启动 daemon 侧外部 abort 兜底监控线程。
///
/// 宿主未建立会话时心跳文件不存在，线程空转成本为每 2 秒一次文件读取。
pub fn spawn_external_abort_monitor() {
    // 接入关闭时没有宿主会话要解冻：监控线程只会空转读一个不存在的
    // 心跳文件，且它的 abort 动作本来就是为宿主会话准备的。
    if !host_attach_enabled() {
        log::info!("daemon external abort monitor skipped: host attach disabled");
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("srx-external-abort".to_string())
        .spawn(external_abort_monitor_main);
    if spawned.is_err() {
        log::warn!("daemon external abort monitor spawn failed");
    }
}

fn external_abort_monitor_main() {
    log::info!("daemon external abort monitor started");
    let mut last_verify_ms = 0i64;
    let mut cooldown_until_ms = 0i64;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(EXTERNAL_ABORT_POLL_MS));
        if !crate::runtime_control::is_module_runtime_enabled() {
            continue;
        }
        let now = paths::monotonic_ms();
        if now < cooldown_until_ms {
            continue;
        }
        let Some(beat) = read_host_heartbeat() else {
            continue;
        };
        // 上次开机的残留心跳（文件写在模块目录，重启后仍在）：boot_id 不符即忽略。
        if beat.boot_id != read_own_boot_id() {
            continue;
        }
        // 宿主进程已退出（正常换代/崩溃）：交给既有 pidfd 换代通道，不归本监控管。
        if !process_alive(beat.host_pid) {
            continue;
        }
        let wall_now = wall_ms();
        let beat_age = wall_now.saturating_sub(beat.wall_ms);
        let completion_age = if beat.last_completion_ms > 0 {
            wall_now.saturating_sub(beat.last_completion_ms)
        } else {
            0
        };
        let beat_stale = beat_age > EXTERNAL_ABORT_BEAT_STALE_MS;
        let completion_stalled =
            beat.waiting > 0 && completion_age > EXTERNAL_ABORT_NO_COMPLETION_MS;
        if !beat_stale && !completion_stalled {
            continue;
        }
        if now.saturating_sub(last_verify_ms) < EXTERNAL_ABORT_VERIFY_THROTTLE_MS {
            continue;
        }
        last_verify_ms = now;
        log::warn!(
            "daemon external stall suspicion host_pid={} conn={} waiting={} beat_age_ms={} completion_age_ms={} trigger={} verifying",
            beat.host_pid,
            beat.conn,
            beat.waiting,
            beat_age,
            completion_age,
            if beat_stale {
                "beat-stale"
            } else {
                "no-completion"
            }
        );
        match verify_host_stall_externally(&beat) {
            ExternalVerdict::Alive(reason) => {
                log::warn!(
                    "daemon external stall verdict=alive host_pid={} conn={} reason={}",
                    beat.host_pid,
                    beat.conn,
                    reason
                );
            }
            ExternalVerdict::Unavailable(reason) => {
                log::warn!(
                    "daemon external stall verdict=unavailable host_pid={} conn={} reason={}",
                    beat.host_pid,
                    beat.conn,
                    reason
                );
            }
            ExternalVerdict::Stalled => {
                crate::fuse_redirect::config::forensics_log(&format!(
                    "daemon external stall confirmed host_pid={} conn={} waiting={} beat_age_ms={} completion_age_ms={} action=abort",
                    beat.host_pid, beat.conn, beat.waiting, beat_age, completion_age
                ));
                dump_external_stall_forensics(&beat);
                let aborted = abort_host_connection_externally(&beat);
                crate::fuse_redirect::config::forensics_log(&format!(
                    "daemon external abort done host_pid={} conn={} aborted={} effect=requests-error-out host-exits generation-rebuild",
                    beat.host_pid, beat.conn, aborted
                ));
                cooldown_until_ms = paths::monotonic_ms() + EXTERNAL_ABORT_COOLDOWN_MS;
            }
        }
    }
}

/// 读取并解析宿主心跳文件；缺失或格式不符返回 None（静默：宿主未建会话是常态）。
fn read_host_heartbeat() -> Option<HostHeartbeat> {
    let text =
        std::fs::read_to_string(crate::platform::module_paths::FUSE_HOST_HEARTBEAT_FILE).ok()?;
    let fields: Vec<&str> = text.trim().split('|').collect();
    if fields.len() != 7 || fields[0] != "srx_beat" {
        return None;
    }
    Some(HostHeartbeat {
        boot_id: fields[1].to_string(),
        host_pid: fields[2].parse().ok()?,
        conn: fields[3].parse().ok()?,
        waiting: fields[4].parse().ok()?,
        last_completion_ms: fields[5].parse().ok()?,
        wall_ms: fields[6].parse().ok()?,
    })
}

/// 当前墙钟毫秒（与宿主心跳的 wall_ms 同口径，宿主与 daemon 共享系统墙钟）。
fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn read_own_boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

fn process_alive(pid: i32) -> bool {
    // kill 0 只做权限与存在性探测，不发信号。
    // SAFETY: 信号 0 是 POSIX 规定的探测用法，不作用于目标进程。
    unsafe { libc::kill(pid, 0) == 0 }
}

/// 外部验证：fork 子进程进入宿主命名空间采样 waiting 并做端到端探测。
///
/// 子进程动作序列（全部无 Rust 状态依赖，fork 后安全）：
/// 1. setns 进宿主 mnt 命名空间——宿主会话连接是游离挂载，daemon 自己的命名空间
///    看不到（设计如此），必须进入宿主的命名空间才能读它的 sysfs；
/// 2. 读 waiting，为 0 或读不到 → alive（EXIT_WAITING_CLEARED）；
/// 3. 等 5 秒再读一次，仍 > 0 才继续——单次积压可能只是瞬时负载；
/// 4. stat 探针路径：应答（含 ENOENT）→ alive（EXIT_ANSWERED）；停摆则挂起，
///    由父进程按超时裁决。
///
/// 父进程全程 WNOHANG 轮询，绝不阻塞：探针 SIGKILL 后仍不退出（D 态等 abort 唤醒）
/// 与超时同样按 Stalled 裁决——TASK_KILLABLE 只在 FUSE 应答或 abort 时醒来，
/// 杀不死本身就是队列已死的证据。
fn verify_host_stall_externally(beat: &HostHeartbeat) -> ExternalVerdict {
    let Ok(c_ns_path) = CString::new(format!("/proc/{}/ns/mnt", beat.host_pid)) else {
        return ExternalVerdict::Unavailable("ns-path-invalid");
    };
    let Ok(c_probe) = CString::new(format!(
        "{}/.srx_stall_probe",
        crate::platform::module_paths::FUSE_HOST_MOUNT_DIR
    )) else {
        return ExternalVerdict::Unavailable("probe-path-invalid");
    };
    let Ok(c_waiting) = CString::new(format!("/sys/fs/fuse/connections/{}/waiting", beat.conn))
    else {
        return ExternalVerdict::Unavailable("waiting-path-invalid");
    };
    crate::logging::prepare_for_fork();
    // SAFETY: fork 前已完成日志准备；子进程只用 libc 与 _exit，不触碰 Rust 运行时状态。
    let child = unsafe { libc::fork() };
    if child < 0 {
        return ExternalVerdict::Unavailable("fork-failed");
    }
    if child == 0 {
        // SAFETY: 三个 c_* 均为 NUL 结尾静态串，指针在整个子进程生命周期内有效。
        unsafe {
            let ns_fd = libc::open(c_ns_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            if ns_fd < 0 {
                libc::_exit(VERIFY_EXIT_UNAVAILABLE);
            }
            if libc::setns(ns_fd, libc::CLONE_NEWNS) != 0 {
                libc::_exit(VERIFY_EXIT_UNAVAILABLE);
            }
            libc::close(ns_fd);
            for _ in 0..2 {
                match read_waiting_raw(&c_waiting) {
                    None => libc::_exit(VERIFY_EXIT_UNAVAILABLE),
                    Some(0) => libc::_exit(VERIFY_EXIT_WAITING_CLEARED),
                    Some(_) => {}
                }
                libc::sleep((EXTERNAL_ABORT_VERIFY_SAMPLE_MS / 1_000) as libc::c_uint);
            }
            let mut buf = std::mem::MaybeUninit::<libc::stat>::uninit();
            // 结果不检查：ENOENT 与成功同样是「连接可应答」的证据，其它 errno 也是。
            libc::stat(c_probe.as_ptr(), buf.as_mut_ptr());
            libc::_exit(VERIFY_EXIT_ANSWERED);
        }
    }
    // 父进程裁决窗：2 次采样间隔 + 探测超时 + 余量。
    let budget_ms = (EXTERNAL_ABORT_VERIFY_SAMPLE_MS * 2 + EXTERNAL_ABORT_PROBE_TIMEOUT_MS) as i64;
    let deadline = paths::monotonic_ms() + budget_ms;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: child 是本次 fork 出的子进程；WNOHANG 不阻塞。
        let waited = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if waited == child {
            return if libc::WIFEXITED(status) {
                match libc::WEXITSTATUS(status) {
                    VERIFY_EXIT_ANSWERED => ExternalVerdict::Alive("probe-answered"),
                    VERIFY_EXIT_WAITING_CLEARED => ExternalVerdict::Alive("waiting-cleared"),
                    VERIFY_EXIT_UNAVAILABLE => ExternalVerdict::Unavailable("ns-or-sysfs"),
                    _ => ExternalVerdict::Unavailable("unexpected-exit"),
                }
            } else {
                ExternalVerdict::Unavailable("child-signalled")
            };
        }
        if waited < 0 {
            return ExternalVerdict::Unavailable("waitpid-failed");
        }
        if paths::monotonic_ms() >= deadline {
            // SAFETY: child 已确认仍存活；SIGKILL 只作用于它。
            unsafe { libc::kill(child, libc::SIGKILL) };
            reap_bounded(child, "external-verify");
            return ExternalVerdict::Stalled;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 在（已 setns 进宿主命名空间的）当前进程读 waiting 计数；仅验证子进程使用。
///
/// # SAFETY
/// c_waiting 必须指向 NUL 结尾串；本函数在 fork 后的子进程内调用，无并发。
unsafe fn read_waiting_raw(c_waiting: &CString) -> Option<u64> {
    // SAFETY: c_waiting 是 NUL 结尾静态串，open 只读不写。
    let fd = libc::open(c_waiting.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
    if fd < 0 {
        return None;
    }
    let mut buffer = [0u8; 32];
    // SAFETY: fd 是本函数刚打开的有效描述符，buffer 为栈上可写缓冲。
    let n = libc::read(fd, buffer.as_mut_ptr() as *mut libc::c_void, buffer.len());
    // SAFETY: fd 是本函数打开的描述符，读取结束即清理。
    libc::close(fd);
    if n <= 0 {
        return None;
    }
    std::str::from_utf8(&buffer[..n as usize])
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// 有界回收 fork 子进程：WNOHANG 轮询至多 3 秒，杀不死就记录状态后放弃。
fn reap_bounded(child: i32, tag: &str) {
    let deadline = paths::monotonic_ms() + 3_000;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: child 是本进程 fork 出的子进程；WNOHANG 不阻塞。
        let waited = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if waited == child || waited < 0 {
            return;
        }
        if paths::monotonic_ms() >= deadline {
            let state = std::fs::read_to_string(format!("/proc/{child}/stat"))
                .ok()
                .and_then(|text| {
                    text.rsplit_once(')')
                        .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "unknown".to_string());
            crate::fuse_redirect::config::forensics_log(&format!(
                "daemon external {tag} child unkillable pid={} state={} abandoned",
                child, state
            ));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 外部 abort：fork 子进程进宿主命名空间写连接 abort 节点。
///
/// abort 生效后内核把在途与后续请求全部置错：卡死的宿主 worker 全部带错退出，
/// 探针 D 态进程随之释放，全系统解冻；宿主进程随会话退出，由既有 pidfd 换代
/// 通道重建。
fn abort_host_connection_externally(beat: &HostHeartbeat) -> bool {
    let Ok(c_ns_path) = CString::new(format!("/proc/{}/ns/mnt", beat.host_pid)) else {
        return false;
    };
    let Ok(c_abort) = CString::new(format!("/sys/fs/fuse/connections/{}/abort", beat.conn)) else {
        return false;
    };
    crate::logging::prepare_for_fork();
    // SAFETY: fork 前已完成日志准备；子进程只用 libc 与 _exit。
    let child = unsafe { libc::fork() };
    if child < 0 {
        log::error!("daemon external abort fork failed");
        return false;
    }
    if child == 0 {
        // SAFETY: c_* 均为 NUL 结尾静态串。
        unsafe {
            let ns_fd = libc::open(c_ns_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            if ns_fd < 0 {
                libc::_exit(VERIFY_EXIT_UNAVAILABLE);
            }
            if libc::setns(ns_fd, libc::CLONE_NEWNS) != 0 {
                libc::_exit(VERIFY_EXIT_UNAVAILABLE);
            }
            libc::close(ns_fd);
            let written = libc::open(c_abort.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
            if written < 0 {
                libc::_exit(VERIFY_EXIT_UNAVAILABLE);
            }
            let payload = b"1\n";
            libc::write(
                written,
                payload.as_ptr() as *const libc::c_void,
                payload.len(),
            );
            libc::close(written);
            libc::_exit(0);
        }
    }
    // abort 节点是纯 sysfs 写入，不经过 FUSE 数据面，正常毫秒级；仍给有界窗口。
    let deadline = paths::monotonic_ms() + 5_000;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: child 是本次 fork 出的子进程；WNOHANG 不阻塞。
        let waited = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if waited == child {
            return libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
        }
        if waited < 0 {
            return false;
        }
        if paths::monotonic_ms() >= deadline {
            // SAFETY: child 已确认仍存活；SIGKILL 只作用于它。
            unsafe { libc::kill(child, libc::SIGKILL) };
            reap_bounded(child, "external-abort");
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 外部 abort 前的 daemon 侧取证：宿主进程全景 + 全系统 D 态 + 内核日志过滤尾。
///
/// 宿主内看门狗的 candidate 取证可能已随看门狗线程一起冻结，这里从 daemon 侧
/// 独立再抓一份——两份现场互为备份，冻结归因不依赖任何单点。
fn dump_external_stall_forensics(beat: &HostHeartbeat) {
    let pid = beat.host_pid;
    if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
        for line in status.lines() {
            if line.starts_with("State:")
                || line.starts_with("Threads:")
                || line.starts_with("VmRSS:")
            {
                crate::fuse_redirect::config::forensics_log(&format!(
                    "daemon external stall host-status pid={pid} {line}"
                ));
            }
        }
    }
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        crate::fuse_redirect::config::forensics_log(&format!(
            "daemon external stall host task-list-failed pid={pid}"
        ));
        return;
    };
    for task in tasks.flatten().take(48) {
        let tid_path = task.path();
        let tid = task.file_name().to_string_lossy().to_string();
        let comm = std::fs::read_to_string(tid_path.join("comm"))
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
        let wchan = std::fs::read_to_string(tid_path.join("wchan"))
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
        crate::fuse_redirect::config::forensics_log(&format!(
            "daemon external stall host-thread tid={tid} comm={comm} wchan={wchan}"
        ));
        if let Ok(syscall) = std::fs::read_to_string(tid_path.join("syscall")) {
            let syscall = syscall.trim();
            if !syscall.is_empty() {
                crate::fuse_redirect::config::forensics_log(&format!(
                    "daemon external stall host-syscall tid={tid} {syscall}"
                ));
            }
        }
        if let Ok(stack) = std::fs::read_to_string(tid_path.join("stack")) {
            for line in stack.lines().take(16) {
                crate::fuse_redirect::config::forensics_log(&format!(
                    "daemon external stall host-stack tid={tid} {line}"
                ));
            }
        }
    }
    crate::fuse_redirect::config::dump_system_dstate_tasks();
    crate::fuse_redirect::config::dump_kmsg_forensics();
}
