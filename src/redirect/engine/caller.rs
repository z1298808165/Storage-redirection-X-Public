use super::{is_already_private_path, resolve_android_private_path_owner, resolve_package_uid};
use crate::config::SettingsHub;
use crate::hook::stats::InterceptHub;
use crate::monitor::infer_download_owner_package_by_path;
use crate::platform;
use crate::redirect::policy as redirect_policy;
use crate::redirect::writer;

pub(super) struct SystemWriterCallerContext {
    pub(super) effective_caller_uid: i32,
    pub(super) effective_caller_package: String,
    pub(super) user_id: i32,
    pub(super) is_caller_from_inferred: bool,
}

pub(super) struct SystemWriterCallerSignal {
    pub(super) effective_caller_uid: i32,
    original_caller_uid: i32,
    pub(super) effective_caller_package: String,
    has_explicit_caller_signal: bool,
    pub(super) has_external_caller_signal: bool,
    can_infer_caller_by_path: bool,
}

impl SystemWriterCallerSignal {
    pub(super) fn from_hub(
        hub: &InterceptHub,
        package_name: &str,
        self_uid: i32,
        is_explicit_caller_decision: bool,
    ) -> Self {
        let effective_caller_uid = hub.get_current_caller_uid();
        let original_caller_uid = effective_caller_uid;
        let effective_caller_package = hub.get_current_caller_package();
        let has_explicit_caller_signal = original_caller_uid >= writer::ANDROID_APP_UID_START
            || !effective_caller_package.is_empty();
        let has_external_caller_signal = has_external_writer_caller_signal(
            package_name,
            self_uid,
            &effective_caller_package,
            original_caller_uid,
        );
        let can_infer_caller_by_path =
            !is_explicit_caller_decision && !crate::hook::is_path_owner_inference_disabled();

        Self {
            effective_caller_uid,
            original_caller_uid,
            effective_caller_package,
            has_explicit_caller_signal,
            has_external_caller_signal,
            can_infer_caller_by_path,
        }
    }

    pub(super) fn into_resolve_request<'a>(
        self,
        package_name: &'a str,
        self_uid: i32,
        is_shared_uid: bool,
        normalized_path: &'a str,
        user_id: i32,
        is_write_operation: bool,
    ) -> SystemWriterCallerResolveRequest<'a> {
        SystemWriterCallerResolveRequest {
            package_name,
            self_uid,
            is_shared_uid,
            normalized_path,
            is_write_operation,
            has_explicit_caller_signal: self.has_explicit_caller_signal,
            can_infer_caller_by_path: self.can_infer_caller_by_path,
            effective_caller_uid: self.effective_caller_uid,
            original_caller_uid: self.original_caller_uid,
            effective_caller_package: self.effective_caller_package,
            user_id,
        }
    }
}

pub(super) struct SystemWriterCallerResolveRequest<'a> {
    package_name: &'a str,
    self_uid: i32,
    is_shared_uid: bool,
    normalized_path: &'a str,
    is_write_operation: bool,
    has_explicit_caller_signal: bool,
    can_infer_caller_by_path: bool,
    effective_caller_uid: i32,
    original_caller_uid: i32,
    effective_caller_package: String,
    user_id: i32,
}

pub(super) fn has_external_writer_caller_signal(
    package_name: &str,
    self_uid: i32,
    caller_package: &str,
    caller_uid: i32,
) -> bool {
    if !caller_package.is_empty()
        && caller_package != package_name
        && !redirect_policy::is_system_writer_package(caller_package)
    {
        return true;
    }

    caller_uid >= writer::ANDROID_APP_UID_START && caller_uid != self_uid
}

#[cfg(test)]
pub(super) fn has_system_writer_redirect_path_owner_hint(
    user_id: i32,
    normalized_path: &str,
) -> bool {
    if user_id < 0 || normalized_path.is_empty() || crate::hook::is_path_owner_inference_disabled()
    {
        return false;
    }

    !resolve_redirect_owner_package_by_path(user_id, normalized_path).is_empty()
}

