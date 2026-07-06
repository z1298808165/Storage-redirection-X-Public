use super::{MountPlanner, PrimaryMountFailure};
use crate::platform::paths;
use libc::{lstat, stat as c_stat};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};

const ALIAS_SUCCESS_LOG_STEP: u64 = 128;
static ALIAS_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);

#[inline]
fn should_log_step(count: u64, step: u64) -> bool {
    count == 1 || count.is_multiple_of(step)
}

impl MountPlanner {
    pub(super) fn expand_storage_alias_paths(&self, canonical_path: &str) -> Vec<String> {
        if canonical_path.is_empty() {
            return Vec::new();
        }

        let user_str = self.user_id.to_string();
        let storage_root = paths::storage_user_root_for_user(self.user_id);
        if !paths::is_same_or_child(canonical_path, &storage_root) {
            return vec![canonical_path.to_string()];
        }

        let suffix = &canonical_path[storage_root.len()..];
        let mut alias_roots: Vec<String> = Vec::with_capacity(13);
        append_unique(&mut alias_roots, storage_root.clone());
        append_unique(
            &mut alias_roots,
            paths::data_media_user_root_for_user(self.user_id),
        );
        append_unique(&mut alias_roots, "/storage/self/primary".to_string());
        if self.user_id == 0 {
            append_unique(&mut alias_roots, "/storage/emulated/legacy".to_string());
        }

        append_unique(
            &mut alias_roots,
            format!("/mnt/user/{}/emulated/{}", user_str, user_str),
        );
        append_unique(
            &mut alias_roots,
            format!("/mnt/runtime/default/emulated/{}", user_str),
        );
        append_unique(
            &mut alias_roots,
            format!("/mnt/runtime/read/emulated/{}", user_str),
        );
        append_unique(
            &mut alias_roots,
            format!("/mnt/runtime/write/emulated/{}", user_str),
        );
        append_unique(
            &mut alias_roots,
            format!("/mnt/installer/{}/emulated/{}", user_str, user_str),
        );
        append_unique(
            &mut alias_roots,
            format!("/mnt/installer/emulated/{}", user_str),
        );
        append_unique(
            &mut alias_roots,
            format!("/mnt/androidwritable/{}/emulated/{}", user_str, user_str),
        );
        append_unique(
            &mut alias_roots,
            format!("/mnt/androidwritable/emulated/{}", user_str),
        );
        append_unique(
            &mut alias_roots,
            format!("/mnt/pass_through/{}/emulated/{}", user_str, user_str),
        );
        append_unique(
            &mut alias_roots,
            format!("/mnt/pass_through/emulated/{}", user_str),
        );

        let mut expanded = Vec::with_capacity(alias_roots.len());
        for root in alias_roots {
            append_unique(&mut expanded, format!("{}{}", root, suffix));
        }
        expanded
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_mount_with_storage_aliases(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_mode: PrimaryMountFailure,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        let mut is_any_mounted = false;
        let alias_targets = self.expand_storage_alias_paths(primary_target);

        for target in alias_targets {
            let is_primary_target = target == primary_target;
            if !is_primary_target && !path_exists(&target) {
                continue;
            }
            if !is_primary_target && should_skip_self_shadowing_alias(source, &target) {
                log::debug!(
                    "alias: skip self-shadowing bind src={} dst={}",
                    source,
                    target
                );
                continue;
            }

            if !self.bind_mount(source, &target, is_recursive) {
                if is_primary_target {
                    if let Some(log_text) = primary_failure_log {
                        log::warn!("alias: {} dst={}", log_text, target);
                    }

                    match primary_failure_mode {
                        PrimaryMountFailure::AbortAll => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return false;
                        }
                        PrimaryMountFailure::StopCurrentTarget => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return true;
                        }
                    }
                }

                if let Some(log_text) = alias_failure_log {
                    log::warn!("alias: {} src={} dst={}", log_text, source, target);
                }
                continue;
            }

            is_any_mounted = true;
            if !is_primary_target && let Some(log_text) = alias_success_log {
                let alias_ok_count = ALIAS_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(alias_ok_count, ALIAS_SUCCESS_LOG_STEP) {
                    log::debug!(
                        "alias: {} src={} dst={} n={}",
                        log_text,
                        source,
                        target,
                        alias_ok_count
                    );
                }
            }
        }

