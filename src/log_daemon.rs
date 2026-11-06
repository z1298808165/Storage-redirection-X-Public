use libc::{
    AF_UNIX, CMSG_DATA, CMSG_FIRSTHDR, EAGAIN, EINTR, EWOULDBLOCK, MSG_DONTWAIT, POLLIN,
    SCM_CREDENTIALS, SO_PASSCRED, SO_RCVBUF, SOCK_CLOEXEC, SOCK_DGRAM, SOL_SOCKET, bind, c_void,
    close, cmsghdr, iovec, msghdr, poll, pollfd, recvmsg, sendto, setsockopt, sockaddr,
    sockaddr_un, socket, ucred,
};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const SOCKET_NAME: &[u8] = b"storage.redirect.x.logd";
const RUNNING_LOG: &str = crate::platform::module_paths::RUNNING_LOG_FILE;
const FILE_MONITOR_LOG: &str = crate::platform::module_paths::FILE_MONITOR_LOG_FILE;
/// 生效次数存放在模块目录之外的持久目录。
///
/// 模块管理器在开机时会先删除旧模块目录再换上待更新目录，且这一步发生在
/// 任何模块脚本之前。把累计计数放在模块目录内，会让「刷入后到重启之间」
/// 由旧 daemon 继续累加的次数随旧目录一起丢失。放在持久目录后，旧 daemon
/// 和新 daemon 读写同一个文件，升级不再影响计数。
const STATS_DIR: &str = crate::platform::module_paths::PERSISTENT_DATA_DIR;
const STATS_FILE: &str = crate::platform::module_paths::STATS_FILE;
const STATS_TEMP_FILE: &str = crate::platform::module_paths::STATS_TEMP_FILE;
const STATS_RESET_ACK_FILE: &str = crate::platform::module_paths::STATS_RESET_ACK_FILE;
const MONITOR_CLEAR_ACK_FILE: &str = crate::platform::module_paths::MONITOR_CLEAR_ACK_FILE;
const MAX_RUNNING_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MONITOR_BYTES: u64 = 1024 * 1024;
const LOG_BACKUPS: usize = 2;
const RECV_BUFFER_SIZE: usize = 16 * 1024;
const FLUSH_BATCH_LINES: usize = 64;
const FLUSH_INTERVAL_MS: i32 = 2_000;
/// 单轮 poll 之后最多连续排空的 datagram 数量，避免持续高压写入时
/// 接收循环长期不返回、迟迟不执行 flush 与统计落盘。
const MAX_DRAIN_PER_ROUND: usize = 512;
const SOCKET_RECV_BUFFER_BYTES: libc::c_int = 512 * 1024;
/// 单个发送方 uid 在一个限流窗口内允许写入的监控/统计记录条数。
///
/// 监控记录由被 hook 的应用进程和 FUSE daemon 发出，发送方是任意应用 uid，
/// 无法按 uid 做身份校验（对比 Control 命令只允许 root）。因此改为限流：
/// 抽象命名空间套接字没有文件权限门槛，任何能连上的进程都可写入，若不限流
/// 就能用几 MB 垃圾把 1 MiB 上限的真实监控记录整体挤出轮转窗口。
const MAX_RECORDS_PER_UID_WINDOW: u32 = 4096;
/// 限流窗口长度。窗口结束后计数清零。
const RATE_LIMIT_WINDOW_MS: i64 = 10_000;
/// 限流表最多跟踪的发送方数量，防止伪造 uid 时表本身无界增长。
const MAX_TRACKED_SENDERS: usize = 256;

const TAG_FILE_MONITOR: &str = "FileMonitorOp";
const TAG_STATS: &str = "Stats";
const TAG_CONTROL: &str = "Control";
/// MP 热重载完成回执（发送方为 MediaProvider 进程，消息仅承载 pid；request id
/// 由本侧从请求文件读取权威值）。
const TAG_HOT_RELOAD: &str = "HotReload";
/// 机器可校验的低频状态事件（发送方为 daemon 自身），无条件落盘 running.log。
const TAG_STATUS: &str = "Status";
/// MP hook 进程请求 daemon（root）代修属主的通道标签；与 src/logging.rs 的
/// 待代修路径队列上限：恶意/异常发送方在 per-uid 限流之外再受总量约束。
const CONTROL_CLEAR_MONITOR: &str = "clear-monitor";
const CONTROL_CLEAR_MONITOR_PREFIX: &str = "clear-monitor:";
const CONTROL_FLUSH_ALL: &str = "flush-all";
const CONTROL_RESET_STATS: &str = "reset-stats";
const CONTROL_RECONCILE_RUNNING: &str = "reconcile-running";
const CONTROL_REGISTER_POLICY_PREFIX: &str = "register-policy:";
const STATS_SCHEMA: &str = "2";

static RECONCILE_REQUEST: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static POLICY_REGISTER_REQUEST: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static DAEMON_WAKE_EVENTFD: AtomicI32 = AtomicI32::new(-1);

pub fn set_daemon_wake_eventfd(fd: i32) {
    DAEMON_WAKE_EVENTFD.store(fd, Ordering::Release);
}

fn wake_daemon() {
    let fd = DAEMON_WAKE_EVENTFD.load(Ordering::Acquire);
    if fd >= 0 {
        let value = 1u64.to_ne_bytes();
        // SAFETY: fd 是登记过的有效 eventfd；write 只写入 8 字节计数，即使并发
        // 唤醒叠加也只是把计数累加，不存在缓冲区越界。失败时保留既有轮询兜底，
        // 不需要处理返回值。
        unsafe {
            libc::write(fd, value.as_ptr() as *const libc::c_void, value.len());
        }
    }
}

