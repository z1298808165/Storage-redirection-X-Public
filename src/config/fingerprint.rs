use crate::platform::{module_paths, paths};
use libc::{O_CLOEXEC, O_CREAT, O_EXCL, O_WRONLY, open, stat};
use std::ffi::CString;
use std::fs;

const FNV_OFFSET_BASIS: u64 = 1469598103934665603;
const FNV_PRIME: u64 = 1099511628211;
const FINGERPRINT_SLOW_MS: i64 = 10;

pub struct ConfigFingerprint {
    pub hash: u64,
    pub app_packages: Vec<String>,
}

pub fn compute_config_fingerprint_snapshot(config_dir: &str) -> ConfigFingerprint {
    let started_ms = paths::monotonic_ms();
    let mut hash = FNV_OFFSET_BASIS;
    hash = fnv_update_str(hash, "srx_config_v2");
    hash = add_file_stat(hash, &paths::join(config_dir, "global.json"));
    hash = add_file_stat(hash, &paths::join(config_dir, "file_monitor_filters.json"));

    let apps_dir = paths::join(config_dir, "apps");
    let Ok(dir_entries) = fs::read_dir(&apps_dir) else {
        hash = fnv_update_str(hash, "apps_dir_missing");
        log_config_fingerprint_perf(config_dir, 0, started_ms, hash);
        return ConfigFingerprint {
            hash,
            app_packages: Vec::new(),
        };
    };

    let mut config_files: Vec<(String, Option<String>)> = Vec::new();
    for entry in dir_entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.ends_with(".json") {
            continue;
        }
        let package_name = name_str
            .strip_suffix(".json")
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        config_files.push((paths::join(&apps_dir, name_str.as_ref()), package_name));
    }

    config_files.sort_by(|left, right| left.0.cmp(&right.0));
    hash = fnv_update_u64(hash, config_files.len() as u64);
    let app_count = config_files.len();
    let mut app_packages = Vec::new();
    for (path, package_name) in config_files {
        if let Some(package_name) = package_name {
            app_packages.push(package_name);
        }
        hash = add_file_stat(hash, &path);
    }

    log_config_fingerprint_perf(config_dir, app_count, started_ms, hash);
    ConfigFingerprint { hash, app_packages }
}

// marker 文件 O_EXCL 成功即首次出现，用于一次性输出摘要日志
pub fn should_log_config_summary_once(fingerprint: u64) -> bool {
    let _ = fs::create_dir_all(module_paths::LOG_DIR);

    if let Ok(entries) = fs::read_dir(module_paths::LOG_DIR) {
        let mut marker_paths = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("config_") || !name.ends_with(".marker") {
                continue;
            }
            marker_paths.push(paths::join(module_paths::LOG_DIR, &name));
        }

        const MAX_MARKERS: usize = 8;
        if marker_paths.len() > MAX_MARKERS {
            marker_paths.sort();
            let remove_count = marker_paths.len() - MAX_MARKERS;
            for path in marker_paths.iter().take(remove_count) {
                let _ = fs::remove_file(path);
            }
        }
    }

    let marker_path = format!(
        "{}/config_{}.marker",
        module_paths::LOG_DIR,
        format_fingerprint_hex(fingerprint)
    );

    let Ok(c_path) = CString::new(marker_path) else {
        return true;
    };
    let fd = unsafe {
        open(
            c_path.as_ptr(),
            O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC,
            0o600,
        )
    };
    if fd >= 0 {
        unsafe { libc::close(fd) };
        return true;
    }
    if last_errno() == libc::EEXIST {
        return false;
    }
    true
}

fn add_file_stat(mut hash: u64, path: &str) -> u64 {
    let Ok(c_path) = CString::new(path) else {
        hash = fnv_update_str(hash, path);
        hash = fnv_update_u64(hash, 0);
        return hash;
    };

    let mut st = std::mem::MaybeUninit::<stat>::uninit();
    let ret = unsafe { libc::stat(c_path.as_ptr(), st.as_mut_ptr()) };
    if ret != 0 {
        hash = fnv_update_str(hash, path);
        hash = fnv_update_u64(hash, 0);
        return hash;
    }

    let st = unsafe { st.assume_init() };
    hash = fnv_update_str(hash, path);
    hash = fnv_update_u64(hash, st.st_dev);
    hash = fnv_update_u64(hash, st.st_ino);
    hash = fnv_update_u64(hash, st.st_mtime as u64);
    hash = fnv_update_u64(hash, stat_mtime_nsec(&st));
    hash = fnv_update_u64(hash, st.st_size as u64);
    hash
}

#[cfg(target_os = "android")]
fn stat_mtime_nsec(st: &stat) -> u64 {
    st.st_mtime_nsec as u64
}

#[cfg(not(target_os = "android"))]
fn stat_mtime_nsec(_st: &stat) -> u64 {
    0
}

fn fnv_update_str(hash: u64, value: &str) -> u64 {
    let bytes = value.as_bytes();
    fnv_update(hash, bytes)
}

fn fnv_update_u64(hash: u64, value: u64) -> u64 {
    fnv_update(hash, &value.to_le_bytes())
}

fn fnv_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn format_fingerprint_hex(value: u64) -> String {
    format!("{:016x}", value)
}

fn log_config_fingerprint_perf(config_dir: &str, app_count: usize, started_ms: i64, hash: u64) {
    let elapsed_ms = paths::monotonic_ms().saturating_sub(started_ms);
    if elapsed_ms >= FINGERPRINT_SLOW_MS || app_count >= 100 {
        log::info!(
            "perf config fingerprint dir={} apps={} ms={} fp={:x}",
            config_dir,
            app_count,
            elapsed_ms,
            hash
        );
    }
}

fn last_errno() -> i32 {
    unsafe { *libc::__errno() }
}