        if let Some(out) = is_any_mounted_out {
            *out = is_any_mounted;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_overlay_mount_with_storage_aliases(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_mode: PrimaryMountFailure,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        let mut is_any_mounted = false;
        let alias_targets = self.expand_storage_alias_paths(primary_target);

        for target in alias_targets {
            let is_primary_target = target == primary_target;
            if !is_primary_target && !path_exists(&target) {
                continue;
            }
            if !is_primary_target && should_skip_self_shadowing_alias(source, &target) {
                log::debug!(
                    "alias: skip self-shadowing overlay src={} dst={}",
                    source,
                    target
                );
                continue;
            }

            if !self.bind_mount_overlay(source, &target, is_recursive) {
                if is_primary_target {
                    if let Some(log_text) = primary_failure_log {
                        log::warn!("alias: {} dst={}", log_text, target);
                    }

                    match primary_failure_mode {
                        PrimaryMountFailure::AbortAll => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return false;
                        }
                        PrimaryMountFailure::StopCurrentTarget => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return true;
                        }
                    }
                }

                if let Some(log_text) = alias_failure_log {
                    log::warn!("alias: {} src={} dst={}", log_text, source, target);
                }
                continue;
            }

            is_any_mounted = true;
            if !is_primary_target && let Some(log_text) = alias_success_log {
                let alias_ok_count = ALIAS_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(alias_ok_count, ALIAS_SUCCESS_LOG_STEP) {
                    log::debug!(
                        "alias: {} src={} dst={} n={}",
                        log_text,
                        source,
                        target,
                        alias_ok_count
                    );
                }
            }
        }

        if let Some(out) = is_any_mounted_out {
            *out = is_any_mounted;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_read_only_mount_with_storage_aliases(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_mode: PrimaryMountFailure,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        let mut is_any_mounted = false;
        let alias_targets = self.expand_storage_alias_paths(primary_target);

        for target in alias_targets {
            let is_primary_target = target == primary_target;
            if !is_primary_target && !path_exists(&target) {
                continue;
            }
            if !is_primary_target && should_skip_self_shadowing_alias(source, &target) {
                log::debug!(
                    "alias: skip self-shadowing readonly src={} dst={}",
                    source,
                    target
                );
                continue;
            }

            if !self.bind_mount_read_only(source, &target, is_recursive) {
                if is_primary_target {
                    if let Some(log_text) = primary_failure_log {
                        log::warn!("alias: {} dst={}", log_text, target);
                    }

                    match primary_failure_mode {
                        PrimaryMountFailure::AbortAll => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return false;
                        }
                        PrimaryMountFailure::StopCurrentTarget => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return true;
                        }
                    }
                }

                if let Some(log_text) = alias_failure_log {
                    log::warn!("alias: {} src={} dst={}", log_text, source, target);
                }
                continue;
            }

            is_any_mounted = true;
            if !is_primary_target && let Some(log_text) = alias_success_log {
                let alias_ok_count = ALIAS_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(alias_ok_count, ALIAS_SUCCESS_LOG_STEP) {
                    log::debug!(
                        "alias: {} src={} dst={} n={}",
                        log_text,
                        source,
                        target,
                        alias_ok_count
                    );
                }
            }
        }

        if let Some(out) = is_any_mounted_out {
            *out = is_any_mounted;
        }
        true
    }
}

fn path_exists(path: &str) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
    let ret = unsafe { lstat(c_path.as_ptr(), st.as_mut_ptr()) };
    ret == 0
}

fn append_unique(list: &mut Vec<String>, value: String) {
    if value.is_empty() {
        return;
    }
    if !list.iter().any(|item| item == &value) {
        list.push(value);
    }
}

fn should_skip_self_shadowing_alias(source: &str, target: &str) -> bool {
    !paths::eq_ignore_case(source, target) && paths::is_child(source, target)
}

#[cfg(test)]
mod tests {
    use super::{MountPlanner, should_skip_self_shadowing_alias};

    #[test]
    fn storage_aliases_include_data_media_backend_for_lower_fs_writers() {
        let planner = MountPlanner::new("com.example.app", 10123, "", "/data/local/tmp/srx", false);

        let aliases = planner.expand_storage_alias_paths("/storage/emulated/0/Download/Locked");

        assert!(aliases.contains(&"/storage/emulated/0/Download/Locked".to_string()));
        assert!(aliases.contains(&"/data/media/0/Download/Locked".to_string()));
    }

    #[test]
    fn storage_alias_bind_skips_private_backend_parent_shadowing() {
        assert!(should_skip_self_shadowing_alias(
            "/data/media/0/Android/data/com.example.app/sdcard",
            "/data/media/0"
        ));
        assert!(!should_skip_self_shadowing_alias(
            "/data/media/0/Android/data/com.example.app/sdcard",
            "/storage/emulated/0"
        ));
        assert!(!should_skip_self_shadowing_alias(
            "/data/media/0/Download/SrtAllow",
            "/data/media/0/Download/SrtAllow"
        ));
    }
}
