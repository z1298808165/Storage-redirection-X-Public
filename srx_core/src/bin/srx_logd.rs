use libc::{
    AF_UNIX, SO_RCVBUF, SOCK_CLOEXEC, SOCK_DGRAM, SOCK_NONBLOCK, SOL_SOCKET, bind, c_void, close,
    recv, sendto, setsockopt, sockaddr, sockaddr_un, socket,
};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::mem;
use std::path::{Path, PathBuf};

const SOCKET_NAME: &[u8] = b"storage.redirect.x.logd";
const LOGS_DIR: &str = "/data/adb/modules/storage.redirect.x/logs";
const RUNNING_LOG: &str = "/data/adb/modules/storage.redirect.x/logs/running.log";
const FILE_MONITOR_LOG: &str = "/data/adb/modules/storage.redirect.x/logs/file_monitor.log";
const MEDIA_STATE_LOG: &str = "/data/adb/modules/storage.redirect.x/logs/media_provider_state.log";
const APP_STATUS_LOG: &str = "/data/adb/modules/storage.redirect.x/logs/app_status.log";
const STATS_FILE: &str = "/data/adb/modules/storage.redirect.x/stats";
const MAX_RUNNING_LINES: usize = 30_000;
const MAX_MONITOR_LINES: usize = 30_000;
const MAX_MEDIA_STATE_LINES: usize = 30_000;
const MAX_APP_STATUS_LINES: usize = 30_000;
const TRIM_BATCH_LINES: usize = 200;
const RECV_BUFFER_SIZE: usize = 8192;
const FLUSH_BATCH_LINES: usize = 64;
const SOCKET_RECV_BUFFER_BYTES: libc::c_int = 512 * 1024;

const TAG_FILE_MONITOR: &str = "FileMonitorOp";
const TAG_MEDIA_STATE: &str = "MediaState";
const TAG_APP_STATUS: &str = "AppStatus";
const TAG_MEDIA_STATE_FLUSH: &str = "MediaStateFlush";
const TAG_APP_STATUS_FLUSH: &str = "AppStatusFlush";
const TAG_STATS: &str = "Stats";
const TAG_CONTROL: &str = "Control";
const CONTROL_CLEAR_ALL: &str = "clear-all";
const CONTROL_FLUSH_ALL: &str = "flush-all";

fn main() {
    let command = std::env::args().nth(1);
    let arg = std::env::args().nth(2);
    let result = match command.as_deref() {
        None => run_daemon(),
        Some("emit-stream") => run_emit_stream(arg),
        Some("control") => run_control(arg),
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: srx_logd [emit-stream <tag> | control <command>]",
        )),
    };
    if let Err(error) = result {
        let _ = writeln!(io::stderr(), "srx_logd: {error}");
    }
}

fn run_daemon() -> io::Result<()> {
    fs::create_dir_all(LOGS_DIR)?;
    let fd = bind_log_socket()?;
    let mut state = LogState::new()?;
    let mut buffer = [0u8; RECV_BUFFER_SIZE];

    loop {
        let size = unsafe { recv(fd, buffer.as_mut_ptr() as *mut c_void, buffer.len(), 0) };
        if size <= 0 {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&buffer[..size as usize]) else {
            continue;
        };
        state.handle(text);
    }
}

fn run_emit_stream(tag: Option<String>) -> io::Result<()> {
    let Some(tag) = tag.filter(|value| !value.is_empty()) else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "missing tag"));
    };
    let Some(socket) = ClientSocket::new() else {
        return Err(io::Error::last_os_error());
    };

    let stdin = io::stdin();
    let reader = BufReader::new(stdin.lock());
    for line in reader.lines() {
        let line = line?;
        socket.send('I', &tag, &line)?;
    }
    if let Some(flush_tag) = flush_tag_for(&tag) {
        socket.send('I', flush_tag, ".")?;
    }
    Ok(())
}

fn run_control(command: Option<String>) -> io::Result<()> {
    let Some(command) = command.filter(|value| !value.is_empty()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing control command",
        ));
    };
    let Some(socket) = ClientSocket::new() else {
        return Err(io::Error::last_os_error());
    };
    socket.send('I', TAG_CONTROL, &command)
}

fn bind_log_socket() -> io::Result<i32> {
    let fd = unsafe { socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    configure_socket_recv_buffer(fd);

    let mut addr: sockaddr_un = unsafe { mem::zeroed() };
    addr.sun_family = AF_UNIX as _;
    if SOCKET_NAME.len() + 1 > addr.sun_path.len() {
        unsafe {
            close(fd);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket name too long",
        ));
    }
    addr.sun_path[0] = 0;
    for (index, byte) in SOCKET_NAME.iter().enumerate() {
        addr.sun_path[index + 1] = *byte as _;
    }

    let len = sockaddr_un_len(SOCKET_NAME.len() + 1);
    let ret = unsafe { bind(fd, &addr as *const _ as *const sockaddr, len) };
    if ret != 0 {
        let error = io::Error::last_os_error();
        unsafe {
            close(fd);
        }
        return Err(error);
    }
    Ok(fd)
}