fn reconcile_request_slot() -> &'static Mutex<Option<String>> {
    RECONCILE_REQUEST.get_or_init(|| Mutex::new(None))
}

/// companion 请求 daemon 为指定应用（`<pkg>:<pid>`）即时预登记宿主策略。
///
/// companion 与 daemon 没有直接控制通道，宿主策略登记只能由 daemon 执行；此前
/// companion 只能等周期 reconcile（最坏 3s+），期间应用处于未套规则的裸视图。
/// 该命令让 daemon 在下一轮主循环（≤1s）完成定点预登记。
fn policy_register_request_slot() -> &'static Mutex<Option<String>> {
    POLICY_REGISTER_REQUEST.get_or_init(|| Mutex::new(None))
}

pub fn take_policy_register_request() -> Option<String> {
    policy_register_request_slot()
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
}

pub fn start() -> io::Result<()> {
    let fd = bind_log_socket()?;
    let state = match LogState::new() {
        Ok(state) => state,
        Err(error) => {
            // SAFETY: 接收线程尚未启动，因此 fd 仍由当前代码持有。
            unsafe { close(fd) };
            return Err(error);
        }
    };
    thread::Builder::new()
        .name("srx-log-writer".to_string())
        .spawn(move || run(fd, state))
        .map(|_| ())
        .inspect_err(|_| {
            // SAFETY: 线程创建失败，因此 fd 的所有权尚未转移。
            unsafe { close(fd) };
        })
}

pub fn send_control(command: &str) -> io::Result<()> {
    if command.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing control command",
        ));
    }

    // SAFETY: socket 不接收借用指针，成功时返回自有描述符。
    let fd = unsafe { socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = send_control_packet(fd, command);
    // SAFETY: fd 由此函数持有，且同步发送已经完成。
    unsafe { close(fd) };
    result
}

pub fn take_reconcile_request() -> Option<String> {
    reconcile_request_slot()
        .lock()
        .ok()
        .and_then(|mut request| request.take())
}

