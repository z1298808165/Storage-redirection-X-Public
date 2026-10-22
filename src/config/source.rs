use super::{SettingsHub, SettingsState};
use crate::platform::paths;
use std::fs;
use std::io::Read;

/// 配置文件与子目录的相对名统一定义；fingerprint/raw_scan/watcher 按目录参数拼接，
/// 保证指纹计算、原始扫描与监视器引用的是同一组入口名。
pub(super) const GLOBAL_CONFIG_FILE: &str = "global.json";
pub(super) const MONITOR_FILTERS_CONFIG_FILE: &str = "file_monitor_filters.json";
pub(super) const APPS_CONFIG_DIR: &str = "apps";
const SELF_PACKAGE_NAME: &str = "com.storage.redirect.x";
const CONFIG_LOAD_SLOW_MS: i64 = 20;
const APP_CONFIG_SLOW_MS: i64 = 5;
const MAX_CONFIG_FILE_BYTES: u64 = 1024 * 1024;

impl SettingsHub {
    /// 当前生效的配置目录。系统代写/桥接进程返回的是 Zygisk 保留 FD 的
    /// `/proc/self/fd/<fd>/config` 路径（对 inotify/stat 同样可用）。
    pub fn config_dir(&self) -> String {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.config_dir.clone()
    }

    pub fn init(&self, config_dir: Option<&str>) -> bool {
        let config_dir = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            if let Some(dir) = config_dir
                && !dir.is_empty()
            {
                state.config_dir = dir.to_string();
            }

            if state.is_loaded {
                // 早退也同步调试开关：同进程内两次 init（不同 config_dir）的调用顺序
                // 不应决定日志开关是否生效。
                crate::logging::set_debug_logging_enabled(state.is_verbose_logging_enabled);
                return true;
            }
            state.config_dir.clone()
        };

        let load_started_ms = paths::monotonic_ms();
        let mut loaded_state = SettingsState::new();
        loaded_state.config_dir = config_dir;
        let fp_started_ms = paths::monotonic_ms();
        let fingerprint_snapshot =
            super::fingerprint::compute_config_fingerprint_snapshot(&loaded_state.config_dir);
        let fp_ms = paths::monotonic_ms().saturating_sub(fp_started_ms);
        let fingerprint = fingerprint_snapshot.hash;
        loaded_state.last_fingerprint = fingerprint;
        loaded_state.should_log_summary =
            super::fingerprint::should_log_config_summary_once(fingerprint);
        if loaded_state.should_log_summary {
            log::debug!("config mgr init dir={}", loaded_state.config_dir);
        }

        let global_started_ms = paths::monotonic_ms();
        load_global_config(&mut loaded_state);
        load_monitor_filter_config(&mut loaded_state);
        crate::logging::set_debug_logging_enabled(loaded_state.is_verbose_logging_enabled);
        let global_ms = paths::monotonic_ms().saturating_sub(global_started_ms);
        let apps_started_ms = paths::monotonic_ms();
        let scanned_app_count =
            load_app_configs(&mut loaded_state, fingerprint_snapshot.app_packages);
        let apps_ms = paths::monotonic_ms().saturating_sub(apps_started_ms);
        loaded_state.is_loaded = true;
        log_config_load_perf(
            "init",
            &loaded_state,
            scanned_app_count,
            load_started_ms,
            global_ms,
            apps_ms,
            fp_ms,
        );

