// 系统代写与共享 UID 策略
use crate::platform::module_paths::SYSTEM_WRITER_UIDS_FILE;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

const MEDIA_PROVIDER_GOOGLE: &str = "com.google.android.providers.media.module";
const MEDIA_PROVIDER_AOSP: &str = "com.android.providers.media.module";
const MEDIA_PROVIDER_LEGACY: &str = "com.android.providers.media";
const DOWNLOAD_PROVIDER: &str = "com.android.providers.downloads";
const DOWNLOAD_PROVIDER_UI: &str = "com.android.providers.downloads.ui";
const EXTERNAL_STORAGE_PROVIDER: &str = "com.android.externalstorage";
const MTP_PACKAGE: &str = "com.android.mtp";

struct SharedUidState {
    uid_by_package: HashMap<String, i32>,
    packages_by_uid: HashMap<i32, Vec<String>>,
    system_writer_shared_uids: HashSet<i32>,
    uid_file_path: String,
    last_fingerprint: u64,
    is_loaded: bool,
}

impl Default for SharedUidState {
    fn default() -> Self {
        Self {
            uid_by_package: HashMap::new(),
            packages_by_uid: HashMap::new(),
            system_writer_shared_uids: HashSet::new(),
            uid_file_path: SYSTEM_WRITER_UIDS_FILE.to_string(),
            last_fingerprint: 0,
            is_loaded: false,
        }
    }
}

static SHARED_UID_STATE: Lazy<Mutex<SharedUidState>> =
    Lazy::new(|| Mutex::new(SharedUidState::default()));

pub fn is_system_writer_package(package_name: &str) -> bool {
    is_system_writer_candidate(package_name)
}

pub fn is_media_provider_package(package_name: &str) -> bool {
    is_media_provider_candidate(package_name)
}

pub fn media_provider_package_aliases(package_name: &str) -> Vec<&'static str> {
    if !is_media_provider_candidate(package_name) {
        return Vec::new();
    }

    let aliases = [
        MEDIA_PROVIDER_GOOGLE,
        MEDIA_PROVIDER_AOSP,
        MEDIA_PROVIDER_LEGACY,
    ];
    let mut ordered = Vec::with_capacity(aliases.len());
    for alias in aliases {
        if alias == package_name {
            ordered.push(alias);
            break;
        }
    }
    for alias in aliases {
        if alias != package_name {
            ordered.push(alias);
        }
    }
    ordered
}

#[allow(dead_code)]
pub fn media_provider_record_package() -> String {
    for candidate in [
        MEDIA_PROVIDER_AOSP,
        MEDIA_PROVIDER_GOOGLE,
        MEDIA_PROVIDER_LEGACY,
    ] {
        if get_uid_for_package(candidate) >= 0 {
            return candidate.to_string();
        }
    }
    MEDIA_PROVIDER_AOSP.to_string()
}

// 媒体链路中间进程：MediaProvider、代写 provider/service，以及系统文件/照片选择 UI。
pub fn is_media_intermediate_package(package_name: &str) -> bool {
    if package_name.is_empty() {
        return false;
    }

    if is_media_provider_candidate(package_name) {
        return true;
    }

    is_file_monitor_bridge_package(package_name) || is_file_monitor_ui_package(package_name)
}

// 文件监视桥接中间层。它们常参与 SAF、下载与 MTP 流程，但也会影响安装应用、
// 设置和文件管理器启动。
pub fn is_file_monitor_bridge_package(package_name: &str) -> bool {
    if package_name.is_empty() {
        return false;
    }

    package_name == DOWNLOAD_PROVIDER
        || package_name == MTP_PACKAGE
        || package_name == EXTERNAL_STORAGE_PROVIDER
}

// SAF 来源识别复用 native 文件监视，只对真实存储/下载桥进程安装 monitor profile。
// 不把 MTP 与 UI shell 纳入，避免扩大系统 provider 影响面。
pub fn is_saf_native_monitor_bridge_package(package_name: &str) -> bool {
    package_name == DOWNLOAD_PROVIDER || package_name == EXTERNAL_STORAGE_PROVIDER
}

