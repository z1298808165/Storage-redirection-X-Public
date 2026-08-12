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
        // SAFETY: socket 不接收借用指针，成功时返回自有描述符。
        let fd = unsafe { socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0) };
        if fd < 0 {
            return None;
        }

        // SAFETY: sockaddr_un 是允许零初始化的普通 C 结构体。
        let mut addr: sockaddr_un = unsafe { mem::zeroed() };
        addr.sun_family = AF_UNIX as _;
        if PRIVATE_LOG_SOCKET_NAME.len() + 1 > addr.sun_path.len() {
            // SAFETY: fd 由当前代码持有，尚未关闭或转移。
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
        // SAFETY: packet 和 addr 在调用期间保持有效，长度与缓冲区一致。
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

pub fn write_mount_prep_record(package_name: &str, path: &str, backend_path: &str) {
    if package_name.is_empty() || path.is_empty() {
        return;
    }
    let timestamp = build_file_monitor_timestamp();
    if timestamp.is_empty() {
        return;
    }
    let mut line = format!(
        "{}|{}|{}|MKDIR|{}|ret=0|errno=0|identify_method=mount_prep|identify_reliability=high|op=mkdir|source=mount_prep",
        timestamp, package_name, package_name, path,
    );
    if !backend_path.is_empty() {
        line.push_str("|backend=");
        line.push_str(backend_path);
    }
    write_log(Level::Info, FILE_MONITOR_LOG_TAG, &line);
}

fn build_file_monitor_timestamp() -> String {
    let mut now: libc::time_t = 0;
    // SAFETY: libc::time 仅写入由本函数独占的有效 time_t 指针。
    unsafe { libc::time(&mut now as *mut _) };

    // SAFETY: libc::tm 是可按字节置零的 C 时间结构，后续由 localtime_r 完整填充。
    let mut tm_value: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r 接收有效时间值和本地可写结构，返回指针仅用于空指针判断。
    let tm_ptr = unsafe { libc::localtime_r(&now as *const _, &mut tm_value as *mut _) };
    if tm_ptr.is_null() {
        return String::new();
    }

    let mut buffer = [0u8; 32];
    let format = b"%Y-%m-%d %H:%M:%S\0";
    // SAFETY: strftime 使用有效格式、可写缓冲区及已初始化的 tm 结构，并受缓冲区长度约束。
    let written = unsafe {
        libc::strftime(
            buffer.as_mut_ptr() as *mut _,
            buffer.len(),
            format.as_ptr() as *const _,
            &tm_value as *const _,
        )
    };
    if written == 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buffer[..written]).to_string()
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

/// 在 fork 之前完成私有日志通道的初始化。
///
/// `private_log_socket` 走 `OnceLock::get_or_init`。若父进程的某个线程正好在 fork
/// 瞬间处于该初始化过程中，子进程会继承一个「已加锁但永远不会被完成」的 OnceLock，
/// 之后任何一条日志都会永久阻塞——而挂载子进程的整条路径都在记日志。
///
/// 因此在 fork 前先由当前线程走完初始化：此后子进程内的 `get_or_init` 只会命中
/// 已完成状态，不再需要获取内部锁。
///
/// zygote 上下文不建立私有通道，此时 OnceLock 仍保持未初始化，子进程的日志会退回
/// android_log；这条路径不涉及 OnceLock 内部锁，因此不构成阻塞风险。
pub fn prepare_for_fork() {
    let _ = private_log_socket();
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