        if loaded_state.should_log_summary {
            log::info!(
                "config loaded monitor={} fuse_fix={} backend_requested=auto verbose_log={} apps={}",
                loaded_state.is_file_monitor_enabled,
                loaded_state.is_fuse_fix_enabled,
                loaded_state.is_verbose_logging_enabled,
                loaded_state.apps.len()
            );
        }

        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if state.is_loaded {
            return true;
        }
        *state = loaded_state;
        self.bump_config_version();
        true
    }

    // 指纹变化或强制重载时原子替换 SettingsState
    pub fn reload_if_changed(&self) -> bool {
        let config_dir = {
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            state.config_dir.clone()
        };

        self.reload_from_dir(&config_dir, false)
    }

    pub fn reload_force(&self) -> bool {
        let config_dir = {
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            state.config_dir.clone()
        };
        self.reload_from_dir(&config_dir, true)
    }

    /// 把当前生效的全局开关与监视过滤规则复制到待加载状态，作为解析失败时的回退基线。
    ///
    /// 只复制这两类由单个文件整体决定的配置；`apps` 由 `load_app_configs` 按包名逐个
    /// 加载，其解析失败已经在 `parse_app_config` 内保留旧条目，无需在此预置。
    fn seed_loaded_state_from_current(&self, loaded_state: &mut SettingsState) {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded {
            return;
        }
        loaded_state.is_file_monitor_enabled = state.is_file_monitor_enabled;
        loaded_state.is_fuse_fix_enabled = state.is_fuse_fix_enabled;
        loaded_state.storage_backend_mode = state.storage_backend_mode;
        loaded_state.is_verbose_logging_enabled = state.is_verbose_logging_enabled;
        loaded_state.monitor_filters = state.monitor_filters.clone();
    }

    fn reload_from_dir(&self, config_dir: &str, force: bool) -> bool {
        let (is_loaded, last_fingerprint) = {
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            (state.is_loaded, state.last_fingerprint)
        };

        let fp_started_ms = paths::monotonic_ms();
        let fingerprint_snapshot =
            super::fingerprint::compute_config_fingerprint_snapshot(config_dir);
        let fp_ms = paths::monotonic_ms().saturating_sub(fp_started_ms);
        let fingerprint = fingerprint_snapshot.hash;
        if !force && is_loaded && fingerprint == last_fingerprint {
            return true;
        }

        log::info!(
            "config fp change cached={:x} cur={:x}, reload force={}",
            last_fingerprint,
            fingerprint,
            force
        );

        let load_started_ms = paths::monotonic_ms();
        let mut loaded_state = SettingsState::new();
        loaded_state.config_dir = config_dir.to_string();
        loaded_state.should_log_summary =
            super::fingerprint::should_log_config_summary_once(fingerprint);
        // 以当前生效的配置作为起点：全局配置或过滤配置解析失败时，解析函数不会改动
        // 对应字段，重载后仍保持上一次生效的值，而不是被静默重置为默认值。
        self.seed_loaded_state_from_current(&mut loaded_state);
        let global_started_ms = paths::monotonic_ms();
        if !load_global_config(&mut loaded_state) {
            log::warn!("global config load failed, keeping previous global settings");
        }
        if !load_monitor_filter_config(&mut loaded_state) {
            log::warn!("monitor filter config load failed, keeping previous filters");
        }
        crate::logging::set_debug_logging_enabled(loaded_state.is_verbose_logging_enabled);
        let global_ms = paths::monotonic_ms().saturating_sub(global_started_ms);
        let apps_started_ms = paths::monotonic_ms();
        let scanned_app_count =
            load_app_configs(&mut loaded_state, fingerprint_snapshot.app_packages);
        let apps_ms = paths::monotonic_ms().saturating_sub(apps_started_ms);
        loaded_state.is_loaded = true;
        loaded_state.last_fingerprint = fingerprint;
        log_config_load_perf(
            "reload",
            &loaded_state,
            scanned_app_count,
            load_started_ms,
            global_ms,
            apps_ms,
            fp_ms,
        );

        log::info!(
            "config reloaded monitor={} fuse_fix={} backend_requested=auto verbose_log={} apps={}",
            loaded_state.is_file_monitor_enabled,
            loaded_state.is_fuse_fix_enabled,
            loaded_state.is_verbose_logging_enabled,
            loaded_state.apps.len()
        );

        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if state.config_dir != loaded_state.config_dir {
            return true;
        }
        *state = loaded_state;
        self.bump_config_version();
        true
    }
}

fn load_global_config(state: &mut SettingsState) -> bool {
    load_global_config_inner(state, true)
}