pub fn is_file_monitor_ui_package(package_name: &str) -> bool {
    if package_name.is_empty() {
        return false;
    }

    let normalized = package_name.to_ascii_lowercase();
    normalized == DOWNLOAD_PROVIDER_UI
        || normalized.contains(".documentsui")
        || normalized.contains(".photopicker")
        || normalized.contains(".filemanager")
        || normalized.contains("fileexplorer")
        || normalized.ends_with(".myfiles")
}

pub fn get_system_writer_name(_package_name: &str) -> &'static str {
    "MediaProvider"
}

// 指纹变更时重新加载
pub fn refresh_shared_uid_cache() {
    let mut state = SHARED_UID_STATE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    refresh_shared_uid_cache_locked(&mut state, false);
}

// 系统代写进程通过 Zygisk 保留的模块目录 FD 读取配置，避免依赖最终 mount namespace
// 中可能不可见的 /data/adb/modules 路径。
pub fn set_shared_uid_config_dir(config_dir: &str) {
    if config_dir.is_empty() {
        return;
    }

    let uid_file_path = uid_file_path_for_config_dir(config_dir);
    let mut state = SHARED_UID_STATE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let source_changed = state.uid_file_path != uid_file_path;
    if source_changed {
        state.uid_file_path = uid_file_path;
    }
    refresh_shared_uid_cache_locked(&mut state, source_changed);
}

fn refresh_shared_uid_cache_locked(state: &mut SharedUidState, force: bool) {
    let fingerprint = compute_uid_file_fingerprint(&state.uid_file_path);
    if !force && state.is_loaded && fingerprint == state.last_fingerprint {
        return;
    }
    load_shared_uid_cache(state, fingerprint);
}

pub fn get_shared_group_members(package_name: &str) -> Vec<String> {
    let state = SHARED_UID_STATE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let Some(uid) = state.uid_by_package.get(package_name) else {
        return vec![];
    };
    if !is_shared_uid(*uid, &state.packages_by_uid) {
        return vec![];
    }
    state.packages_by_uid.get(uid).cloned().unwrap_or_default()
}

pub fn is_shared_group_package(package_name: &str) -> bool {
    let state = SHARED_UID_STATE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let Some(uid) = state.uid_by_package.get(package_name) else {
        return false;
    };
    is_shared_uid(*uid, &state.packages_by_uid)
}

pub fn get_packages_for_uid(uid: i32) -> Vec<String> {
    if uid < 0 {
        return vec![];
    }
    let state = SHARED_UID_STATE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    state.packages_by_uid.get(&uid).cloned().unwrap_or_default()
}

pub fn get_all_package_names() -> Vec<String> {
    let state = SHARED_UID_STATE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let mut packages = state.uid_by_package.keys().cloned().collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    packages
}

pub fn get_uid_for_package(package_name: &str) -> i32 {
    if package_name.is_empty() {
        return -1;
    }
    let state = SHARED_UID_STATE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    state
        .uid_by_package
        .get(package_name)
        .copied()
        .unwrap_or(-1)
}

pub fn get_fresh_uid_for_package(package_name: &str) -> i32 {
    if package_name.is_empty() {
        return -1;
    }
    refresh_shared_uid_cache();
    get_uid_for_package(package_name)
}

// 格式: "pkg1,pkg2,pkg3"
pub fn get_shared_uid_packages_string(uid: i32) -> String {
    let packages = get_packages_for_uid(uid);
    if packages.len() < 2 {
        return String::new();
    }
    packages.join(",")
}

pub fn is_shared_uid_process(uid: i32) -> bool {
    if uid < 0 {
        return false;
    }
    let state = SHARED_UID_STATE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    state.system_writer_shared_uids.contains(&uid)
}

fn is_media_provider_candidate(package_name: &str) -> bool {
    package_name == MEDIA_PROVIDER_GOOGLE
        || package_name == MEDIA_PROVIDER_AOSP
        || package_name == MEDIA_PROVIDER_LEGACY
}

// 仅 MediaProvider 是 FUSE 服务端，DP/MTP 作为客户端写入最终经过 MediaProvider
fn is_system_writer_candidate(package_name: &str) -> bool {
    is_media_provider_candidate(package_name)
}

