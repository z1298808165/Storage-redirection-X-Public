//! daemon 自守护看门狗：pidfd 事件驱动的独立守护进程。
//!
//! 取代 service.sh 中每 20 秒 `pidof` 轮询存活并按固定间隔重启的 shell 看门狗。
//! 旧方案的两个根因问题：空闲期也周期性唤醒（功耗 + 与其它唤醒源叠加）；
//! 重启延迟以 20 秒间隔粒度量化（daemon 崩溃后最长 20 秒无 daemon）。
//!
//! 事件链：daemon 启动早期 fork 并 exec 出一个独立守护进程（comm=`srx_watchdog`，
//! 与 `srx_daemon` 进程名区分，避免 `pidof`/存活检测误判），它对目标进程
//! `pidfd_open` 并阻塞在 `poll` 上——进程退出由内核事件立即唤醒，空闲零唤醒。
//! 退出后按序判定：
//!
//! 1. 已有其它存活 daemon（如 WebUI 恰好先拉起）→ 退出，新 daemon 会孵化
//!    自己的守护进程，避免双守护互相拉扯。
//! 2. 清理 daemon 遗留的 FUSE 子进程（srx_fuse_host/srx_fuse，它们 fork 未
//!    exec，持有实例锁 fd 会阻止任何新 daemon 获取实例锁）——**无论 daemon
//!    因何退出都要清理**，否则 stop/start 循环后模块无法再启动。
//! 3. runtime disable 文件存在 → daemon 是被用户主动停止的，清理完遗留后
//!    守护进程随之退出；下次由 WebUI/下次开机重新拉起（各自路径都会孵化新
//!    守护进程）。
//! 4. 否则按崩溃退避间隔拉起新 daemon。
//!
//! 拉起的 daemon 通过 `SRX_DAEMON_WATCHDOG_CHILD` 环境变量得知自己由守护进程
//! 孵化，不再重复孵化，保证任何时刻至多一条守护链。
//!
//! 守护进程自身死亡时的保护窗口：与旧 shell 看门狗一致（它被杀后同样失去
//! 保护），在 daemon 下次启动时自愈；不为补这个窗口引入轮询。
//!
//! 兼容性：pidfd_open 需内核 ≥5.3，本模块的目标设备（Android 14+，内核
//! 5.10+）均满足；极端老旧内核回退为对 `/proc/<pid>` 的 5 秒存在性检查，
//! 此时轮询是该内核上唯一可用的存活观察手段。

use crate::logging::Logger;
use libc::{POLLIN, PR_SET_NAME, WNOHANG, c_int, c_uint, c_ulong, poll, pollfd, prctl, waitpid};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// daemon 稳定运行超过该时长后，下次崩溃的重启间隔回落到基准值。
const RESTART_GRACE_MS: u64 = 30_000;
/// 崩溃退避基准间隔（daemon 拉起后立刻退出时的重启延迟）。
const RESTART_BACKOFF_BASE_MS: u64 = 1_000;
/// 崩溃退避上限，防止 daemon 启动即失败时形成紧致拉起风暴。
const RESTART_BACKOFF_MAX_MS: u64 = 60_000;
/// 无 pidfd 内核上的存在性检查回退间隔（详见模块文档兼容性说明）。
const PROC_FALLBACK_POLL_MS: u64 = 5_000;
/// 守护进程的进程名；与 srx_daemon 明确区分，避免存活检测把守护进程算进去。
const WATCHDOG_PROC_NAME: &[u8] = b"srx_watchdog\0";
/// daemon 的 comm 名（二进制名），存活检测口径与 shell 端一致。
const DAEMON_PROC_NAME: &[u8] = b"srx_daemon";
/// 需要在重启前清理的 daemon 遗留 FUSE 子进程（继承 flock 与 FUSE 连接）。
const FUSE_CHILD_PROC_NAMES: [&[u8]; 2] = [b"srx_fuse_host", b"srx_fuse"];
/// 告知被拉起 daemon「由守护进程孵化」的环境变量。
const WATCHDOG_CHILD_ENV: &str = "SRX_DAEMON_WATCHDOG_CHILD=1";
/// /proc/self/exe readlink 在二进制被替换后追加的后缀（内核 proc_pid_get_link）。
const DELETED_EXE_SUFFIX: &str = " (deleted)";