fn configure_socket_recv_buffer(fd: i32) {
    let recv_buffer = SOCKET_RECV_BUFFER_BYTES;
    unsafe {
        let _ = setsockopt(
            fd,
            SOL_SOCKET,
            SO_RCVBUF,
            &recv_buffer as *const _ as *const _,
            mem::size_of_val(&recv_buffer) as libc::socklen_t,
        );
    }
}

fn sockaddr_un_len(path_len: usize) -> libc::socklen_t {
    (mem::size_of::<libc::sa_family_t>() + path_len) as libc::socklen_t
}

struct ClientSocket {
    fd: i32,
    addr: sockaddr_un,
    addr_len: libc::socklen_t,
}

impl ClientSocket {
    fn new() -> Option<Self> {
        let fd = unsafe { socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0) };
        if fd < 0 {
            return None;
        }

        let mut addr: sockaddr_un = unsafe { mem::zeroed() };
        addr.sun_family = AF_UNIX as _;
        if SOCKET_NAME.len() + 1 > addr.sun_path.len() {
            unsafe {
                close(fd);
            }
            return None;
        }
        addr.sun_path[0] = 0;
        for (index, byte) in SOCKET_NAME.iter().enumerate() {
            addr.sun_path[index + 1] = *byte as _;
        }

        Some(Self {
            fd,
            addr,
            addr_len: sockaddr_un_len(SOCKET_NAME.len() + 1),
        })
    }

    fn send(&self, level: char, tag: &str, message: &str) -> io::Result<()> {
        let message = sanitize_transport_field(message);
        if tag.is_empty() || message.is_empty() {
            return Ok(());
        }

        let packet = format!("{level}\t{tag}\t{message}");
        let ret = unsafe {
            sendto(
                self.fd,
                packet.as_ptr() as *const c_void,
                packet.len(),
                0,
                &self.addr as *const _ as *const sockaddr,
                self.addr_len,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for ClientSocket {
    fn drop(&mut self) {
        unsafe {
            close(self.fd);
        }
    }
}

struct LogState {
    running: RollingLog,
    monitor: RollingLog,
    media_state: RollingLog,
    app_status: RollingLog,
    stats_total: i64,
    stats_dirty: usize,
}

impl LogState {
    fn new() -> io::Result<Self> {
        Ok(Self {
            running: RollingLog::open(RUNNING_LOG, MAX_RUNNING_LINES)?,
            monitor: RollingLog::open(FILE_MONITOR_LOG, MAX_MONITOR_LINES)?,
            media_state: RollingLog::open(MEDIA_STATE_LOG, MAX_MEDIA_STATE_LINES)?,
            app_status: RollingLog::open(APP_STATUS_LOG, MAX_APP_STATUS_LINES)?,
            stats_total: read_stats_total(),
            stats_dirty: 0,
        })
    }

    fn handle(&mut self, packet: &str) {
        let Some((level, tag, message)) = parse_packet(packet) else {
            return;
        };

        match tag {
            TAG_STATS => self.handle_stats(message),
            TAG_CONTROL => self.handle_control(message),
            TAG_FILE_MONITOR => self.monitor.append(message),
            TAG_MEDIA_STATE => self.media_state.append(message),
            TAG_APP_STATUS => self.app_status.append(message),
            TAG_MEDIA_STATE_FLUSH => self.media_state.flush(),
            TAG_APP_STATUS_FLUSH => self.app_status.flush(),
            _ => {
                let line = format_running_line(level, tag, message);
                self.running.append(&line);
            }
        }
    }

    fn handle_stats(&mut self, message: &str) {
        let Some(value) = message.strip_prefix('+') else {
            return;
        };
        let Ok(delta) = value.trim().parse::<i64>() else {
            return;
        };
        if delta <= 0 {
            return;
        }

        self.stats_total = self.stats_total.saturating_add(delta);
        self.stats_dirty += 1;
        if self.stats_dirty >= 50 {
            self.flush_stats();
        }
    }

    fn handle_control(&mut self, message: &str) {
        match message {
            CONTROL_CLEAR_ALL => self.clear_all(),
            CONTROL_FLUSH_ALL => self.flush_all(),
            _ => {}
        }
    }

    fn clear_all(&mut self) {
        self.running.clear();
        self.monitor.clear();
        self.media_state.clear();
        self.app_status.clear();
        self.stats_total = 0;
        self.stats_dirty = 0;
        let _ = fs::write(STATS_FILE, "0\n");
    }

    fn flush_all(&mut self) {
        self.running.flush();
        self.monitor.flush();
        self.media_state.flush();
        self.app_status.flush();
        if self.stats_dirty > 0 {
            self.flush_stats();
        }
    }

    fn flush_stats(&mut self) {
        let _ = fs::write(STATS_FILE, format!("{}\n", self.stats_total));
        self.stats_dirty = 0;
    }
}

impl Drop for LogState {
    fn drop(&mut self) {
        self.running.flush();
        self.monitor.flush();
        self.media_state.flush();
        self.app_status.flush();
        if self.stats_dirty > 0 {
            self.flush_stats();
        }
    }
}

struct RollingLog {
    path: PathBuf,
    max_lines: usize,
    line_count: usize,
    pending_flush: usize,
    writer: BufWriter<File>,
}

impl RollingLog {
    fn open(path: impl Into<PathBuf>, max_lines: usize) -> io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            line_count: count_lines(&path).unwrap_or(0),
            max_lines,
            path,
            pending_flush: 0,
            writer: BufWriter::new(file),
        })
    }

    fn append(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        if writeln!(self.writer, "{line}").is_err() {
            return;
        }

        self.line_count += 1;
        self.pending_flush += 1;
        if self.pending_flush >= FLUSH_BATCH_LINES {
            self.flush();
        }
        if self.line_count > self.max_lines {
            self.trim();
        }
    }

    fn flush(&mut self) {
        let _ = self.writer.flush();
        self.pending_flush = 0;
    }

    fn trim(&mut self) {
        self.flush();
        let drop_lines = self
            .line_count
            .saturating_sub(self.max_lines)
            .saturating_add(TRIM_BATCH_LINES);
        if drop_lines == 0 {
            return;
        }
        if trim_file_drop_head(&self.path, drop_lines).is_err() {
            return;
        }

        self.line_count = self.line_count.saturating_sub(drop_lines);
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            self.writer = BufWriter::new(file);
        }
    }

    fn clear(&mut self) {
        self.flush();
        if fs::write(&self.path, []).is_err() {
            return;
        }
        self.line_count = 0;
        self.pending_flush = 0;
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            self.writer = BufWriter::new(file);
        }
    }
}

