use super::{SettingsHub, SettingsState};
use crate::platform::paths;
use std::fs;
use std::io::Read;

const GLOBAL_CONFIG_FILE: &str = "global.json";
const MONITOR_FILTERS_CONFIG_FILE: &str = "file_monitor_filters.json";
const APPS_CONFIG_DIR: &str = "apps";
const SELF_PACKAGE_NAME: &str = "com.storage.redirect.x";
const CONFIG_LOAD_SLOW_MS: i64 = 20;
const APP_CONFIG_SLOW_MS: i64 = 5;
const MAX_CONFIG_FILE_BYTES: u64 = 1024 * 1024;

impl SettingsHub {
    pub fn init(&self, config_dir: Option<&str>) -> bool {
        let config_dir = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            if let Some(dir) = config_dir
                && !dir.is_empty()
            {
                state.config_dir = dir.to_string();
            }

            if state.is_loaded {
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
                "config loaded monitor={} fuse_fix={} fuse_daemon={} verbose_log={} apps={}",
                loaded_state.is_file_monitor_enabled,
                loaded_state.is_fuse_fix_enabled,
                loaded_state.is_fuse_daemon_redirect_enabled,
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
        let global_started_ms = paths::monotonic_ms();
        if !load_global_config(&mut loaded_state) {
            log::warn!("global config load failed, monitor off");
            loaded_state.is_file_monitor_enabled = false;
            loaded_state.is_fuse_daemon_redirect_enabled = false;
            loaded_state.is_verbose_logging_enabled = false;
        }
        load_monitor_filter_config(&mut loaded_state);
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
            "config reloaded monitor={} fuse_fix={} fuse_daemon={} verbose_log={} apps={}",
            loaded_state.is_file_monitor_enabled,
            loaded_state.is_fuse_fix_enabled,
            loaded_state.is_fuse_daemon_redirect_enabled,
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
    let global_path = paths::join(&state.config_dir, GLOBAL_CONFIG_FILE);
    let mut is_too_large = false;
    let content = read_file(&global_path, &mut is_too_large);

    if is_too_large {
        log::warn!("global config too large, ignored: {}", global_path);
        state.is_file_monitor_enabled = false;
        state.is_fuse_fix_enabled = true;
        state.is_fuse_daemon_redirect_enabled = false;
        state.is_verbose_logging_enabled = false;
        return false;
    }

    if content.is_empty() {
        if state.should_log_summary {
            log::debug!("global config missing, defaults");
        }
        state.is_file_monitor_enabled = false;
        state.is_fuse_fix_enabled = true;
        state.is_fuse_daemon_redirect_enabled = false;
        state.is_verbose_logging_enabled = false;
        return true;
    }

    super::ingest::parse_global_config(state, &content)
}

fn load_monitor_filter_config(state: &mut SettingsState) -> bool {
    let filter_path = paths::join(&state.config_dir, MONITOR_FILTERS_CONFIG_FILE);
    let mut is_too_large = false;
    let content = read_file(&filter_path, &mut is_too_large);

    if is_too_large {
        log::warn!("monitor filter config too large, defaults: {}", filter_path);
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
    let started_ms = paths::monotonic_ms();
    if package_name == SELF_PACKAGE_NAME {
        log::warn!("skip self config: {}", package_name);
        return false;
    }

    let apps_dir = paths::join(&state.config_dir, APPS_CONFIG_DIR);
    let app_path = paths::join(&apps_dir, &format!("{}.json", package_name));
    let is_existing = fs::metadata(&app_path).is_ok();
    let mut is_too_large = false;
    let content = read_file(&app_path, &mut is_too_large);
    if content.is_empty() {
        if is_too_large {
            log::warn!("app config too large, ignored: {}", package_name);
            state.invalid_packages.insert(package_name.to_string());
            log_app_config_perf(package_name, content.len(), started_ms, false);
            return false;
        }
        if is_existing {
            state.invalid_packages.insert(package_name.to_string());
        }
        log_app_config_perf(package_name, content.len(), started_ms, false);
        return false;
    }

    let is_ok = super::ingest::parse_app_config(state, package_name, &content);
    if !is_ok && is_existing {
        state.invalid_packages.insert(package_name.to_string());
    } else {
        state.invalid_packages.remove(package_name);
    }
    log_app_config_perf(package_name, content.len(), started_ms, is_ok);
    is_ok
}

// 超过 MAX_CONFIG_FILE_BYTES 返回空并置位 is_too_large
fn read_file(path: &str, is_too_large: &mut bool) -> String {
    *is_too_large = false;
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return String::new(),
    };

    if metadata.len() > MAX_CONFIG_FILE_BYTES {
        *is_too_large = true;
        return String::new();
    }

    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return String::new(),
    };

    let mut content = String::new();
    if file.read_to_string(&mut content).is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn config_version_advances_on_forced_reload_even_when_fingerprint_is_unchanged() {
        let config_dir = temp_config_dir("generation_force_reload");
        std::fs::create_dir_all(config_dir.join(APPS_CONFIG_DIR)).expect("create apps dir");

        let hub = SettingsHub::new();
        assert_eq!(hub.config_version(), 0);

        assert!(hub.init(Some(&config_dir.to_string_lossy())));
        let after_init = hub.config_version();
        assert_ne!(after_init, 0);

        assert!(hub.reload_if_changed());
        assert_eq!(hub.config_version(), after_init);

        assert!(hub.reload_force());
        let after_force = hub.config_version();
        assert_ne!(after_force, after_init);

        assert!(hub.reload_if_changed());
        assert_eq!(hub.config_version(), after_force);

        let _ = std::fs::remove_dir_all(config_dir);
    }

    fn temp_config_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "srx_config_{}_{}_{}",
            name,
            std::process::id(),
            now
        ))
    }
}