pub(super) fn has_system_writer_mapping_request_owner_hint(
    user_id: i32,
    normalized_path: &str,
) -> bool {
    if user_id < 0 || normalized_path.is_empty() || crate::hook::is_path_owner_inference_disabled()
    {
        return false;
    }

    !resolve_mapping_request_owner_package_by_path(user_id, normalized_path).is_empty()
}

pub(super) fn has_system_writer_recent_public_caller_hint(
    user_id: i32,
    normalized_path: &str,
) -> bool {
    if user_id < 0 || normalized_path.is_empty() || crate::hook::is_path_owner_inference_disabled()
    {
        return false;
    }

    crate::monitor::infer_recent_path_caller_identity(normalized_path, user_id).is_some()
}

pub(super) fn has_system_writer_read_only_owner_hint(user_id: i32, normalized_path: &str) -> bool {
    if user_id < 0 || normalized_path.is_empty() || crate::hook::is_path_owner_inference_disabled()
    {
        return false;
    }

    !resolve_read_only_owner_package_by_path(user_id, normalized_path).is_empty()
}

pub(super) fn resolve_system_writer_caller_context(
    hub: &InterceptHub,
    request: SystemWriterCallerResolveRequest<'_>,
) -> SystemWriterCallerContext {
    let SystemWriterCallerResolveRequest {
        package_name,
        self_uid,
        is_shared_uid,
        normalized_path,
        is_write_operation,
        has_explicit_caller_signal,
        can_infer_caller_by_path,
        effective_caller_uid,
        original_caller_uid,
        effective_caller_package,
        user_id,
    } = request;
    let mut effective_caller_uid = effective_caller_uid;
    let mut effective_caller_package = effective_caller_package;
    let mut is_caller_from_inferred = false;
    let mut is_caller_from_mapping_request_owner = false;

    if can_infer_caller_by_path {
        maybe_override_system_writer_caller_by_mapping_request_path(
            normalized_path,
            user_id,
            &mut effective_caller_uid,
            &mut effective_caller_package,
            &mut is_caller_from_inferred,
            &mut is_caller_from_mapping_request_owner,
        );
    }

    if can_infer_caller_by_path && original_caller_uid >= writer::ANDROID_APP_UID_START {
        writer::maybe_override_system_writer_caller_by_path(
            normalized_path,
            &mut effective_caller_uid,
            self_uid,
            user_id,
            &mut effective_caller_package,
            &mut is_caller_from_inferred,
        );
    }

    if effective_caller_package.is_empty()
        && has_explicit_caller_signal
        && let Some(package_name) = resolve_explicit_caller_package_by_uid(
            package_name,
            self_uid,
            effective_caller_uid,
            user_id,
        )
    {
        log::debug!(
            "writer: uid caller={} uid={} path={}",
            package_name,
            effective_caller_uid,
            normalized_path
        );
        effective_caller_package = package_name;
    }

    let has_mapping_request_owner_hint = can_infer_caller_by_path
        && has_system_writer_mapping_request_owner_hint(user_id, normalized_path);
    if can_infer_caller_by_path
        && (is_write_operation || has_explicit_caller_signal || has_mapping_request_owner_hint)
        && (effective_caller_package.is_empty() || !has_explicit_caller_signal)
    {
        maybe_infer_system_writer_caller_by_redirect_path(
            normalized_path,
            user_id,
            original_caller_uid,
            self_uid,
            &mut effective_caller_uid,
            &mut effective_caller_package,
            &mut is_caller_from_inferred,
        );
    }

    let has_external_caller_signal = has_external_writer_caller_signal(
        package_name,
        self_uid,
        &effective_caller_package,
        effective_caller_uid,
    );

    if effective_caller_package.is_empty()
        && !has_external_caller_signal
        && !crate::hook::is_path_owner_inference_disabled()
        && redirect_policy::is_media_provider_package(package_name)
        && let Some(path_identity) =
            crate::monitor::infer_recent_path_caller_identity(normalized_path, user_id)
    {
        let inferred_uid = redirect_policy::get_fresh_uid_for_package(&path_identity.package_name);
        if inferred_uid >= writer::ANDROID_APP_UID_START {
            effective_caller_uid = inferred_uid;
        }
        log::debug!(
            "writer: recent path hint caller={} uid={} source={} path={}",
            path_identity.package_name,
            effective_caller_uid,
            path_identity.source,
            normalized_path
        );
        effective_caller_package = path_identity.package_name;
        is_caller_from_inferred = true;
    }

    let can_override_with_read_only_owner = !has_external_caller_signal
        && (effective_caller_package.is_empty()
            || effective_caller_package == package_name
            || redirect_policy::is_system_writer_package(&effective_caller_package));
    if can_override_with_read_only_owner
        && !crate::hook::is_path_owner_inference_disabled()
        && redirect_policy::is_media_provider_package(package_name)
    {
        let inferred = resolve_read_only_owner_package_by_path(user_id, normalized_path);
        if !inferred.is_empty() {
            let inferred_uid = redirect_policy::get_fresh_uid_for_package(&inferred);
            if inferred_uid >= writer::ANDROID_APP_UID_START {
                effective_caller_uid = inferred_uid;
            }
            log::debug!(
                "writer: read-only path infer caller={} uid={} path={}",
                inferred,
                effective_caller_uid,
                normalized_path
            );
            effective_caller_package = inferred;
            is_caller_from_inferred = true;
        }
    }

    if effective_caller_package.is_empty()
        && !has_explicit_caller_signal
        && !crate::hook::is_path_owner_inference_disabled()
        && let Some(path_owner) = resolve_android_private_path_owner(normalized_path)
    {
        crate::monitor::remember_private_path_owner_hint(normalized_path, &path_owner, user_id);
        let owner_uid = resolve_package_uid(&path_owner);
        if owner_uid >= writer::ANDROID_APP_UID_START {
            effective_caller_uid = owner_uid;
        }
        log::debug!(
            "writer: private path owner caller={} uid={} path={}",
            path_owner,
            effective_caller_uid,
            normalized_path
        );
        effective_caller_package = path_owner;
        is_caller_from_inferred = true;
    }

    if effective_caller_package.is_empty()
        && !has_explicit_caller_signal
        && !crate::hook::is_path_owner_inference_disabled()
        && is_already_private_path(normalized_path)
    {
        let inferred = SettingsHub::instance()
            .resolve_enabled_package_by_path_for_user(user_id, normalized_path);
        if inferred.is_empty() {
            if original_caller_uid < writer::ANDROID_APP_UID_START {
                writer::log_system_writer_skip_path_infer_for_low_uid(
                    original_caller_uid,
                    normalized_path,
                );
            }
        } else {
            crate::monitor::remember_private_path_owner_hint(normalized_path, &inferred, user_id);
            let inferred_uid = redirect_policy::get_fresh_uid_for_package(&inferred);
            if inferred_uid >= writer::ANDROID_APP_UID_START {
                effective_caller_uid = inferred_uid;
            }
            log::debug!(
                "writer: path infer caller={} uid={} path={}",
                inferred,
                effective_caller_uid,
                normalized_path
            );
            effective_caller_package = inferred;
            is_caller_from_inferred = true;
        }
    }

    if effective_caller_package.is_empty()
        && should_query_download_owner_for_writer(self_uid, package_name)
        && let Some(owner_package) = infer_download_owner_package_by_path(normalized_path)
        && !redirect_policy::is_media_intermediate_package(&owner_package)
    {
        let owner_uid = redirect_policy::get_fresh_uid_for_package(&owner_package);
        if owner_uid >= writer::ANDROID_APP_UID_START {
            effective_caller_uid = owner_uid;
        }
        log::debug!(
            "writer: download owner caller={} uid={} path={}",
            owner_package,
            effective_caller_uid,
            normalized_path
        );
        effective_caller_package = owner_package;
        is_caller_from_inferred = true;
    }

    if effective_caller_package.is_empty()
        && is_shared_uid
        && !is_already_private_path(normalized_path)
    {
        let mut candidates: Vec<String> = if effective_caller_uid >= writer::ANDROID_APP_UID_START {
            redirect_policy::get_packages_for_uid(effective_caller_uid)
        } else {
            Vec::new()
        };
        if candidates.is_empty() && original_caller_uid >= writer::ANDROID_APP_UID_START {
            candidates = redirect_policy::get_packages_for_uid(original_caller_uid);
        }
        if candidates.is_empty() {
            candidates = redirect_policy::get_packages_for_uid(self_uid);
        }
        candidates.retain(|pkg| !redirect_policy::is_system_writer_package(pkg));
        if let Some(pkg) = crate::monitor::infer_caller_package_by_stack(&candidates) {
            let inferred_uid = redirect_policy::get_fresh_uid_for_package(&pkg);
            if inferred_uid >= writer::ANDROID_APP_UID_START {
                effective_caller_uid = inferred_uid;
            }
            log::debug!(
                "writer: stack infer caller={} uid={} path={}",
                pkg,
                effective_caller_uid,
                normalized_path
            );
            effective_caller_package = pkg;
            is_caller_from_inferred = true;
        }
    }

    if effective_caller_package.is_empty()
        && self_uid >= writer::ANDROID_APP_UID_START
        && user_id >= 0
        && writer::is_path_in_user_storage(normalized_path, user_id)
        && !redirect_policy::is_system_writer_package(package_name)
        && SettingsHub::instance().should_redirect(package_name, self_uid)
    {
        log::debug!(
            "writer self fallback caller={} uid={} path={}",
            package_name,
            self_uid,
            normalized_path
        );
        effective_caller_uid = self_uid;
        effective_caller_package = package_name.to_string();
    }

    if !effective_caller_package.is_empty() {
        if hub.get_current_caller_package().is_empty() || is_caller_from_mapping_request_owner {
            hub.set_current_caller_package(&effective_caller_package);
        }
        if (hub.get_current_caller_uid() < writer::ANDROID_APP_UID_START
            || is_caller_from_mapping_request_owner)
            && effective_caller_uid >= writer::ANDROID_APP_UID_START
        {
            hub.set_current_caller_uid(effective_caller_uid);
        }
    }

    SystemWriterCallerContext {
        effective_caller_uid,
        effective_caller_package,
        user_id,
        is_caller_from_inferred,
    }
}

