use libc::{
    AF_UNIX, SOCK_CLOEXEC, SOCK_DGRAM, SOCK_NONBLOCK, c_char, c_int, c_void, close, sendto,
    sockaddr, sockaddr_un, socket,
};
use log::{Level as LogLevel, LevelFilter, Log, Metadata, Record};
use std::ffi::CString;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};

const LOG_LEVEL_VERBOSE: i32 = 0;
const LOG_LEVEL_DEBUG: i32 = 1;
const LOG_LEVEL_INFO: i32 = 2;
const LOG_LEVEL_WARN: i32 = 3;
const LOG_LEVEL_ERROR: i32 = 4;

const CURRENT_LOG_LEVEL: i32 = LOG_LEVEL_DEBUG;
const DEFAULT_LOG_TAG: &str = "StorageRedirect";
const FILE_MONITOR_LOG_TAG: &str = "FileMonitorOp";
const STATS_LOG_TAG: &str = "Stats";
const PRIVATE_LOG_SOCKET_NAME: &[u8] = b"storage.redirect.x.logd";

const ANDROID_LOG_VERBOSE: i32 = 2;
const ANDROID_LOG_DEBUG: i32 = 3;
const ANDROID_LOG_INFO: i32 = 4;
const ANDROID_LOG_WARN: i32 = 5;
const ANDROID_LOG_ERROR: i32 = 6;

#[repr(i32)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Level {
    Verbose = LOG_LEVEL_VERBOSE,
    Debug = LOG_LEVEL_DEBUG,
    Info = LOG_LEVEL_INFO,
    Warn = LOG_LEVEL_WARN,
    Error = LOG_LEVEL_ERROR,
}

static LOG_INIT: Once = Once::new();
static LOG_ADAPTER: LogAdapter = LogAdapter;
static DEBUG_LOGGING_ENABLED: AtomicBool = AtomicBool::new(false);
static PRIVATE_LOG_SOCKET: OnceLock<Option<PrivateLogSocket>> = OnceLock::new();

struct LogAdapter;

struct PrivateLogSocket {
    fd: c_int,
    addr: sockaddr_un,
    addr_len: libc::socklen_t,
}

pub struct Logger;

impl Logger {
    pub fn init(_package_name: Option<&str>) {
        ensure_log_adapter();
    }
}

pub fn set_debug_logging_enabled(enabled: bool) {
    DEBUG_LOGGING_ENABLED.store(enabled, Ordering::Relaxed);
}

impl PrivateLogSocket {
    fn new() -> Option<Self> {
        // SAFETY: socket takes no borrowed pointers and returns an owned descriptor on success.
        let fd = unsafe { socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0) };
        if fd < 0 {
            return None;
        }

        // SAFETY: sockaddr_un is a plain C structure that permits zero initialization.
        let mut addr: sockaddr_un = unsafe { mem::zeroed() };
        addr.sun_family = AF_UNIX as _;
        if PRIVATE_LOG_SOCKET_NAME.len() + 1 > addr.sun_path.len() {
            // SAFETY: fd is owned here and has not been closed or transferred.
            unsafe { close(fd) };
            return None;
        }
        addr.sun_path[0] = 0;
        for (index, byte) in PRIVATE_LOG_SOCKET_NAME.iter().enumerate() {
            addr.sun_path[index + 1] = *byte as _;
        }

        Some(Self {
            fd,
            addr,
            addr_len: (mem::size_of::<libc::sa_family_t>() + PRIVATE_LOG_SOCKET_NAME.len() + 1)
                as libc::socklen_t,
        })
    }

    fn send(&self, level: Level, tag: &str, message: &str) -> bool {
        let message = sanitize_transport_message(message);
        if message.is_empty() {
            return false;
        }
        let packet = format!("{}\t{}\t{}", level_to_code(level), tag, message);
        // SAFETY: packet and addr remain alive for the call and their lengths match the buffers.
        unsafe {
            sendto(
                self.fd,
                packet.as_ptr() as *const c_void,
                packet.len(),
                0,
                &self.addr as *const _ as *const sockaddr,
                self.addr_len,
            ) >= 0
        }
    }
}

impl Log for LogAdapter {
    fn enabled(&self, metadata: &Metadata) -> bool {
        is_record_enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = map_log_level(record.level());
        let tag = resolve_record_tag(record.target());
        let message = format_record_message(level, tag, &record.args().to_string());
        write_log(level, tag, &message);
    }

    fn flush(&self) {}
}

fn format_record_message(level: Level, tag: &str, message: &str) -> String {
    if message.is_empty() {
        return String::new();
    }
    if tag == DEFAULT_LOG_TAG {
        return format!("[Rs{}] {}", level_to_text(level), message);
    }
    message.to_string()
}

