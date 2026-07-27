use libc::{
    AF_UNIX, CMSG_DATA, CMSG_FIRSTHDR, EAGAIN, EINTR, EWOULDBLOCK, MSG_DONTWAIT, POLLIN,
    SCM_CREDENTIALS, SO_PASSCRED, SO_RCVBUF, SOCK_CLOEXEC, SOCK_DGRAM, SOL_SOCKET, bind, c_void,
    close, cmsghdr, iovec, msghdr, poll, pollfd, recvmsg, sendto, setsockopt, sockaddr,
    sockaddr_un, socket, ucred,
};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const SOCKET_NAME: &[u8] = b"storage.redirect.x.logd";
const RUNNING_LOG: &str = "/data/adb/modules/storage.redirect.x/logs/running.log";
const FILE_MONITOR_LOG: &str = "/data/adb/modules/storage.redirect.x/logs/file_monitor.log";
const STATS_FILE: &str = "/data/adb/modules/storage.redirect.x/stats";
const STATS_TEMP_FILE: &str = "/data/adb/modules/storage.redirect.x/.stats.tmp";
const STATS_RESET_ACK_FILE: &str = "/data/adb/modules/storage.redirect.x/.stats.reset.ok";
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

const TAG_FILE_MONITOR: &str = "FileMonitorOp";
const TAG_STATS: &str = "Stats";
const TAG_CONTROL: &str = "Control";
const CONTROL_CLEAR_MONITOR: &str = "clear-monitor";
const CONTROL_FLUSH_ALL: &str = "flush-all";
const CONTROL_RESET_STATS: &str = "reset-stats";
const STATS_SCHEMA: &str = "2";

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
        .map_err(|error| {
            // SAFETY: 线程创建失败，因此 fd 的所有权尚未转移。
            unsafe { close(fd) };
            error
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
            let (size, sender_uid) = recv_with_credentials(fd, &mut payload, MSG_DONTWAIT);
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
            state.handle(packet, sender_uid);
        }
        state.flush_if_due();
    }
}

/// 接收一条 datagram，同时提取 SCM_CREDENTIALS 中的发送方 UID。
/// 返回 (接收字节数, 发送方 uid)；uid 仅在内核附加了合法 ucred 时为 Some。
/// `flags` 直接传给 recvmsg；排空循环使用 MSG_DONTWAIT 避免在阻塞 socket 上卡死。
fn recv_with_credentials(fd: i32, payload: &mut [u8], flags: libc::c_int) -> (isize, Option<u32>) {
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

    // SAFETY: msg 已完整初始化，payload 有 payload.len() 字节可写空间，fd 由调用方持有。
    let size = unsafe { recvmsg(fd, &mut msg, flags) };
    if size <= 0 {
        return (size, None);
    }

    // 提取发送方 UID：查找 SOL_SOCKET / SCM_CREDENTIALS 控制消息。
    // SAFETY: CMSG_FIRSTHDR 在 msg_controllen > 0 时返回指向已初始化控制缓冲区内的指针，
    //         或在无控制消息时返回 NULL；对 NULL 的检查在下一行完成。
    let cmsg_ptr = unsafe { CMSG_FIRSTHDR(&msg) };
    if cmsg_ptr.is_null() {
        return (size, None);
    }
    // SAFETY: cmsg_ptr 非空且由 CMSG_FIRSTHDR 返回，指向已由内核初始化的 cmsghdr。
    let hdr = unsafe { &*cmsg_ptr };
    if hdr.cmsg_level != SOL_SOCKET || hdr.cmsg_type != SCM_CREDENTIALS {
        return (size, None);
    }
    // 验证 cmsg_len 足够容纳完整的 ucred，防止越界读取截断的控制消息。
    let min_len = mem::size_of::<cmsghdr>() + mem::size_of::<ucred>();
    if (hdr.cmsg_len as usize) < min_len {
        return (size, None);
    }
    // SAFETY: 已确认 cmsg_level/cmsg_type 正确且 cmsg_len 涵盖完整 ucred，
    //         CMSG_DATA 返回紧跟 cmsghdr 之后、由内核写入的 ucred 起始地址，
    //         该地址在 cmsg_buf 生命周期内始终有效。
    let creds = unsafe { &*(CMSG_DATA(cmsg_ptr) as *const ucred) };
    (size, Some(creds.uid as u32))
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

struct LogState {
    running: RollingLog,
    monitor: RollingLog,
    runtime_activations: u64,
    stats_dirty: bool,
    last_flush: Instant,
}

impl LogState {
    fn new() -> io::Result<Self> {
        Ok(Self {
            running: RollingLog::open(RUNNING_LOG, MAX_RUNNING_BYTES)?,
            monitor: RollingLog::open(FILE_MONITOR_LOG, MAX_MONITOR_BYTES)?,
            runtime_activations: read_runtime_activations(),
            stats_dirty: false,
            last_flush: Instant::now(),
        })
    }

    fn handle(&mut self, packet: &str, sender_uid: Option<u32>) {
        let Some((level, tag, message)) = parse_packet(packet) else {
            return;
        };
        match tag {
            TAG_FILE_MONITOR => self.monitor.append(message),
            TAG_STATS => self.add_stats(message),
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
            _ => self
                .running
                .append(&format_running_line(level, tag, message)),
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
            CONTROL_CLEAR_MONITOR => self.monitor.clear(),
            CONTROL_FLUSH_ALL => self.flush_pending(),
            CONTROL_RESET_STATS => self.reset_stats(),
            _ => {}
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
    let mut file = File::create(STATS_TEMP_FILE)?;
    file.write_all(format_stats(runtime_activations).as_bytes())?;
    file.sync_all()?;
    fs::rename(STATS_TEMP_FILE, STATS_FILE)
}