fn send_control_packet(fd: i32, command: &str) -> io::Result<()> {
    let (addr, addr_len) = socket_addr()?;
    let packet = format!("I\t{TAG_CONTROL}\t{command}");
    // SAFETY: packet 和 addr 在调用期间保持有效，长度与缓冲区一致。
    let sent = unsafe {
        sendto(
            fd,
            packet.as_ptr() as *const c_void,
            packet.len(),
            0,
            &addr as *const _ as *const sockaddr,
            addr_len,
        )
    };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn run(fd: i32, mut state: LogState) {
    let mut payload = [0u8; RECV_BUFFER_SIZE];

    loop {
        let mut poll_fd = pollfd {
            fd,
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: poll_fd 在调用期间指向一个已初始化的 pollfd。
        let ready = unsafe { poll(&mut poll_fd, 1, FLUSH_INTERVAL_MS) };
        if ready <= 0 {
            state.flush_pending();
            continue;
        }

        // 一次 poll 之后把内核队列里的 datagram 连续排空，否则突发写入时每轮只取一条，
        // 队列会持续堆积并被内核直接丢弃。
        for _ in 0..MAX_DRAIN_PER_ROUND {
            let (size, sender_uid, sender_addr) = recv_with_credentials(fd, &mut payload, MSG_DONTWAIT);
            if size < 0 {
                let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if errno == EINTR {
                    continue;
                }
                if errno != EAGAIN && errno != EWOULDBLOCK {
                    log::warn!("log daemon recvmsg failed errno={}", errno);
                }
                break;
            }
            if size == 0 {
                continue;
            }
            let Ok(packet) = std::str::from_utf8(&payload[..size as usize]) else {
                continue;
            };
            // 身份询问包：就地问一次 /proc 并回包给发送方。必须在 state.handle 之前
            // 分流——它不带日志语义，混进日志会污染记录并触发无谓的限流计数。
            if packet.starts_with(crate::daemon_identity_query::QUERY_PREFIX) {
                reply_identity_query(fd, packet, sender_addr.as_ref());
                continue;
            }
            state.handle(packet, sender_uid);
        }
        state.flush_if_due();
    }
}

/// 回答一条身份询问包，回包给发送方地址。
///
/// 回包失败只影响那一次决策的身份解析精度（调用方按未解析回退），因此不重试、不记录
/// 日志噪音：询问发生在每次快照未命中的写入上，失败一条就记一条会把 running.log 冲掉。
fn reply_identity_query(
    fd: i32,
    packet: &str,
    sender: Option<&(libc::sockaddr_un, libc::socklen_t)>,
) {
    let Some((addr, addr_len)) = sender else {
        return;
    };
    let Some(reply) = crate::daemon_identity_query::answer_query(packet) else {
        return;
    };
    let bytes = reply.as_bytes();
    // SAFETY: reply 与 addr 在调用期间保持有效；addr_len 由 recvmsg 填回。
    unsafe {
        sendto(
            fd,
            bytes.as_ptr() as *const c_void,
            bytes.len(),
            0,
            addr as *const _ as *const sockaddr,
            *addr_len,
        );
    }
}

/// 接收一条 datagram：返回 (字节数, 发送方 uid, 发送方地址)。
///
/// 发送方 uid 取自 SCM_CREDENTIALS，仅在内核附加了合法 ucred 时为 Some。发送方地址用于
/// 「身份询问」的回包：日志包只是单向 sink，而询问包需要就地答复。`flags` 直接传给
/// recvmsg；排空循环使用 MSG_DONTWAIT 避免在阻塞 socket 上卡死。
type ReceivedDatagram = (isize, Option<u32>, Option<(libc::sockaddr_un, libc::socklen_t)>);

fn recv_with_credentials(fd: i32, payload: &mut [u8], flags: libc::c_int) -> ReceivedDatagram {
    // 控制消息缓冲区，用于接收 SCM_CREDENTIALS（ucred）凭据。
    // 64 字节超过 CMSG_SPACE(sizeof(ucred)) 所需的约 32 字节，留有余量。
    // repr(align(8)) 保证 cmsghdr 所需的对齐要求（64 位系统 sizeof(size_t) = 8）。
    #[repr(align(8))]
    struct CmsgBuf([u8; 64]);
    let mut cmsg_buf = CmsgBuf([0u8; 64]);

    let mut iov = iovec {
        iov_base: payload.as_mut_ptr() as *mut c_void,
        iov_len: payload.len(),
    };
    // SAFETY: msghdr 是允许零初始化的普通 C 结构体，各字段在下方逐一赋值。
    let mut msg: msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.0.as_mut_ptr() as *mut c_void;
    msg.msg_controllen = cmsg_buf.0.len() as _;
    // 来源地址：询问包需要按发送方地址回包。
    // SAFETY: sockaddr_un 是允许零初始化的普通 C 结构体。
    let mut peer: libc::sockaddr_un = unsafe { mem::zeroed() };
    msg.msg_name = &mut peer as *mut _ as *mut c_void;
    msg.msg_namelen = mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;

    // SAFETY: msg 已完整初始化，payload 有 payload.len() 字节可写空间，fd 由调用方持有。
    let size = unsafe { recvmsg(fd, &mut msg, flags) };
    if size <= 0 {
        return (size, None, None);
    }
    let peer_addr = if msg.msg_namelen > 0 {
        Some((peer, msg.msg_namelen))
    } else {
        None
    };

    // 提取发送方 UID：查找 SOL_SOCKET / SCM_CREDENTIALS 控制消息。
    // SAFETY: CMSG_FIRSTHDR 在 msg_controllen > 0 时返回指向已初始化控制缓冲区内的指针，
    //         或在无控制消息时返回 NULL；对 NULL 的检查在下一行完成。
    let cmsg_ptr = unsafe { CMSG_FIRSTHDR(&msg) };
    if cmsg_ptr.is_null() {
        return (size, None, peer_addr);
    }
    // SAFETY: cmsg_ptr 非空且由 CMSG_FIRSTHDR 返回，指向已由内核初始化的 cmsghdr。
    let hdr = unsafe { &*cmsg_ptr };
    if hdr.cmsg_level != SOL_SOCKET || hdr.cmsg_type != SCM_CREDENTIALS {
        return (size, None, peer_addr);
    }
    // 验证 cmsg_len 足够容纳完整的 ucred，防止越界读取截断的控制消息。
    let min_len = mem::size_of::<cmsghdr>() + mem::size_of::<ucred>();
    if (hdr.cmsg_len as usize) < min_len {
        return (size, None, peer_addr);
    }
    // SAFETY: 已确认 cmsg_level/cmsg_type 正确且 cmsg_len 涵盖完整 ucred，
    //         CMSG_DATA 返回紧跟 cmsghdr 之后、由内核写入的 ucred 起始地址，
    //         该地址在 cmsg_buf 生命周期内始终有效。
    let creds = unsafe { &*(CMSG_DATA(cmsg_ptr) as *const ucred) };
    (size, Some(creds.uid as u32), peer_addr)
}

fn bind_log_socket() -> io::Result<i32> {
    // SAFETY: socket 不接收借用指针，成功时返回自有描述符。
    let fd = unsafe { socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = bind_log_socket_fd(fd);
    if let Err(error) = result {
        // SAFETY: bind 失败，因此 fd 仍由当前代码持有且尚未转移。
        unsafe { close(fd) };
        return Err(error);
    }
    Ok(fd)
}

fn bind_log_socket_fd(fd: i32) -> io::Result<()> {
    let recv_buffer = SOCKET_RECV_BUFFER_BYTES;
    // SAFETY: recv_buffer 已初始化，选项长度与其类型完全一致。
    unsafe {
        let _ = setsockopt(
            fd,
            SOL_SOCKET,
            SO_RCVBUF,
            &recv_buffer as *const _ as *const _,
            mem::size_of_val(&recv_buffer) as libc::socklen_t,
        );
    }

    // 启用 SO_PASSCRED：内核在 recvmsg 的辅助数据中附加发送方凭据（SCM_CREDENTIALS），
    // 从而可以校验发送方 UID，拒绝非可信来源的 Stats / Control 包。
    let passcred: libc::c_int = 1;
    // SAFETY: passcred 已初始化，选项长度与其类型完全一致。
    unsafe {
        let _ = setsockopt(
            fd,
            SOL_SOCKET,
            SO_PASSCRED,
            &passcred as *const _ as *const _,
            mem::size_of_val(&passcred) as libc::socklen_t,
        );
    }

    let (addr, addr_len) = socket_addr()?;
    // SAFETY: addr 已初始化，addr_len 仅覆盖已初始化的地址字节。
    if unsafe { bind(fd, &addr as *const _ as *const sockaddr, addr_len) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn socket_addr() -> io::Result<(sockaddr_un, libc::socklen_t)> {
    // SAFETY: sockaddr_un 是允许零初始化的普通 C 结构体。
    let mut addr: sockaddr_un = unsafe { mem::zeroed() };
    addr.sun_family = AF_UNIX as _;
    if SOCKET_NAME.len() + 1 > addr.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private log socket name too long",
        ));
    }
    addr.sun_path[0] = 0;
    for (index, byte) in SOCKET_NAME.iter().enumerate() {
        addr.sun_path[index + 1] = *byte as _;
    }
    Ok((
        addr,
        (mem::size_of::<libc::sa_family_t>() + SOCKET_NAME.len() + 1) as libc::socklen_t,
    ))
}

/// 单个发送方在当前限流窗口内的计数。
struct SenderQuota {
    window_started_ms: i64,
    count: u32,
    /// 本窗口内已丢弃的条数，窗口结束时汇总告警一次。
    dropped: u32,
}

struct LogState {
    running: RollingLog,
    monitor: RollingLog,
    runtime_activations: u64,
    stats_dirty: bool,
    last_flush: Instant,
    sender_quotas: HashMap<u32, SenderQuota>,
}

impl LogState {
    fn new() -> io::Result<Self> {
        Ok(Self {
            running: RollingLog::open(RUNNING_LOG, MAX_RUNNING_BYTES)?,
            monitor: RollingLog::open(FILE_MONITOR_LOG, MAX_MONITOR_BYTES)?,
            runtime_activations: read_runtime_activations(),
            stats_dirty: false,
            last_flush: Instant::now(),
            sender_quotas: HashMap::new(),
        })
    }

    /// 判断该发送方在当前窗口内是否还有配额。
    ///
    /// 凭据缺失（`None`）时不限流：内核在 `SO_PASSCRED` 下正常都会附带凭据，
    /// 缺失多为异常路径，限流会误伤模块自身记录。
    fn allow_from_sender(&mut self, sender_uid: Option<u32>) -> bool {
        let Some(uid) = sender_uid else {
            return true;
        };
        let now_ms = crate::platform::paths::monotonic_ms();

        // 表满时清理已过窗口的条目，避免伪造 uid 让表无界增长。
        if self.sender_quotas.len() >= MAX_TRACKED_SENDERS && !self.sender_quotas.contains_key(&uid)
        {
            self.sender_quotas.retain(|_, quota| {
                now_ms.saturating_sub(quota.window_started_ms) < RATE_LIMIT_WINDOW_MS
            });
            if self.sender_quotas.len() >= MAX_TRACKED_SENDERS {
                return false;
            }
        }

        let quota = self.sender_quotas.entry(uid).or_insert(SenderQuota {
            window_started_ms: now_ms,
            count: 0,
            dropped: 0,
        });

        if now_ms.saturating_sub(quota.window_started_ms) >= RATE_LIMIT_WINDOW_MS {
            let dropped = quota.dropped;
            quota.window_started_ms = now_ms;
            quota.count = 0;
            quota.dropped = 0;
            if dropped > 0 {
                self.running.append(&format_running_line(
                    "W",
                    "LogDaemon",
                    &format!("发送方 uid={} 超出记录限流，已丢弃 {} 条", uid, dropped),
                ));
            }
        }

        if quota.count >= MAX_RECORDS_PER_UID_WINDOW {
            quota.dropped = quota.dropped.saturating_add(1);
            return false;
        }
        quota.count = quota.count.saturating_add(1);
        true
    }

    fn handle(&mut self, packet: &str, sender_uid: Option<u32>) {
        let Some((level, tag, message)) = parse_packet(packet) else {
            return;
        };
        match tag {
            // 监控与统计的发送方是任意应用 uid，无法做身份校验，改为按发送方限流。
            TAG_FILE_MONITOR => {
                if self.allow_from_sender(sender_uid) {
                    // 先按 caller_pid 补强身份，再落盘与登记 hint：两者必须用同一份
                    // 内容，否则台账里的调用方会与记录里的身份不一致。
                    let enriched = enrich_monitor_record_identity(message);
                    let line = enriched.as_deref().unwrap_or(message);
                    self.monitor.append(line);
                    register_request_path_hint(line);
                }
            }
            TAG_STATS => {
                if self.allow_from_sender(sender_uid) {
                    self.add_stats(message);
                }
            }
            TAG_CONTROL => {
                // Control 命令仅允许 root（uid 0）发送；其它进程的凭据缺失或非 root 时拒绝。
                if sender_uid == Some(0) {
                    self.handle_control(message);
                } else {
                    self.running.append(&format_running_line(
                        "W",
                        "LogDaemon",
                        "收到非 root 进程的 Control 命令，已忽略",
                    ));
                }
            }
            TAG_HOT_RELOAD => {
                self.handle_hot_reload(message);
            }
            TAG_STATUS => {
                self.running
                    .append(&format_running_line("I", TAG_STATUS, message));
            }
            _ => {
                // 残留发送方（如尚未重启的宿主进程、仍在运行的应用注入侧）在全局
                // verbose 关闭后仍可能按旧判定发送 Info/Debug/Verbose 级运行日志；
                // 在接收侧只保留 Warn/Error，其余按 daemon 当前配置二次过滤，避免
                // 单个陈旧 emitter 把逐操作日志继续写到下一次宿主换代。监控、统计、
                // 控制、热重载与状态通道有独立分支，不受此门禁影响。
                if !matches!(level, "W" | "E") && !crate::logging::is_debug_logging_enabled() {
                    return;
                }
                self.running
                    .append(&format_running_line(level, tag, message));
            }
        }
    }

    fn add_stats(&mut self, message: &str) {
        let Some(delta) = message.strip_prefix('+') else {
            return;
        };
        let Ok(delta) = delta.trim().parse::<u64>() else {
            return;
        };
        if delta == 0 {
            return;
        }
        self.runtime_activations = self.runtime_activations.saturating_add(delta);
        self.stats_dirty = true;
    }

    fn handle_control(&mut self, command: &str) {
        match command {
            CONTROL_CLEAR_MONITOR => self.clear_monitor(None),
            command if command.starts_with(CONTROL_CLEAR_MONITOR_PREFIX) => {
                let token = command.trim_start_matches(CONTROL_CLEAR_MONITOR_PREFIX);
                if !token.is_empty() {
                    self.clear_monitor(Some(token));
                }
            }
            CONTROL_FLUSH_ALL => self.flush_pending(),
            CONTROL_RESET_STATS => self.reset_stats(),
            CONTROL_RECONCILE_RUNNING => {
                if let Ok(mut request) = reconcile_request_slot().lock() {
                    // quality-allow(chinese-language): legacy 是控制协议的固定请求标识，供旧调用方保留完成日志语义。
                    *request = Some("legacy".to_string());
                    wake_daemon();
                }
            }
            command if command.starts_with("reconcile-running:") => {
                let token = command.trim_start_matches("reconcile-running:");
                if !token.is_empty()
                    && let Ok(mut request) = reconcile_request_slot().lock()
                {
                    // quality-allow(chinese-language): token 来自控制协议，作为机器匹配字段必须保持原值。
                    *request = Some(token.to_string());
                    wake_daemon();
                }
            }
            command if command.starts_with(CONTROL_REGISTER_POLICY_PREFIX) => {
                let token = command
                    .trim_start_matches(CONTROL_REGISTER_POLICY_PREFIX)
                    .to_string();
                if !token.is_empty()
                    && let Ok(mut slot) = policy_register_request_slot().lock()
                {
                    *slot = Some(token);
                    // 必须唤醒主循环：应用刚启动、companion 的宿主视图等待预算只有秒级，
                    // 若只入队不唤醒，token 要等下一个无关事件（配置写入/宿主事件/周期
                    // reconcile 唤醒点）才被取走，真机实测可积压 13-28s——期间应用要么
                    // 被测试流杀掉（token 永远落在 process gone），要么在裸屏障上撞出
                    // ENOENT。与上方 reconcile-running 分支的唤醒语义保持一致。
                    wake_daemon();
                }
            }
            _ => {}
        }
    }

    /// 处理 MP 热重载完成回执：无条件落盘完成行，并代写 ack 回执文件。
    ///
    /// MP 进程（mediaprovider 域）对模块目录（shell_data_file）无读写权限——
    /// 请求文件读不到、ack 写不出——因此 request id 由本侧以 root 身份从请求
    /// 文件读取权威值，ack 亦由本侧落盘。完成行不受 verbose 门禁：它是低频的
    /// 一次性状态事件，且是 srxctl 等待循环（running.log 计数比对）的唯一判据。
    fn handle_hot_reload(&mut self, message: &str) {
        let pid = message
            .split_whitespace()
            .find_map(|item| item.strip_prefix("pid="))
            .unwrap_or("unknown");
        let request_id = fs::read_to_string(
            crate::platform::module_paths::MEDIA_PROVIDER_HOT_RELOAD_REQUEST_FILE,
        )
        .ok()
        .and_then(|text| {
            text.split_whitespace()
                .find_map(|item| item.strip_prefix("request=").map(str::to_string))
        })
        .unwrap_or_else(|| "signal".to_string());
        self.running.append(&format_running_line(
            "I",
            TAG_HOT_RELOAD,
            &format!(
                "media provider hot reload completed request={} pid={}",
                request_id, pid
            ),
        ));
        write_hot_reload_ack(pid, &request_id);
    }

    fn clear_monitor(&mut self, token: Option<&str>) {
        self.monitor.clear();
        self.monitor.flush();
        let Some(token) = token else {
            return;
        };
        let Some(parent) = Path::new(MONITOR_CLEAR_ACK_FILE).parent() else {
            return;
        };
        let _ = fs::create_dir_all(parent);
        let ack_tmp = format!("{MONITOR_CLEAR_ACK_FILE}.tmp");
        if fs::write(&ack_tmp, format!("{token}\n")).is_ok() {
            let _ = fs::rename(ack_tmp, MONITOR_CLEAR_ACK_FILE);
        }
    }

    fn reset_stats(&mut self) {
        if persist_runtime_activations(0).is_ok() {
            self.runtime_activations = 0;
            self.stats_dirty = false;
            let _ = fs::write(STATS_RESET_ACK_FILE, b"ok\n");
        }
    }

    fn flush_pending(&mut self) {
        self.running.flush();
        self.monitor.flush();
        if self.stats_dirty && persist_runtime_activations(self.runtime_activations).is_ok() {
            self.stats_dirty = false;
        }
        self.last_flush = Instant::now();
    }

    fn flush_if_due(&mut self) {
        if self.last_flush.elapsed() >= Duration::from_millis(FLUSH_INTERVAL_MS as u64) {
            self.flush_pending();
        }
    }
}

/// 从台账行登记请求侧 path hint。
///
/// `.recent_path_caller_hint` 此前设计为由 MediaProvider 进程在记录台账时直写
/// （temp+rename），但 logs 目录为 root 属主且 SELinux 标签对 MediaProvider 域
/// 不可写，直写始终静默失败，daemon 监视侧
/// `has_recent_public_path_request` 的「请求已记录则抑制观察记录」去重因此从未
/// 生效。台账行本身经私有日志通道进入本进程（root），在此对请求侧写入类记录
/// 补登记 hint：MediaProvider 侧直写保留为兜底，跨进程去重以本路径为准。
///
/// 只处理写入类 op_filter（mkdir / provider_open 写形态 / 原生 open 写形态，
/// open 写形态映射到 provider_open 对应值登记），与请求侧 hint 原有登记范围
/// 一致；行内字段带 `|` 的路径会被 hint 校验拒绝，天然安全。登记失败（包名
/// 无效、路径非公共存储、caller_uid 非应用 uid 等）时静默跳过，不影响台账落盘。
/// 用记录里的 `caller_pid` 补强源头身份，返回改写后的整行（不需要改写时 None）。
///
/// 为什么必须在 daemon 侧做：`caller_pid` 来自内核 FUSE 请求头，是「哪个进程发起
/// 这次写入」最精确的证据；但 MediaProvider 域读不到 root / 特权 helper 的
/// `/proc/<pid>/cmdline`（SELinux 拒绝，实测 `proc_pkg=` 空、`proc_uid=-1`），
/// 于是「root 文件管理器 helper 往 /sdcard 写文件」这类记录在 MP 侧只能落成未知。
/// daemon 是 root：读得到，而且记录是经 socket 实时送来的，它与请求几乎同时，
/// 不需要依赖 pid 表快照（快照抓不住亚秒级 helper 进程）。
///
/// 改写范围刻意收窄：仅 `event_role=request`，且 identity 为 `-` 或非包名的
/// basename 时才替换；已有包名身份一律不动，避免二次污染。
/// 应用 uid 起始值（与 `daemon.rs` 的同名常量保持一致）。
const SYSTEM_APP_UID_START: i32 = 10_000;

/// 系统 uid → 包名缓存：(文件指纹, uid → 唯一包名/不可用)。
type SystemUidPackageCache = (u64, HashMap<i32, Option<String>>);

/// 系统 uid → 唯一包名。
///
/// 只回答「这个 uid 在系统里唯一对应哪个包」：同一 uid 对应多个包时（真机 uid 1000
/// 有 53 个包）返回 `None`。**绝不按排序挑一个** —— 猜错比留空更糟，用户会据此
/// 认定是某个具体应用干的。缓存按文件长度与 mtime 失效，避免每条记录都读盘。
fn system_uid_package(uid: i32) -> Option<String> {
    if !(0..SYSTEM_APP_UID_START).contains(&uid) {
        return None;
    }
    static CACHE: OnceLock<Mutex<SystemUidPackageCache>> = OnceLock::new();
    let path = crate::platform::module_paths::SYSTEM_WRITER_UIDS_FILE;
    let fingerprint = fs::metadata(path)
        .map(|meta| {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            meta.len() ^ mtime
        })
        .unwrap_or(0);
    let cache = CACHE.get_or_init(|| Mutex::new((0, HashMap::new())));
    let mut guard = cache.lock().unwrap_or_else(|err| err.into_inner());
    if guard.0 != fingerprint || guard.1.is_empty() {
        let mut map: HashMap<i32, Option<String>> = HashMap::new();
        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines() {
                let Some((package, value)) = line.rsplit_once(':') else {
                    continue;
                };
                let Ok(package_uid) = value.trim().parse::<i32>() else {
                    continue;
                };
                if !(0..SYSTEM_APP_UID_START).contains(&package_uid) {
                    continue;
                }
                let package = package.trim();
                if package.is_empty() {
                    continue;
                }
                match map.get(&package_uid) {
                    // 同一 uid 对应多个包：标记不可用，宁缺勿猜。
                    Some(existing) if existing.as_deref() != Some(package) => {
                        map.insert(package_uid, None);
                    }
                    Some(_) => {}
                    None => {
                        map.insert(package_uid, Some(package.to_string()));
                    }
                }
            }
        }
        guard.0 = fingerprint;
        guard.1 = map;
    }
    guard.1.get(&uid).cloned().flatten()
}

/// 记录身份补强：把「谁发起了这次写入」补进目标包名列与 `identify_*` 字段。
///
/// 两条证据来源，置信度不同：
/// 1. `caller_pid`：内核 FUSE 请求头里的发起线程 id。daemon 以 root 读 /proc 反查，
///    是 helper / 特权进程写盘时唯一稳定的证据 —— `high`。
/// 2. `caller_evidence=cross_thread` 加 `caller_uid`：MediaProvider 的 binder 线程与
///    FuseDaemon 处理线程不是同一个线程，Java 侧捕获的调用方只能以「同进程 + 1.5 秒
///    窗口」桥接过来（见 `hook::context::record_caller_uid`）。系统 uid 按 uid 表反查
///    包名（多包一律放弃），应用 uid 仍按 pid 反查 —— `medium`：它是时序桥接，
///    不像内核请求头那样权威。
///
/// 只补第 3 列为占位符（`-`、空、无点号）的记录：已经指出具体应用的行不再改写。
fn enrich_monitor_record_identity(message: &str) -> Option<String> {
    let fields: Vec<&str> = message.split('|').collect();
    if fields.len() < 5 {
        return None;
    }
    let mut event_role = "";
    let mut caller_pid = -1;
    let mut caller_uid = -1;
    let mut cross_thread = false;
    for field in fields.iter().skip(4) {
        if let Some(value) = field.strip_prefix("caller_pid=") {
            caller_pid = value.parse::<i32>().unwrap_or(-1);
        } else if let Some(value) = field.strip_prefix("caller_uid=") {
            caller_uid = value.parse::<i32>().unwrap_or(-1);
        } else if let Some(value) = field.strip_prefix("event_role=") {
            event_role = value;
        } else if *field == "caller_evidence=cross_thread" {
            cross_thread = true;
        }
    }
    if event_role != "request" {
        return None;
    }
    let identity = fields[2];
    let replaceable = identity == "-" || identity.is_empty() || !identity.contains('.');
    if !replaceable {
        return None;
    }

    let (package, method, reliability) =
        if cross_thread && (0..SYSTEM_APP_UID_START).contains(&caller_uid) {
            (system_uid_package(caller_uid)?, "caller_uid", "medium")
        } else if caller_pid > 0 {
            let reliability = if cross_thread { "medium" } else { "high" };
            if let Some(package) =
                crate::platform::proc_identity::resolve_package_from_pid(caller_pid)
            {
                (package, "caller_pid", reliability)
            } else {
                // 命令行里没有包名（应用 exec 的 shell / helper）：按内核进程组追溯发起
                // 应用。记录与决策共用这一条证据，用户在记录里看到的源头就是规则会匹配的源头。
                let uid = crate::platform::proc_identity::read_proc_uid(caller_pid)
                    .filter(|uid| caller_uid < 0 || *uid == caller_uid)?;
                let package = crate::platform::proc_identity::resolve_launching_app_by_process_group(
                    caller_pid, uid,
                )?;
                (package, "process_group", reliability)
            }
        } else {
            return None;
        };
    if package == identity {
        return None;
    }

    let mut rebuilt = String::with_capacity(message.len() + package.len() + 24);
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            rebuilt.push('|');
        }
        match index {
            2 => rebuilt.push_str(&package),
            _ => {
                if field.starts_with("identify_method=") {
                    rebuilt.push_str("identify_method=");
                    rebuilt.push_str(method);
                } else if field.starts_with("identify_reliability=") {
                    rebuilt.push_str("identify_reliability=");
                    rebuilt.push_str(reliability);
                } else {
                    rebuilt.push_str(field);
                }
            }
        }
    }
    if !identity.is_empty() && identity != "-" {
        rebuilt.push_str("|native=");
        rebuilt.push_str(identity);
    }
    Some(rebuilt)
}