/// `allow_mid_write_retry`：truncate-then-write 的非原子写入窗口里，global.json
/// 可能被读成空内容——「用户清空」与「mid-write」在此无法直接区分。短暂等待后
/// 重读一次：mid-write 自愈；确实被清空则仅延后 50ms 走原有默认值路径。
fn load_global_config_inner(state: &mut SettingsState, allow_mid_write_retry: bool) -> bool {
    let global_path = paths::join(&state.config_dir, GLOBAL_CONFIG_FILE);
    let mut is_too_large = false;
    let mut read_failed = false;
    let content = read_file(&global_path, &mut is_too_large, &mut read_failed);

    // 文件过大属于内容异常，交由调用方保留上一次生效的开关，不在此处重置。
    if is_too_large {
        log::warn!("global config too large, ignored: {}", global_path);
        return false;
    }

    // 读取失败（权限、EIO 等）不是用户的清空动作：保留上一次生效的全局开关，
    // 绝不能把 verbose/monitor 静默重置为默认值。
    if read_failed {
        log::warn!(
            "global config unreadable, keeping previous settings: {}",
            global_path
        );
        return false;
    }

    // 文件缺失或为空表示用户确实清空了配置，回落默认值。
    if content.is_empty() {
        let is_existing = fs::metadata(&global_path).is_ok();
        if is_existing && allow_mid_write_retry {
            std::thread::sleep(std::time::Duration::from_millis(50));
            return load_global_config_inner(state, false);
        }
        if state.should_log_summary {
            log::debug!("global config missing, defaults");
        }
        state.is_file_monitor_enabled = false;
        state.is_fuse_fix_enabled = true;
        state.storage_backend_mode = super::StorageBackendMode::Auto;
        state.is_verbose_logging_enabled = false;
        return true;
    }

    super::ingest::parse_global_config(state, &content)
}

fn load_monitor_filter_config(state: &mut SettingsState) -> bool {
    let filter_path = paths::join(&state.config_dir, MONITOR_FILTERS_CONFIG_FILE);
    let mut is_too_large = false;
    let mut read_failed = false;
    let content = read_file(&filter_path, &mut is_too_large, &mut read_failed);

    if is_too_large {
        log::warn!("monitor filter config too large, defaults: {}", filter_path);
        return false;
    }

    // 读取失败保留上一次生效的过滤规则，与全局开关的处理保持一致。
    if read_failed {
        log::warn!(
            "monitor filter config unreadable, keeping previous filters: {}",
            filter_path
        );
        return false;
    }

    if content.is_empty() {
        if state.should_log_summary {
            log::debug!("monitor filter config missing, defaults");
        }
        return true;
    }

    super::ingest::parse_monitor_filter_config(state, &content)
}

fn load_app_configs(state: &mut SettingsState, packages: Vec<String>) -> usize {
    let package_count = packages.len();
    if state.should_log_summary {
        log::debug!("app configs found n={}", package_count);
    }

    for package_name in packages {
        load_app_config(state, &package_name);
    }
    package_count
}

fn load_app_config(state: &mut SettingsState, package_name: &str) -> bool {
    load_app_config_inner(state, package_name, true)
}

/// `allow_mid_write_retry`：写入方普遍以 truncate-then-write 落盘（非原子），
/// reload 可能恰好读到半截内容。对「文件存在但拿不到有效配置」（空/读失败/解析失败）
/// 短暂等待后重读一次：mid-write 场景第二次读取即自愈；真被清空/损坏时仅多 50ms，
/// 收尾行为与原来一致。没有这层自愈时，应用会带着 invalid 缓存以未配置（monitor-only
/// 只读）姿态走完整个场景——EROFS/ENOENT 且应用重启救不回，直到下次配置重写。
fn load_app_config_inner(
    state: &mut SettingsState,
    package_name: &str,
    allow_mid_write_retry: bool,
) -> bool {
    let started_ms = paths::monotonic_ms();
    if package_name == SELF_PACKAGE_NAME {
        log::warn!("skip self config: {}", package_name);
        return false;
    }

    let apps_dir = paths::join(&state.config_dir, APPS_CONFIG_DIR);
    let app_path = paths::join(&apps_dir, &format!("{}.json", package_name));
    let is_existing = fs::metadata(&app_path).is_ok();
    let mut is_too_large = false;
    let mut read_failed = false;
    let content = read_file(&app_path, &mut is_too_large, &mut read_failed);
    if content.is_empty() {
        if is_too_large {
            log::warn!("app config too large, ignored: {}", package_name);
            state.invalid_packages.insert(package_name.to_string());
            log_app_config_perf(package_name, content.len(), started_ms, false);
            return false;
        }
        if is_existing || read_failed {
            if allow_mid_write_retry {
                std::thread::sleep(std::time::Duration::from_millis(50));
                return load_app_config_inner(state, package_name, false);
            }
            // 读取失败不是配置不存在：记为无效以便后续重载重试，同时留可见痕迹。
            if read_failed {
                log::warn!("app config unreadable: {} ({})", package_name, app_path);
            }
            if is_existing {
                state.invalid_packages.insert(package_name.to_string());
            }
            log_app_config_perf(package_name, content.len(), started_ms, false);
            return false;
        }
        log_app_config_perf(package_name, content.len(), started_ms, false);
        return false;
    }

    let is_ok = super::ingest::parse_app_config(state, package_name, &content);
    if !is_ok && is_existing {
        if allow_mid_write_retry {
            std::thread::sleep(std::time::Duration::from_millis(50));
            return load_app_config_inner(state, package_name, false);
        }
        state.invalid_packages.insert(package_name.to_string());
    } else {
        state.invalid_packages.remove(package_name);
    }
    // 加载结果落规则计数：全量场景跑的失败家族（ENOENT/EROFS）根因是
    // 「reload 时盘上内容 vs 请求构建时的规则快照」不一致，这里把每次加载
    // 解析出的规则数与字节数落到日志，即可直接判定盘上文件当时的内容形态。
    if is_ok
        && let Some(profile) = state.apps.get(package_name)
        && let Some(user) = profile.user_profiles.get(&0)
    {
        log::debug!(
            "app config rules pkg={} bytes={} allow={} excl={} sandbox={} ro={} map={} enabled={}",
            package_name,
            content.len(),
            user.allowed_real_paths.len(),
            user.excluded_real_paths.len(),
            user.sandboxed_paths.len(),
            user.read_only_paths.len(),
            user.path_mappings.len(),
            user.is_enabled
        );
    }
    log_app_config_perf(package_name, content.len(), started_ms, is_ok);
    is_ok
}