fn trim_ascii(value: &str) -> String {
    let trimmed = value.trim_matches(|c| c == ' ' || c == '\t' || c == '\n' || c == '\r');
    trimmed.to_string()
}

// 格式: "包名:UID"
fn parse_line(line: &str) -> Option<(String, i32)> {
    let trimmed = trim_ascii(line);
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let separator = trimmed.find(':')?;
    let package_value = trim_ascii(&trimmed[..separator]);
    let uid_value = trim_ascii(&trimmed[separator + 1..]);
    if package_value.is_empty() || uid_value.is_empty() {
        return None;
    }

    if !uid_value.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let uid = uid_value.parse::<i32>().ok()?;
    if uid < 0 {
        return None;
    }
    Some((package_value, uid))
}

fn is_shared_uid(uid: i32, packages_by_uid: &HashMap<i32, Vec<String>>) -> bool {
    packages_by_uid
        .get(&uid)
        .map(|packages| packages.len() >= 2)
        .unwrap_or(false)
}

fn uid_file_path_for_config_dir(config_dir: &str) -> String {
    SYSTEM_WRITER_UIDS_FILE.replacen(crate::platform::module_paths::CONFIG_DIR, config_dir, 1)
}

// 基于 mtime 与文件大小
fn compute_uid_file_fingerprint(uid_file_path: &str) -> u64 {
    let Ok(meta) = std::fs::metadata(uid_file_path) else {
        return 0;
    };
    let size = meta.len();
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    size ^ modified
}

fn load_shared_uid_cache(state: &mut SharedUidState, fingerprint: u64) {
    let file = File::open(&state.uid_file_path);
    let Ok(file) = file else {
        handle_uid_cache_open_failure(state, fingerprint);
        return;
    };

    let mut uid_by_package = HashMap::new();
    let mut packages_by_uid: HashMap<i32, Vec<String>> = HashMap::new();

    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let Some((package, uid)) = parse_line(&line) else {
            continue;
        };
        uid_by_package.insert(package.clone(), uid);
        packages_by_uid.entry(uid).or_default().push(package);
    }

    for packages in packages_by_uid.values_mut() {
        packages.sort();
        packages.dedup();
    }

    let mut system_writer_shared_uids = HashSet::new();
    for (uid, packages) in &packages_by_uid {
        if packages.len() < 2 {
            continue;
        }
        if packages.iter().any(|name| is_system_writer_candidate(name)) {
            system_writer_shared_uids.insert(*uid);
        }
    }

    state.uid_by_package = uid_by_package;
    state.packages_by_uid = packages_by_uid;
    state.system_writer_shared_uids = system_writer_shared_uids;
    state.last_fingerprint = fingerprint;
    state.is_loaded = true;
}

fn handle_uid_cache_open_failure(state: &mut SharedUidState, fingerprint: u64) {
    if state.is_loaded && !state.uid_by_package.is_empty() {
        state.last_fingerprint = fingerprint;
        return;
    }

    state.uid_by_package.clear();
    state.packages_by_uid.clear();
    state.system_writer_shared_uids.clear();
    state.last_fingerprint = fingerprint;
    state.is_loaded = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_monitor_ui_detects_system_and_oem_file_shells() {
        assert!(is_file_monitor_ui_package("com.android.documentsui"));
        assert!(is_file_monitor_ui_package("com.android.photopicker"));
        assert!(is_file_monitor_ui_package(
            "com.android.providers.downloads.ui"
        ));
        assert!(is_file_monitor_ui_package("com.coloros.filemanager"));
        assert!(is_file_monitor_ui_package(
            "com.mi.android.globalFileExplorer"
        ));
        assert!(is_file_monitor_ui_package("com.sec.android.app.myfiles"));
        assert!(!is_file_monitor_ui_package(
            "com.android.providers.downloads"
        ));
    }

    #[test]
    fn media_intermediate_includes_file_ui_for_attribution() {
        assert!(is_media_intermediate_package(
            "com.android.providers.media.module"
        ));
        assert!(is_media_intermediate_package(
            "com.android.providers.downloads"
        ));
        assert!(is_media_intermediate_package("com.android.externalstorage"));
        assert!(is_media_intermediate_package("com.coloros.filemanager"));
    }
}