fn register_request_path_hint(message: &str) {
    if !message.contains("event_role=request") {
        return;
    }
    // 行格式：时间戳|进程包名|目标包名|OP|路径|ret=..|k=v...；额外字段从下标 5 起。
    let fields: Vec<&str> = message.split('|').collect();
    if fields.len() < 6 {
        return;
    }
    let Some(op_filter) = fields
        .iter()
        .skip(5)
        .find_map(|field| field.strip_prefix("op_filter="))
    else {
        return;
    };
    let (source, hint_op_filter) = match op_filter {
        "mkdir" => ("provider_mkdir", "mkdir"),
        "provider_open" | "provider_open:create" | "provider_open:write" => {
            ("provider_open", op_filter)
        }
        // 原生 hook 的写入类 open（映射路径等场景）：请求行本就是台账对这次
        // 写入的记录，映射到 provider_open 写形态登记 hint，让纯映射路径的
        // request/观察记录也走同一套 10 秒窗口去重。
        "open:create" => ("provider_open", "provider_open:create"),
        "open:write" => ("provider_open", "provider_open:write"),
        _ => return,
    };
    let caller_uid = fields
        .iter()
        .skip(5)
        .find_map(|field| field.strip_prefix("caller_uid="))
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(-1);
    let correlation_id = fields
        .iter()
        .skip(5)
        .find_map(|field| field.strip_prefix("correlation_id="))
        .unwrap_or("");
    crate::monitor::remember_public_path_caller_hint_with_op_filter(
        fields[4],
        fields[2],
        caller_uid,
        source,
        "high",
        hint_op_filter,
        correlation_id,
    );
}