fn resolve_explicit_caller_package_by_uid(
    process_package: &str,
    self_uid: i32,
    caller_uid: i32,
    user_id: i32,
) -> Option<String> {
    if caller_uid < writer::ANDROID_APP_UID_START {
        return None;
    }

    let mut packages = redirect_policy::get_packages_for_uid(caller_uid);
    if packages.is_empty() {
        redirect_policy::refresh_shared_uid_cache();
        packages = redirect_policy::get_packages_for_uid(caller_uid);
    }
    if packages.is_empty() {
        return None;
    }

    packages.sort();
    packages.dedup();
    packages.retain(|package| {
        !package.is_empty()
            && package != process_package
            && !redirect_policy::is_system_writer_package(package)
    });
    if packages.is_empty() {
        return None;
    }

    let config_user_id = if user_id >= 0 {
        user_id
    } else {
        platform::user_id_from_uid(caller_uid)
    };
    let mut configured = packages
        .iter()
        .filter(|package| {
            config_user_id >= 0
                && SettingsHub::instance()
                    .get_user_redirect_enablement(package, caller_uid, config_user_id)
                    .is_enabled()
        })
        .cloned()
        .collect::<Vec<_>>();

    if configured.len() == 1 {
        return configured.pop();
    }

    if packages.len() == 1 && caller_uid != self_uid {
        return packages.pop();
    }

    None
}

