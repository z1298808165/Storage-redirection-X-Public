use libc::{c_char, c_int};
use log::{Level as LogLevel, LevelFilter, Log, Metadata, Record};
use std::ffi::CString;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

const LOG_LEVEL_VERBOSE: i32 = 0;
const LOG_LEVEL_DEBUG: i32 = 1;
const LOG_LEVEL_INFO: i32 = 2;
const LOG_LEVEL_WARN: i32 = 3;
const LOG_LEVEL_ERROR: i32 = 4;

const CURRENT_LOG_LEVEL: i32 = LOG_LEVEL_DEBUG;
const DEFAULT_LOG_TAG: &str = "StorageRedirect";
const FILE_MONITOR_LOG_TAG: &str = "FileMonitorOp";
const STATS_LOG_TAG: &str = "Stats";

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

struct LogAdapter;

pub struct Logger;

impl Logger {
    pub fn init(_package_name: Option<&str>) {
        ensure_log_adapter();
    }
}

pub fn set_debug_logging_enabled(enabled: bool) {
    DEBUG_LOGGING_ENABLED.store(enabled, Ordering::Relaxed);
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
    if tag != FILE_MONITOR_LOG_TAG && !is_debug_logging_enabled() {
        return;
    }

    let priority = level_to_priority(level);
    android_log(priority, tag, message);
}

fn is_record_enabled(metadata: &Metadata) -> bool {
    if !is_level_enabled(map_log_level(metadata.level())) {
        return false;
    }
    metadata.target() == FILE_MONITOR_LOG_TAG || is_debug_logging_enabled()
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

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level as LogLevel;

    #[test]
    fn file_monitor_records_do_not_require_debug_logging() {
        set_debug_logging_enabled(false);

        let monitor_metadata = Metadata::builder()
            .level(LogLevel::Info)
            .target(FILE_MONITOR_LOG_TAG)
            .build();
        let default_metadata = Metadata::builder()
            .level(LogLevel::Info)
            .target(DEFAULT_LOG_TAG)
            .build();

        assert!(is_record_enabled(&monitor_metadata));
        assert!(!is_record_enabled(&default_metadata));

        set_debug_logging_enabled(true);
        assert!(is_record_enabled(&default_metadata));
        set_debug_logging_enabled(false);
    }
}