// 超过 MAX_CONFIG_FILE_BYTES 返回空并置位 is_too_large
/// 读取配置文件内容。
///
/// 三种结局需要区分，否则会把「暂时读不到」误判成「用户清空了配置」并静默重置
/// 全局开关（verbose/monitor 曾因此被意外关闭，导致诊断日志整体静默）：
/// - 文件不存在（NotFound）或确实为空：`read_failed=false`，内容为空——调用方按
///   「用户清空配置」回落默认值；
/// - 文件过大：`is_too_large=true`——调用方保留上一次生效值；
/// - 其它 I/O 失败（权限、EIO 等）：`read_failed=true`——调用方保留上一次生效值。
fn read_file(path: &str, is_too_large: &mut bool, read_failed: &mut bool) -> String {
    *is_too_large = false;
    *read_failed = false;
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return String::new(),
        Err(error) => {
            log::warn!("config file stat failed ({}): {}", error.kind(), path);
            *read_failed = true;
            return String::new();
        }
    };

    if metadata.len() > MAX_CONFIG_FILE_BYTES {
        *is_too_large = true;
        return String::new();
    }

    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!("config file open failed ({}): {}", error.kind(), path);
            *read_failed = true;
            return String::new();
        }
    };

    let mut content = String::new();
    if let Err(error) = file.read_to_string(&mut content) {
        log::warn!("config file read failed ({}): {}", error.kind(), path);
        *read_failed = true;
        return String::new();
    }
    content
}

fn log_config_load_perf(
    phase: &str,
    state: &SettingsState,
    scanned_app_count: usize,
    started_ms: i64,
    global_ms: i64,
    apps_ms: i64,
    fp_ms: i64,
) {
    let elapsed_ms = paths::monotonic_ms().saturating_sub(started_ms);
    if elapsed_ms < CONFIG_LOAD_SLOW_MS && scanned_app_count < 100 {
        return;
    }
    log::info!(
        "perf config {} dir={} scanned={} loaded={} invalid={} global_ms={} apps_ms={} fp_ms={} total_ms={}",
        phase,
        state.config_dir,
        scanned_app_count,
        state.apps.len(),
        state.invalid_packages.len(),
        global_ms,
        apps_ms,
        fp_ms,
        elapsed_ms
    );
}

fn log_app_config_perf(package_name: &str, bytes: usize, started_ms: i64, is_ok: bool) {
    let elapsed_ms = paths::monotonic_ms().saturating_sub(started_ms);
    if elapsed_ms >= APP_CONFIG_SLOW_MS {
        log::info!(
            "perf config app pkg={} bytes={} ok={} ms={}",
            package_name,
            bytes,
            is_ok,
            elapsed_ms
        );
    }
}