fn maybe_override_system_writer_caller_by_mapping_request_path(
    normalized_path: &str,
    user_id: i32,
    effective_caller_uid: &mut i32,
    effective_caller_package: &mut String,
    is_caller_from_inferred: &mut bool,
    is_caller_from_mapping_request_owner: &mut bool,
) {
    if user_id < 0 {
        return;
    }

    let inferred = resolve_mapping_request_owner_package_by_path(user_id, normalized_path);
    if inferred.is_empty() {
        return;
    }

    let inferred_uid = redirect_policy::get_fresh_uid_for_package(&inferred);
    if inferred_uid >= writer::ANDROID_APP_UID_START {
        *effective_caller_uid = inferred_uid;
    }
    let previous_package = std::mem::replace(effective_caller_package, inferred);
    log::debug!(
        "writer: mapping request owner caller={} previous={} uid={} path={}",
        effective_caller_package,
        previous_package,
        *effective_caller_uid,
        normalized_path
    );
    *is_caller_from_inferred = true;
    *is_caller_from_mapping_request_owner = true;
}

fn maybe_infer_system_writer_caller_by_redirect_path(
    normalized_path: &str,
    user_id: i32,
    original_caller_uid: i32,
    self_uid: i32,
    effective_caller_uid: &mut i32,
    effective_caller_package: &mut String,
    is_caller_from_inferred: &mut bool,
) {
    if user_id < 0 || !effective_caller_package.is_empty() {
        return;
    }

    let inferred = resolve_redirect_owner_package_by_path(user_id, normalized_path);
    if inferred.is_empty() {
        return;
    }

    let inferred_uid = redirect_policy::get_fresh_uid_for_package(&inferred);
    if original_caller_uid >= writer::ANDROID_APP_UID_START
        && original_caller_uid != self_uid
        && original_caller_uid != inferred_uid
    {
        log::debug!(
            "writer path infer skip explicit_uid uid={} inferred={} inferred_uid={} path={}",
            original_caller_uid,
            inferred,
            inferred_uid,
            normalized_path
        );
        return;
    }

    if inferred_uid >= writer::ANDROID_APP_UID_START {
        *effective_caller_uid = inferred_uid;
    }
    log::debug!(
        "writer: redirect path infer caller={} uid={} path={}",
        inferred,
        *effective_caller_uid,
        normalized_path
    );
    *effective_caller_package = inferred;
    *is_caller_from_inferred = true;
}