fn parse_packet(packet: &str) -> Option<(&str, &str, &str)> {
    let mut parts = packet.splitn(3, '\t');
    let level = parts.next()?;
    let tag = parts.next()?;
    let message = parts.next()?;
    if level.is_empty() || tag.is_empty() || message.is_empty() {
        return None;
    }
    Some((level, tag, message))
}

fn format_running_line(level: &str, tag: &str, message: &str) -> String {
    let now = timestamp_text();
    if message.starts_with("[Rs") || message.starts_with("[Kt") {
        return format!("{now} {message}");
    }

    let source = if tag == "SRX" { "Jv" } else { "Rs" };
    format!("{now} [{source}{}] {message}", level_text(level))
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
    unsafe {
        libc::time(&mut now as *mut libc::time_t);
    }
    let mut tm_value: libc::tm = unsafe { mem::zeroed() };
    let tm_ptr = unsafe { libc::localtime_r(&now as *const libc::time_t, &mut tm_value) };
    if tm_ptr.is_null() {
        return "00/00 00:00:00".to_string();
    }

    let mut buffer = [0u8; 32];
    let fmt = b"%m/%d %H:%M:%S\0";
    let written = unsafe {
        libc::strftime(
            buffer.as_mut_ptr() as *mut _,
            buffer.len(),
            fmt.as_ptr() as *const _,
            &tm_value as *const _,
        )
    };
    if written == 0 {
        return "00/00 00:00:00".to_string();
    }
    String::from_utf8_lossy(&buffer[..written]).to_string()
}

fn sanitize_transport_field(message: &str) -> String {
    if !message.contains(['\n', '\r', '\t']) {
        return message.to_string();
    }

    message
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' => ' ',
            _ => ch,
        })
        .collect()
}

fn flush_tag_for(tag: &str) -> Option<&'static str> {
    match tag {
        TAG_MEDIA_STATE => Some(TAG_MEDIA_STATE_FLUSH),
        TAG_APP_STATUS => Some(TAG_APP_STATUS_FLUSH),
        _ => None,
    }
}

fn count_lines(path: &Path) -> io::Result<usize> {
    let data = fs::read(path)?;
    Ok(data.iter().filter(|byte| **byte == b'\n').count())
}

fn trim_file_drop_head(path: &Path, drop_lines: usize) -> io::Result<()> {
    if drop_lines == 0 {
        return Ok(());
    }

    let data = fs::read(path)?;
    let mut dropped = 0usize;
    let mut start = 0usize;
    while start < data.len() && dropped < drop_lines {
        if data[start] == b'\n' {
            dropped += 1;
        }
        start += 1;
    }

    let remaining = if dropped >= drop_lines {
        &data[start..]
    } else {
        &[]
    };
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, remaining)?;
    fs::rename(temp_path, path)?;
    Ok(())
}

fn read_stats_total() -> i64 {
    fs::read_to_string(STATS_FILE)
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .unwrap_or(0)
}