fn ensure_log_adapter() {
    LOG_INIT.call_once(|| {
        let _ = log::set_logger(&LOG_ADAPTER);
        log::set_max_level(current_level_filter());
    });
}

pub fn is_debug_logging_enabled() -> bool {
    DEBUG_LOGGING_ENABLED.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn srx_is_debug_logging_enabled() -> bool {
    is_debug_logging_enabled()
}

pub fn is_level_enabled(level: Level) -> bool {
    (level as i32) >= CURRENT_LOG_LEVEL
}

pub fn write_log(level: Level, tag: &str, message: &str) {
    if tag.is_empty() || message.is_empty() {
        return;
    }
    let is_critical = matches!(level, Level::Warn | Level::Error);
    if tag != FILE_MONITOR_LOG_TAG
        && tag != STATS_LOG_TAG
        && !is_critical
        && !is_debug_logging_enabled()
    {
        return;
    }

    let private_sent = private_log_socket()
        .map(|socket| socket.send(level, tag, message))
        .unwrap_or(false);
    if is_critical || (!private_sent && matches!(tag, FILE_MONITOR_LOG_TAG | STATS_LOG_TAG)) {
        android_log(level_to_priority(level), tag, message);
    }
}

fn is_record_enabled(metadata: &Metadata) -> bool {
    if !is_level_enabled(map_log_level(metadata.level())) {
        return false;
    }
    metadata.target() == FILE_MONITOR_LOG_TAG
        || metadata.target() == STATS_LOG_TAG
        || matches!(metadata.level(), LogLevel::Warn | LogLevel::Error)
        || is_debug_logging_enabled()
}

fn private_log_socket() -> Option<&'static PrivateLogSocket> {
    if PRIVATE_LOG_SOCKET.get().is_none() && is_zygote_selinux_context() {
        return None;
    }
    PRIVATE_LOG_SOCKET
        .get_or_init(PrivateLogSocket::new)
        .as_ref()
}

fn is_zygote_selinux_context() -> bool {
    std::fs::read_to_string("/proc/self/attr/current")
        .map(|context| context.contains("zygote"))
        .unwrap_or(false)
}

fn sanitize_transport_message(message: &str) -> String {
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

fn map_log_level(level: LogLevel) -> Level {
    match level {
        LogLevel::Error => Level::Error,
        LogLevel::Warn => Level::Warn,
        LogLevel::Info => Level::Info,
        LogLevel::Debug => Level::Debug,
        LogLevel::Trace => Level::Verbose,
    }
}

fn current_level_filter() -> LevelFilter {
    match CURRENT_LOG_LEVEL {
        LOG_LEVEL_VERBOSE => LevelFilter::Trace,
        LOG_LEVEL_DEBUG => LevelFilter::Debug,
        LOG_LEVEL_INFO => LevelFilter::Info,
        LOG_LEVEL_WARN => LevelFilter::Warn,
        LOG_LEVEL_ERROR => LevelFilter::Error,
        _ => LevelFilter::Info,
    }
}

fn resolve_record_tag(target: &str) -> &str {
    if target == FILE_MONITOR_LOG_TAG {
        return FILE_MONITOR_LOG_TAG;
    }
    if target == STATS_LOG_TAG {
        return STATS_LOG_TAG;
    }
    DEFAULT_LOG_TAG
}

fn level_to_priority(level: Level) -> i32 {
    match level {
        Level::Verbose => ANDROID_LOG_VERBOSE,
        Level::Debug => ANDROID_LOG_DEBUG,
        Level::Info => ANDROID_LOG_INFO,
        Level::Warn => ANDROID_LOG_WARN,
        Level::Error => ANDROID_LOG_ERROR,
    }
}

fn level_to_text(level: Level) -> &'static str {
    match level {
        Level::Verbose => "Verbose",
        Level::Debug => "Debug",
        Level::Info => "Info",
        Level::Warn => "Warn",
        Level::Error => "Error",
    }
}

fn level_to_code(level: Level) -> char {
    match level {
        Level::Verbose => 'V',
        Level::Debug => 'D',
        Level::Info => 'I',
        Level::Warn => 'W',
        Level::Error => 'E',
    }
}

fn android_log(priority: i32, tag: &str, message: &str) {
    let Ok(tag_c) = CString::new(tag) else {
        return;
    };
    let Ok(msg_c) = CString::new(message) else {
        return;
    };
    unsafe {
        __android_log_print(priority, tag_c.as_ptr(), c"%s".as_ptr(), msg_c.as_ptr());
    }
}

unsafe extern "C" {
    fn __android_log_print(prio: c_int, tag: *const c_char, fmt: *const c_char, ...) -> c_int;
}