fn resolve_redirect_owner_package_by_path(user_id: i32, normalized_path: &str) -> String {
    if user_id < 0 || normalized_path.is_empty() {
        return String::new();
    }

    let synthetic_uid = user_id
        .saturating_mul(platform::ANDROID_USER_ID_OFFSET)
        .saturating_add(writer::ANDROID_APP_UID_START);
    SettingsHub::instance()
        .resolve_redirect_package_by_path_for_user(synthetic_uid, normalized_path)
}

fn resolve_mapping_request_owner_package_by_path(user_id: i32, normalized_path: &str) -> String {
    if user_id < 0 || normalized_path.is_empty() {
        return String::new();
    }

    SettingsHub::instance()
        .resolve_mapping_request_package_by_path_for_user(user_id, normalized_path)
}

fn resolve_read_only_owner_package_by_path(user_id: i32, normalized_path: &str) -> String {
    if user_id < 0 || normalized_path.is_empty() {
        return String::new();
    }

    SettingsHub::instance().resolve_read_only_package_by_path_for_user(user_id, normalized_path)
}

fn should_query_download_owner_for_writer(self_uid: i32, package_name: &str) -> bool {
    if redirect_policy::is_media_provider_package(package_name) {
        return false;
    }
    redirect_policy::get_packages_for_uid(self_uid)
        .iter()
        .any(|pkg| pkg == "com.android.providers.downloads")
}