struct RollingLog {
    path: PathBuf,
    max_bytes: u64,
    persisted_bytes: u64,
    pending_bytes: u64,
    pending_lines: usize,
    /// 轮转或清空过程中旧文件已经失效、而新文件尚未成功打开时置空，
    /// 避免继续向已被重命名或删除的文件写入。
    writer: Option<BufWriter<File>>,
}

impl RollingLog {
    fn open(path: impl Into<PathBuf>, max_bytes: u64) -> io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        set_log_permissions(&path);
        Ok(Self {
            persisted_bytes: file.metadata().map(|metadata| metadata.len()).unwrap_or(0),
            path,
            max_bytes,
            pending_bytes: 0,
            pending_lines: 0,
            writer: Some(BufWriter::new(file)),
        })
    }

    /// 返回可写入的 writer；若上一次轮转或清空后尚未打开新文件，则在此重新打开。
    fn writer_mut(&mut self) -> Option<&mut BufWriter<File>> {
        if self.writer.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok()?;
            set_log_permissions(&self.path);
            self.persisted_bytes = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            self.pending_bytes = 0;
            self.pending_lines = 0;
            self.writer = Some(BufWriter::new(file));
        }
        self.writer.as_mut()
    }

    fn append(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        let Some(writer) = self.writer_mut() else {
            return;
        };
        if writeln!(writer, "{line}").is_err() {
            return;
        }
        self.pending_bytes = self
            .pending_bytes
            .saturating_add(line.len() as u64)
            .saturating_add(1);
        self.pending_lines += 1;
        if self.pending_lines >= FLUSH_BATCH_LINES {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let Some(writer) = self.writer.as_mut() else {
            // 上一次轮转或清空后尚未打开新文件，等到下一次写入时再重新打开。
            self.pending_bytes = 0;
            self.pending_lines = 0;
            return;
        };
        if self.pending_lines > 0 {
            if writer.flush().is_err() {
                return;
            }
            self.persisted_bytes = self.persisted_bytes.saturating_add(self.pending_bytes);
            self.pending_bytes = 0;
            self.pending_lines = 0;
        }
        self.persisted_bytes = fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or(self.persisted_bytes);
        if self.persisted_bytes > self.max_bytes {
            self.rotate();
        }
    }

    fn rotate(&mut self) {
        if let Some(writer) = self.writer.as_mut() {
            let _ = writer.flush();
        }
        // 先释放旧句柄，避免重命名成功后继续向已轮转的文件追加日志。
        self.writer = None;
        self.pending_bytes = 0;
        self.pending_lines = 0;
        let oldest = backup_path(&self.path, LOG_BACKUPS);
        let _ = fs::remove_file(oldest);
        for index in (2..=LOG_BACKUPS).rev() {
            let source = backup_path(&self.path, index - 1);
            if source.exists() {
                let _ = fs::rename(source, backup_path(&self.path, index));
            }
        }
        if fs::rename(&self.path, backup_path(&self.path, 1)).is_err() {
            // 重命名失败说明原文件仍然可用，重新打开继续写入。
            self.persisted_bytes = fs::metadata(&self.path)
                .map(|metadata| metadata.len())
                .unwrap_or(self.persisted_bytes);
            let _ = self.writer_mut();
            return;
        }
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            self.writer = Some(BufWriter::new(file));
            self.persisted_bytes = 0;
            set_log_permissions(&self.path);
        }
    }

    fn clear(&mut self) {
        if let Some(writer) = self.writer.as_mut() {
            let _ = writer.flush();
        }
        // 截断前先释放句柄，避免 append 模式下的旧偏移把日志写回被清空的文件尾部。
        self.writer = None;
        self.pending_bytes = 0;
        self.pending_lines = 0;
        if fs::write(&self.path, []).is_err() {
            let _ = self.writer_mut();
            return;
        }
        remove_numeric_backups(&self.path);
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            self.writer = Some(BufWriter::new(file));
            self.persisted_bytes = 0;
            set_log_permissions(&self.path);
        }
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), index))
}