/// 解析自身可执行文件路径，供 fork 后 exec 复用。
///
/// `/proc/self/exe` 的 readlink 结果在二进制被替换（原地升级、模块刷入）后
/// 会带 ` (deleted)` 后缀——旧 inode 已卸链但运行中进程仍指向它。必须剥离该
/// 后缀 exec 同名新文件：看门狗的整个存在意义就是 daemon 死后拉起新实例，
/// 而升级场景恰恰是 daemon 最可能死亡的时机，此时路径上的后缀会让 exec
/// 永久 ENOENT、守护链静默失效。同路径新文件内容即为升级后的正确二进制。
fn self_exec_path() -> Option<PathBuf> {
    let raw = std::fs::read_link("/proc/self/exe").ok()?;
    let text = raw.to_string_lossy();
    let stripped = text.strip_suffix(DELETED_EXE_SUFFIX).unwrap_or(&text);
    Some(PathBuf::from(stripped.to_string()))
}

/// daemon 启动早期孵化自守护进程。
///
/// 守护进程通过 fork + exec 摆脱 daemon 的全部继承 fd（std 打开的 fd 均带
/// O_CLOEXEC，实例锁随 exec 释放）；exec 失败只告警不致命——行为退化为
/// 「无自动重启」，与引入本模块之前一致。
pub fn spawn_self_watchdog() {
    if std::env::var_os("SRX_DAEMON_WATCHDOG_CHILD").is_some() {
        // 由守护进程孵化的 daemon 不再孵化，保证全局至多一条守护链。
        return;
    }
    let Some(exec_path) = self_exec_path() else {
        log::warn!("daemon watchdog spawn skipped: self exe unresolved");
        return;
    };
    let Ok(c_exec) = CString::new(exec_path.as_os_str().as_encoded_bytes()) else {
        log::warn!("daemon watchdog spawn skipped: self exe invalid");
        return;
    };
    crate::logging::prepare_for_fork();
    // SAFETY: getpid 无副作用。
    let self_pid = unsafe { libc::getpid() };
    // SAFETY: fork 只分裂进程；子进程立即 exec，不触碰任何 Rust 所有权数据。
    let child = unsafe { libc::fork() };
    if child < 0 {
        log::warn!(
            "daemon watchdog fork failed errno={}",
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        );
        return;
    }
    if child != 0 {
        // 守护进程独立存活，daemon 不 wait 也不需要回收（由 init 收养）。
        return;
    }
    // 子进程：argv[0] 改名，使 cmdline 不含 srx_daemon；argv[1] 必须是命令行
    // 分发识别的 "watchdog" token（main() 按 argv[1] 路由），argv[2] 为 daemon
    // pid。显式传 pid 而非依赖 getppid：若 daemon 在本进程 exec 完成前退出，
    // getppid 会变成收养者（init），等待它毫无意义。
    // pid 为十进制数字、分发 token 与环境变量均为代码内常量，CString 构造的
    // 失败分支实际不可达；仍显式处理以守住无 unwrap/expect 的生产代码约定。
    let Ok(pid_arg) = CString::new(format!("{self_pid}")) else {
        // SAFETY: 子进程无法继续，直接退出不执行任何 Rust 析构。
        unsafe { libc::_exit(127) };
    };
    let Ok(c_dispatch) = CString::new("watchdog") else {
        // SAFETY: 子进程无法继续，直接退出不执行任何 Rust 析构。
        unsafe { libc::_exit(127) };
    };
    let argv = [
        c_exec.as_ptr(),
        c_dispatch.as_ptr(),
        pid_arg.as_ptr(),
        std::ptr::null(),
    ];
    let Ok(c_env) = CString::new(WATCHDOG_CHILD_ENV) else {
        // SAFETY: 子进程无法继续，直接退出不执行任何 Rust 析构。
        unsafe { libc::_exit(127) };
    };
    let envp = [c_env.as_ptr(), std::ptr::null()];
    // SAFETY: execve 的路径、argv、envp 在调用期间全部有效且以 null 结尾。
    unsafe { libc::execve(c_exec.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
    // SAFETY: exec 失败后直接退出，不执行任何 Rust 析构。
    unsafe { libc::_exit(127) };
}

/// 守护进程入口（srx_daemon 命令行分发 `watchdog <pid>` 调用）。
///
/// `target_pid` 是孵化自己的 daemon 进程号；缺省时回退 getppid（覆盖手工调试
/// 启动等无参数场景）。
pub fn entry(target_pid: Option<c_int>) -> i32 {
    Logger::init(Some("srx_daemon"));
    // SAFETY: PR_SET_NAME 只设置本进程名，传入 null 结尾的合法字节串。
    unsafe { prctl(PR_SET_NAME, WATCHDOG_PROC_NAME.as_ptr() as c_ulong, 0, 0, 0) };
    // SAFETY: getppid 无副作用。
    let mut target = target_pid.unwrap_or_else(|| unsafe { libc::getppid() });
    let mut backoff_ms = RESTART_BACKOFF_BASE_MS;
    // 首个观察目标（孵化自己的 daemon）的真实启动时间不可得，按守护进程启动
    // 时刻起算；若该 daemon 立即崩溃，退避从第二轮开始生效。
    let mut target_started = Instant::now();
    loop {
        wait_for_process_exit(target);
        // SAFETY: waitpid 兜底回收（初始目标可能并非本进程子进程，返回 -1 无碍）。
        let mut status: c_int = 0;
        unsafe { waitpid(target, &mut status, WNOHANG) };
        // 目标死亡时刻（内核时钟滴答）：daemon 死亡瞬间可能刚 fork 出 FUSE
        // 子进程（尚未执行到 prctl 改名，comm 仍是 srx_daemon），必须按
        // 「启动时间早于死亡时刻」识别并清理，否则它们既会被误判成
        // 「别处拉起的新 daemon」（看门狗错误退出），又会作为锁持有者孤儿
        // 阻断后续一切启动。
        let death_ticks = boot_ticks_now();
        kill_dead_daemon_children(death_ticks);
        if live_daemon_exists(death_ticks) {
            // 其它路径（WebUI 等）已拉起新 daemon，它孵化自己的守护进程。
            log::info!("watchdog exit reason=daemon_restarted_elsewhere");
            return 0;
        }
        // daemon 已死且无人接管：无论退出原因，遗留的 FUSE 子进程（fork 未 exec）
        // 都持有实例锁 fd，不清理会让下一次启动 already_running 静默退出——
        // 包括用户主动 stop 的场景，否则 stop/start 循环后模块无法再启动。
        kill_leftover_fuse_children();
        if !crate::runtime_control::is_module_runtime_enabled() {
            log::info!("watchdog exit reason=runtime_disabled");
            return 0;
        }
        let ran_ms = target_started.elapsed().as_millis() as u64;
        backoff_ms = if ran_ms >= RESTART_GRACE_MS {
            RESTART_BACKOFF_BASE_MS
        } else {
            (backoff_ms * 2).min(RESTART_BACKOFF_MAX_MS)
        };
        std::thread::sleep(Duration::from_millis(backoff_ms));
        // 睡眠期间用户可能翻转 runtime disable；复查避免拉起刚被叫停的 daemon。
        if !crate::runtime_control::is_module_runtime_enabled() {
            log::info!("watchdog exit reason=runtime_disabled_after_backoff");
            return 0;
        }
        match spawn_daemon() {
            Some(child) => {
                log::info!(
                    "watchdog respawned daemon child={} backoff_ms={}",
                    child,
                    backoff_ms
                );
                target = child;
                target_started = Instant::now();
            }
            None => {
                log::warn!("watchdog respawn failed; exiting");
                return 1;
            }
        }
    }
}

/// fork + exec 拉起新 daemon；成功返回子进程 pid。
///
/// 守护进程自身只有主线程，fork+exec 无多线程锁风险；exec 带上孵化标记环境
/// 变量，daemon 据此跳过重复孵化。
fn spawn_daemon() -> Option<c_int> {
    let exec_path = self_exec_path()?;
    let c_exec = CString::new(exec_path.as_os_str().as_encoded_bytes()).ok()?;
    let c_env = CString::new(WATCHDOG_CHILD_ENV).ok()?;
    let argv = [c_exec.as_ptr(), std::ptr::null()];
    // 继承现有环境并追加孵化标记；先剔除可能存在的旧值，避免重复定义。
    use std::os::unix::ffi::OsStrExt;
    let mut env_strings: Vec<CString> = std::env::vars_os()
        .filter(|(key, _)| key.as_bytes() != b"SRX_DAEMON_WATCHDOG_CHILD")
        .filter_map(|(key, value)| {
            let mut bytes = key.as_bytes().to_vec();
            bytes.push(b'=');
            bytes.extend_from_slice(value.as_bytes());
            CString::new(bytes).ok()
        })
        .collect();
    env_strings.push(c_env);
    let mut envp: Vec<*const libc::c_char> = env_strings.iter().map(|v| v.as_ptr()).collect();
    envp.push(std::ptr::null());
    // SAFETY: fork 后子进程立即 exec；execve 参数均为调用期间有效的 null 结尾数据。
    let child = unsafe { libc::fork() };
    if child < 0 {
        log::warn!(
            "watchdog spawn daemon fork failed errno={}",
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        );
        return None;
    }
    if child == 0 {
        // SAFETY: execve 参数在调用期间有效。
        unsafe { libc::execve(c_exec.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        // SAFETY: exec 失败后直接退出，不执行任何 Rust 析构。
        unsafe { libc::_exit(127) };
    }
    Some(child)
}

/// 阻塞等待目标进程退出。
///
/// 主路径 pidfd + 无限超时 poll：退出事件由内核送达，空闲零唤醒。pidfd 不可用
/// 的老旧内核回退为存在性检查（轮询是该内核上唯一手段，已在上文说明）。
fn wait_for_process_exit(pid: c_int) {
    // SAFETY: pidfd_open(434) 只接收整型参数，返回新 fd 或 -1（ENOSYS 表示内核不支持）。
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as c_uint, 0 as c_uint) } as c_int;
    if pidfd >= 0 {
        let mut fds = [pollfd {
            fd: pidfd,
            events: POLLIN,
            revents: 0,
        }];
        // SAFETY: fds 是栈上有效 pollfd 数组，-1 表示无限等待内核事件。
        let ready = unsafe { poll(fds.as_mut_ptr(), 1, -1) };
        // SAFETY: pidfd 是本函数创建的合法 fd。
        unsafe { libc::close(pidfd) };
        if ready >= 0 {
            return;
        }
        // poll 失败（罕见）落到 /proc 回退路径。
    }
    let proc_path = format!("/proc/{pid}");
    let path = Path::new(&proc_path);
    while path.exists() {
        std::thread::sleep(Duration::from_millis(PROC_FALLBACK_POLL_MS));
    }
}

/// 当前系统启动以来的时钟滴答（/proc/uptime，Android USER_HZ=100 与
/// /proc/<pid>/stat 的 starttime 同刻度）。
fn boot_ticks_now() -> u64 {
    let Ok(uptime) = std::fs::read_to_string("/proc/uptime") else {
        return 0;
    };
    let seconds = uptime
        .split_whitespace()
        .next()
        .and_then(|value| value.split_once('.'))
        .and_then(|(sec, centis)| {
            Some(sec.parse::<u64>().ok()? * 100 + centis.parse::<u64>().ok()?)
        });
    seconds.unwrap_or(0)
}

/// 进程启动时刻（时钟滴答）；/proc/<pid>/stat 第 22 字段，comm 含空格时需
/// 跳过最后一个 ')' 再解析。
fn proc_start_ticks(pid: c_int) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit_once(')')?.1;
    // rest 自第 3 字段（state）起：第 22 字段 starttime 对应下标 22 - 3 = 19。
    rest.split_whitespace().nth(19)?.parse::<u64>().ok()
}

/// 清理死亡 daemon 的未改名 fork 子进程：它们的 comm 仍是 srx_daemon 但启动
/// 时间早于死亡时刻（fork 发生在 daemon 存活期内），与死亡后由其它路径拉起的
/// 新 daemon（启动时间晚于死亡时刻）天然可分。
fn kill_dead_daemon_children(death_ticks: u64) {
    for pid in pids_with_name(DAEMON_PROC_NAME) {
        match proc_start_ticks(pid) {
            // 无法判定启动时间时不杀：宁可放过一个孤儿（退避重试会再清），
            // 也不冒杀掉其它路径刚拉起 daemon 的风险。
            Some(start_ticks) if start_ticks < death_ticks => {
                // SAFETY: kill 只接收整型参数。
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            _ => {}
        }
    }
}

/// 扫描 /proc 判断是否有死亡之后才启动的存活 daemon（comm 判定，与 shell 端
/// 存活检测同口径；早于死亡时刻的同名进程已由 kill_dead_daemon_children 清理）。
fn live_daemon_exists(death_ticks: u64) -> bool {
    pids_with_name(DAEMON_PROC_NAME).into_iter().any(|pid| {
        proc_start_ticks(pid)
            .map(|start_ticks| start_ticks > death_ticks)
            // 无启动时间可查时保守视为新 daemon（与旧行为一致）。
            .unwrap_or(true)
    })
}

/// 清理 daemon 遗留的 FUSE 子进程：它们继承实例锁 fd，不清会让新 daemon
/// 因 flock 冲突 already_running 静默退出（与旧 shell 看门狗同语义）。
fn kill_leftover_fuse_children() {
    for name in FUSE_CHILD_PROC_NAMES {
        for pid in pids_with_name(name) {
            // SAFETY: kill 只接收整型参数。
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

fn pids_with_name(name: &[u8]) -> Vec<c_int> {
    let mut result = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return result;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<c_int>() else {
            continue;
        };
        let Ok(comm) = std::fs::read(entry.path().join("comm")) else {
            continue;
        };
        // /proc/<pid>/comm 以换行结尾，手动截掉尾部换行与 NUL（Vec 无 trim 方法）。
        let end = comm
            .iter()
            .rposition(|&b| b != b'\n' && b != 0)
            .map_or(0, |index| index + 1);
        if &comm[..end] == name {
            result.push(pid);
        }
    }
    result
}
