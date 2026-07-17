use libc::{
    AF_UNIX, POLLIN, SO_RCVBUF, SOCK_CLOEXEC, SOCK_DGRAM, SOL_SOCKET, bind, c_void, close, poll,
    pollfd, recv, sendto, setsockopt, sockaddr, sockaddr_un, socket,
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
            // SAFETY: fd is still owned locally because the receiver thread was not started.
            unsafe { close(fd) };
            return Err(error);
        }
    };
    thread::Builder::new()
        .name("srx-log-writer".to_string())
        .spawn(move || run(fd, state))
        .map(|_| ())
        .map_err(|error| {
            // SAFETY: thread creation failed, so ownership of fd was not transferred.
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

    // SAFETY: socket takes no borrowed pointers and returns an owned descriptor on success.
    let fd = unsafe { socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = send_control_packet(fd, command);
    // SAFETY: fd is owned by this function and the synchronous send has completed.
    unsafe { close(fd) };
    result
}

fn send_control_packet(fd: i32, command: &str) -> io::Result<()> {
    let (addr, addr_len) = socket_addr()?;
    let packet = format!("I\t{TAG_CONTROL}\t{command}");
    // SAFETY: packet and addr remain alive for the call and their lengths match the buffers.
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
    let mut buffer = [0u8; RECV_BUFFER_SIZE];

    loop {
        let mut poll_fd = pollfd {
            fd,
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: poll_fd points to one initialized pollfd for the duration of the call.
        let ready = unsafe { poll(&mut poll_fd, 1, FLUSH_INTERVAL_MS) };
        if ready <= 0 {
            state.flush_pending();
            continue;
        }

        // SAFETY: buffer is writable for buffer.len() bytes and fd is owned by this thread.
        let size = unsafe { recv(fd, buffer.as_mut_ptr() as *mut c_void, buffer.len(), 0) };
        if size <= 0 {
            continue;
        }
        let Ok(packet) = std::str::from_utf8(&buffer[..size as usize]) else {
            continue;
        };
        state.handle(packet);
        state.flush_if_due();
    }
}

fn bind_log_socket() -> io::Result<i32> {
    // SAFETY: socket takes no borrowed pointers and returns an owned descriptor on success.
    let fd = unsafe { socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = bind_log_socket_fd(fd);
    if let Err(error) = result {
        // SAFETY: bind failed, so fd remains owned locally and has not been transferred.
        unsafe { close(fd) };
        return Err(error);
    }
    Ok(fd)
}

fn bind_log_socket_fd(fd: i32) -> io::Result<()> {
    let recv_buffer = SOCKET_RECV_BUFFER_BYTES;
    // SAFETY: recv_buffer is initialized and the option length exactly matches its type.
    unsafe {
        let _ = setsockopt(
            fd,
            SOL_SOCKET,
            SO_RCVBUF,
            &recv_buffer as *const _ as *const _,
            mem::size_of_val(&recv_buffer) as libc::socklen_t,
        );
    }

    let (addr, addr_len) = socket_addr()?;
    // SAFETY: addr is initialized and addr_len covers only its initialized address bytes.
    if unsafe { bind(fd, &addr as *const _ as *const sockaddr, addr_len) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn socket_addr() -> io::Result<(sockaddr_un, libc::socklen_t)> {
    // SAFETY: sockaddr_un is a plain C structure that permits zero initialization.
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

    fn handle(&mut self, packet: &str) {
        let Some((level, tag, message)) = parse_packet(packet) else {
            return;
        };
        match tag {
            TAG_FILE_MONITOR => self.monitor.append(message),
            TAG_STATS => self.add_stats(message),
            TAG_CONTROL => self.handle_control(message),
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
    writer: BufWriter<File>,
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
            writer: BufWriter::new(file),
        })
    }

    fn append(&mut self, line: &str) {
        if line.is_empty() || writeln!(self.writer, "{line}").is_err() {
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
        if self.pending_lines > 0 {
            if self.writer.flush().is_err() {
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
        let _ = self.writer.flush();
        let oldest = backup_path(&self.path, LOG_BACKUPS);
        let _ = fs::remove_file(oldest);
        for index in (2..=LOG_BACKUPS).rev() {
            let source = backup_path(&self.path, index - 1);
            if source.exists() {
                let _ = fs::rename(source, backup_path(&self.path, index));
            }
        }
        if fs::rename(&self.path, backup_path(&self.path, 1)).is_err() {
            self.persisted_bytes = fs::metadata(&self.path)
                .map(|metadata| metadata.len())
                .unwrap_or(self.persisted_bytes);
            return;
        }
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            self.writer = BufWriter::new(file);
            self.persisted_bytes = 0;
            set_log_permissions(&self.path);
        }
    }

    fn clear(&mut self) {
        let _ = self.writer.flush();
        if fs::write(&self.path, []).is_err() {
            return;
        }
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            self.writer = BufWriter::new(file);
            self.persisted_bytes = 0;
            self.pending_bytes = 0;
            self.pending_lines = 0;
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
    // SAFETY: now points to writable storage for one time_t value.
    let _ = unsafe { libc::time(&mut now) };
    // SAFETY: libc::tm is a plain C structure that permits zero initialization.
    let mut value: libc::tm = unsafe { mem::zeroed() };
    // SAFETY: now and value are valid pointers and localtime_r writes only one tm value.
    if unsafe { libc::localtime_r(&now, &mut value) }.is_null() {
        return "00/00 00:00:00".to_string();
    }
    let mut buffer = [0u8; 32];
    let format = b"%m/%d %H:%M:%S\0";
    // SAFETY: buffer, format, and value remain valid for the call with correct lengths.
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