fn remove_numeric_backups(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    let Some(base_name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let prefix = format!("{base_name}.");
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(suffix) = name.strip_prefix(&prefix) else {
            continue;
        };
        if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let _ = fs::remove_file(entry.path());
    }
}

fn set_log_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o666));
}

fn parse_packet(packet: &str) -> Option<(&str, &str, &str)> {
    let mut parts = packet.splitn(3, '\t');
    let level = parts.next()?;
    let tag = parts.next()?;
    let message = parts.next()?;
    (!level.is_empty() && !tag.is_empty() && !message.is_empty()).then_some((level, tag, message))
}

/// 代 MP 写热重载 ack 回执（MP 域对模块目录无写权限，必须由 root 侧落盘）。
/// 原子写（tmp+rename）保持与旧 MP 侧实现相同的 torn-write 语义。
fn write_hot_reload_ack(pid: &str, request_id: &str) {
    let path = Path::new(crate::platform::module_paths::MEDIA_PROVIDER_HOT_RELOAD_ACK_FILE);
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let boot_id = crate::platform::read_boot_id();
    let content = format!(
        "stage=hot_reload_ok request={} pid={} boot_id={}\n",
        request_id,
        pid,
        if boot_id.is_empty() {
            "unknown"
        } else {
            &boot_id
        }
    );
    let temp = path.with_extension("ack.tmp");
    if fs::write(&temp, content).is_err() {
        return;
    }
    if fs::rename(&temp, path).is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o644));
    }
}