pub(super) fn is_media_provider_internal_without_caller(
    package_name: &str,
    has_explicit_caller_signal: bool,
    effective_caller_package: &str,
) -> bool {
    redirect_policy::is_media_provider_package(package_name)
        && !has_explicit_caller_signal
        && effective_caller_package.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppProfile, UserProfile};
    use crate::domain::PathMapping;
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard};

    static CALLER_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_caller_test() -> MutexGuard<'static, ()> {
        CALLER_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    #[test]
    fn anonymous_writer_path_can_infer_mapping_request_owner() {
        let _guard = lock_caller_test();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.mobileqq".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec!["/storage/emulated/0/Download".to_string()],
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/QQ".to_string(),
                            "/storage/emulated/0/Download/third-party/QQ".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let mut caller_uid = writer::ANDROID_APP_UID_START;
        let mut caller_package = String::new();
        let mut is_inferred = false;
        let path = "/storage/emulated/0/Download/QQ";
        assert!(has_system_writer_redirect_path_owner_hint(0, path));
        maybe_infer_system_writer_caller_by_redirect_path(
            path,
            0,
            -1,
            10217,
            &mut caller_uid,
            &mut caller_package,
            &mut is_inferred,
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(caller_package, "com.tencent.mobileqq");
        assert!(is_inferred);
    }

    #[test]
    fn anonymous_writer_path_ignores_broad_public_mapping_request_owner() {
        let _guard = lock_caller_test();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.lolm".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Documents".to_string(),
                            "/storage/emulated/0/Android/data/com.tencent.lolm/files".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let mut caller_uid = writer::ANDROID_APP_UID_START;
        let mut caller_package = String::new();
        let mut is_inferred = false;
        let path = "/storage/emulated/0/Documents/MTManager/apks/coolapk.apk";
        assert!(!has_system_writer_redirect_path_owner_hint(0, path));
        maybe_infer_system_writer_caller_by_redirect_path(
            path,
            0,
            -1,
            10217,
            &mut caller_uid,
            &mut caller_package,
            &mut is_inferred,
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(caller_package.is_empty());
        assert!(!is_inferred);
    }

    #[test]
    fn media_provider_self_uid_does_not_block_mapping_request_owner_inference() {
        let _guard = lock_caller_test();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.mm".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/Weixin".to_string(),
                            "/storage/emulated/0/Download/third-party/WeChat".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let self_uid = 10217;
        let mut caller_uid = self_uid;
        let mut caller_package = String::new();
        let mut is_inferred = false;
        let path = "/storage/emulated/0/Download/Weixin/.nomedia";
        maybe_infer_system_writer_caller_by_redirect_path(
            path,
            0,
            caller_uid,
            self_uid,
            &mut caller_uid,
            &mut caller_package,
            &mut is_inferred,
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(caller_package, "com.tencent.mm");
        assert!(is_inferred);
    }

    #[test]
    fn media_provider_self_signal_does_not_block_read_only_owner_inference() {
        let _guard = lock_caller_test();
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "me.fakerqu.test.storageredirect".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec![
                            "/storage/emulated/0/Download/SrtMonitorLocked".to_string(),
                        ],
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = redirect_policy::replace_test_uid_cache(HashMap::from([(
            "me.fakerqu.test.storageredirect".to_string(),
            10192,
        )]));

        let self_uid = 10217;
        let result = resolve_system_writer_caller_context(
            InterceptHub::instance(),
            SystemWriterCallerResolveRequest {
                package_name: "com.android.providers.media.module",
                self_uid,
                is_shared_uid: false,
                normalized_path: "/storage/emulated/0/Download/SrtMonitorLocked/.pending-1783893109-srt_monitor_27_media-read-only-denied.bin",
                is_write_operation: true,
                has_explicit_caller_signal: true,
                can_infer_caller_by_path: true,
                effective_caller_uid: self_uid,
                original_caller_uid: self_uid,
                effective_caller_package: "com.android.providers.media.module".to_string(),
                user_id: 0,
            },
        );

        redirect_policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(
            result.effective_caller_package,
            "me.fakerqu.test.storageredirect"
        );
        assert_eq!(result.effective_caller_uid, 10192);
        assert!(result.is_caller_from_inferred);
    }
}