fn format_running_line(level: &str, tag: &str, message: &str) -> String {
    let timestamp = timestamp_text();
    if message.starts_with("[Rs") || message.starts_with("[Kt") || message.starts_with("[Jv") {
        return format!("{timestamp} {message}");
    }
    let source = if tag == "SRX" { "Jv" } else { "Rs" };
    format!("{timestamp} [{source}{}] {message}", level_text(level))
}

fn level_text(level: &str) -> &'static str {
    match level.as_bytes().first().copied() {
        Some(b'V') => "Verbose",
        Some(b'D') => "Debug",
        Some(b'I') => "Info",
        Some(b'W') => "Warn",
        Some(b'E') => "Error",
        _ => "Info",
    }
}

fn timestamp_text() -> String {
    let mut now: libc::time_t = 0;
    // SAFETY: now 指向可写入一个 time_t 值的存储空间。
    let _ = unsafe { libc::time(&mut now) };
    // SAFETY: libc::tm 是允许零初始化的普通 C 结构体。
    let mut value: libc::tm = unsafe { mem::zeroed() };
    // SAFETY: now 和 value 均为有效指针，localtime_r 只写入一个 tm 值。
    if unsafe { libc::localtime_r(&now, &mut value) }.is_null() {
        return "00/00 00:00:00".to_string();
    }
    let mut buffer = [0u8; 32];
    let format = b"%m/%d %H:%M:%S\0";
    // SAFETY: buffer、format 和 value 在调用期间保持有效，且长度正确。
    let written = unsafe {
        libc::strftime(
            buffer.as_mut_ptr() as *mut _,
            buffer.len(),
            format.as_ptr() as *const _,
            &value,
        )
    };
    if written == 0 {
        return "00/00 00:00:00".to_string();
    }
    String::from_utf8_lossy(&buffer[..written]).into_owned()
}

/// 启动时读取已累计的生效次数。
///
/// 文件不存在（首次安装）或读取失败时均返回 0，daemon 从 0 重新累加。
/// `schema` 与当前版本不一致时同样返回 0：schema 只在统计口径变化时提升，
/// 旧值与新口径不可比，丢弃是有意行为。
fn read_runtime_activations() -> u64 {
    let Ok(text) = fs::read_to_string(STATS_FILE) else {
        return 0;
    };
    let mut schema = None;
    let mut runtime_activations = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "schema" => schema = Some(value.trim()),
            "runtime_activations" => runtime_activations = value.trim().parse::<u64>().ok(),
            _ => {}
        }
    }
    if schema == Some(STATS_SCHEMA) {
        runtime_activations.unwrap_or(0)
    } else {
        0
    }
}

fn format_stats(runtime_activations: u64) -> String {
    format!("schema={STATS_SCHEMA}\nruntime_activations={runtime_activations}\n")
}

fn persist_runtime_activations(runtime_activations: u64) -> io::Result<()> {
    // 目录可能尚不存在（首次安装或迁移前的首次启动），确保先创建。
    fs::create_dir_all(STATS_DIR)?;
    let mut file = File::create(STATS_TEMP_FILE)?;
    file.write_all(format_stats(runtime_activations).as_bytes())?;
    file.sync_all()?;
    fs::rename(STATS_TEMP_FILE, STATS_FILE)
}
