use super::*;
use crate::config::{AppProfile, MonitorAppSpec, UserProfile};
use crate::domain::PathMapping;
use crate::platform;
use crate::redirect::policy;
use std::collections::HashMap;

fn profile_enabled(is_enabled: bool) -> UserProfile {
    UserProfile {
        is_enabled,
        is_mapping_mode_only: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
    }
}

fn with_test_app_config<T>(
    package_name: &str,
    uid: i32,
    is_enabled: bool,
    test: impl FnOnce() -> T,
) -> T {
    let user_id = platform::user_id_from_uid(uid);
    let hub = SettingsHub::instance();
    let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
        package_name.to_string(),
        AppProfile {
            user_profiles: HashMap::from([(user_id, profile_enabled(is_enabled))]),
        },
    )]));
    let previous_uid_cache =
        policy::replace_test_uid_cache(HashMap::from([(package_name.to_string(), uid)]));

    let result = test();

    policy::restore_test_uid_cache(
        previous_uid_cache.0,
        previous_uid_cache.1,
        previous_uid_cache.2,
        previous_uid_cache.3,
    );
    hub.restore_test_apps(previous_apps, previous_loaded);
    result
}

#[test]
fn skips_allowed_real_path_events_without_matching_owner_evidence() {
    let identity = MonitorIdentity {
        package_name: "cn.wps.moffice_eng".to_string(),
        identify_method: "daemon_inotify",
        identify_reliability: "medium",
    };

    assert!(should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "allowed_real_path",
        "/storage/emulated/0/Download/file.txt",
        "cn.wps.moffice_eng",
    ));
    assert!(!should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "path_mapping",
        "/storage/emulated/0/Download/file.txt",
        "cn.wps.moffice_eng",
    ));

    let media_owner = MonitorIdentity {
        package_name: "com.android.providers.media.module".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };

    assert!(should_skip_ambiguous_allowed_real_path_event(
        &media_owner,
        "allowed_real_path",
        "/storage/emulated/0/Download/DLManager/thumbs",
        "cn.wps.moffice_eng",
    ));
}

#[test]
fn keeps_allowed_real_path_events_with_owner_evidence() {
    let identity = MonitorIdentity {
        package_name: "info.muge.appshare".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };

    assert!(!should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "allowed_real_path",
        "/storage/emulated/0/Download/file.txt",
        "info.muge.appshare",
    ));
}

#[test]
fn public_root_events_require_owner_identity_for_watch_package() {
    let daemon_identity = MonitorIdentity {
        package_name: "org.srx.demo".to_string(),
        identify_method: "daemon_inotify",
        identify_reliability: "medium",
    };
    assert!(should_skip_public_root_event_identity(
        &daemon_identity,
        "public_root",
        "org.srx.demo"
    ));

    let matching_owner = MonitorIdentity {
        package_name: "org.srx.demo".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };
    assert!(!should_skip_public_root_event_identity(
        &matching_owner,
        "public_root",
        "org.srx.demo"
    ));

    let other_owner = MonitorIdentity {
        package_name: "org.srx.other".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };
    assert!(should_skip_public_root_event_identity(
        &other_owner,
        "public_root",
        "org.srx.demo"
    ));
}

#[test]
fn system_intermediate_owner_identity_is_kept_for_common_watch_sources() {
    let identity = MonitorIdentity {
        package_name: "com.android.providers.downloads".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };

    assert!(!should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "allowed_real_path",
        "/storage/emulated/0/Download/file.txt",
        "org.srx.demo",
    ));
    assert!(!should_skip_ambiguous_read_only_path_event(
        &identity,
        "read_only_path",
        "org.srx.demo",
    ));
    assert!(!should_skip_public_root_event_identity(
        &identity,
        "public_root",
        "org.srx.demo",
    ));
}

#[test]
fn media_provider_owner_identity_is_skipped_for_common_watch_sources() {
    let identity = MonitorIdentity {
        package_name: "com.android.providers.media.module".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };

    assert!(should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "allowed_real_path",
        "/storage/emulated/0/Download/file.txt",
        "xyz.nextalone.nnngram",
    ));
    assert!(should_skip_ambiguous_read_only_path_event(
        &identity,
        "read_only_path",
        "xyz.nextalone.nnngram",
    ));
    assert!(should_skip_public_root_event_identity(
        &identity,
        "public_root",
        "xyz.nextalone.nnngram",
    ));
}

#[test]
fn non_intermediate_owner_identity_still_skips_for_common_watch_sources() {
    let identity = MonitorIdentity {
        package_name: "com.tencent.mobileqq".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };

    assert!(should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "allowed_real_path",
        "/storage/emulated/0/Download/file.txt",
        "org.srx.demo",
    ));
    assert!(should_skip_ambiguous_read_only_path_event(
        &identity,
        "read_only_path",
        "org.srx.demo",
    ));
    assert!(should_skip_public_root_event_identity(
        &identity,
        "public_root",
        "org.srx.demo",
    ));
}

#[test]
fn read_only_path_events_require_owner_identity_for_watch_package() {
    let guessed_identity = MonitorIdentity {
        package_name: "com.tencent.mobileqq".to_string(),
        identify_method: "watch_package",
        identify_reliability: "medium",
    };
    assert!(should_skip_ambiguous_read_only_path_event(
        &guessed_identity,
        "read_only_path",
        "com.tencent.mobileqq"
    ));
    assert!(!should_skip_ambiguous_read_only_path_event(
        &guessed_identity,
        "path_mapping",
        "com.tencent.mobileqq"
    ));

    let fallback_identity = MonitorIdentity {
        package_name: "com.tencent.mobileqq".to_string(),
        identify_method: "daemon_inotify",
        identify_reliability: "medium",
    };
    assert!(should_skip_ambiguous_read_only_path_event(
        &fallback_identity,
        "read_only_path",
        "com.tencent.mobileqq"
    ));

    let matching_owner = MonitorIdentity {
        package_name: "com.tencent.mobileqq".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };
    assert!(!should_skip_ambiguous_read_only_path_event(
        &matching_owner,
        "read_only_path",
        "com.tencent.mobileqq"
    ));

    let other_owner = MonitorIdentity {
        package_name: "xyz.nextalone.nnngram".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };
    assert!(should_skip_ambiguous_read_only_path_event(
        &other_owner,
        "read_only_path",
        "com.tencent.mobileqq"
    ));
}

#[test]
fn skips_allowed_real_path_diagnostic_and_mismatched_owner_events() {
    let identity = MonitorIdentity {
        package_name: "com.android.providers.media.module".to_string(),
        identify_method: "owner_uid",
        identify_reliability: "high",
    };

    assert!(should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "allowed_real_path",
        "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz",
        "org.srx.manager",
    ));
    assert!(!should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "path_mapping",
        "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz",
        "org.srx.manager",
    ));
    assert!(should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "allowed_real_path",
        "/storage/emulated/0/Download/storage-redirect-x-backup-20260613.srxbak.zip",
        "org.srx.manager",
    ));
}

#[test]
fn keeps_media_provider_fallback_records_for_ambiguous_saf_writes() {
    let identity = MonitorIdentity {
        package_name: "com.android.providers.media.module".to_string(),
        identify_method: "media_provider_fallback",
        identify_reliability: "fallback",
    };

    assert!(!should_skip_ambiguous_allowed_real_path_event(
        &identity,
        "allowed_real_path",
        "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz",
        "org.srx.manager",
    ));
    assert!(!should_skip_ambiguous_read_only_path_event(
        &identity,
        "read_only_path",
        "cn.wps.moffice_eng",
    ));
    assert!(!should_skip_public_root_event_identity(
        &identity,
        "public_root",
        "cn.wps.moffice_eng",
    ));
}

#[test]
fn builds_watch_roots_for_redirect_allowed_and_mapping_sources() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.demo".to_string(),
        user_id: 0,
        is_enabled: true,
        is_mapping_mode_only: false,
        allowed_real_paths: vec!["Download/Public".to_string()],
        excluded_real_paths: vec!["/storage/emulated/0/Download/Public/tmp".to_string()],
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: vec![PathMapping::new(
            "/storage/emulated/0/DCIM/Demo".to_string(),
            "/storage/emulated/0/Pictures/Demo".to_string(),
        )],
    };

    let roots = build_watch_roots(&spec);
    assert_eq!(roots.len(), 3);
    assert_eq!(roots[0].source, "redirect_root");
    assert_eq!(
        roots[0].backend_root,
        "/data/media/0/Android/data/org.srx.demo/sdcard"
    );
    assert_eq!(roots[0].display_root, "/storage/emulated/0");

    assert_eq!(roots[1].source, "allowed_real_path");
    assert_eq!(roots[1].backend_root, "/data/media/0/Download/Public");
    assert_eq!(
        roots[1].excluded_roots,
        vec!["/storage/emulated/0/Download/Public/tmp".to_string()]
    );

    assert_eq!(roots[2].source, "path_mapping");
    assert_eq!(roots[2].record_from_root, "/storage/emulated/0/DCIM/Demo");
    assert_eq!(roots[2].display_root, "/storage/emulated/0/Pictures/Demo");
    assert_eq!(
        roots[2].record_display_root,
        "/storage/emulated/0/Pictures/Demo"
    );
    assert_eq!(roots[2].backend_root, "/data/media/0/Pictures/Demo");
}

#[test]
fn disabled_profile_public_root_watches_storage_root() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.disabled".to_string(),
        user_id: 0,
        is_enabled: false,
        is_mapping_mode_only: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
    };

    let roots = build_watch_roots(&spec);

    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].source, "public_root");
    assert_eq!(roots[0].backend_root, "/data/media/0");
    assert_eq!(roots[0].display_root, "/storage/emulated/0");
    assert_eq!(roots[0].record_display_root, "/storage/emulated/0");
}

#[test]
fn build_watch_roots_skips_path_mapping_android_private_targets() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.demo".to_string(),
        user_id: 0,
        is_enabled: true,
        is_mapping_mode_only: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: vec![
            PathMapping::new(
                "Download/App".to_string(),
                "Android/data/com.example/files".to_string(),
            ),
            PathMapping::new(
                "Download/Obb".to_string(),
                "Android/obb/com.example".to_string(),
            ),
            PathMapping::new(
                "Download/Media".to_string(),
                "Android/media/com.example/files".to_string(),
            ),
        ],
    };

    let roots = build_watch_roots(&spec);
    let mapping_roots: Vec<_> = roots
        .iter()
        .filter(|root| root.source == "path_mapping")
        .collect();

    assert_eq!(mapping_roots.len(), 1);
    assert_eq!(
        mapping_roots[0].record_from_root,
        "/storage/emulated/0/Download/Media"
    );
    assert_eq!(
        mapping_roots[0].display_root,
        "/storage/emulated/0/Android/media/com.example/files"
    );
    assert_eq!(
        mapping_roots[0].backend_root,
        "/data/media/0/Android/media/com.example/files"
    );
}

#[test]
fn allowed_watch_roots_dedup_excludes_case_insensitive_deepest_first() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.demo".to_string(),
        user_id: 0,
        is_enabled: true,
        is_mapping_mode_only: false,
        allowed_real_paths: vec!["Download/Public".to_string()],
        excluded_real_paths: vec![
            "/storage/emulated/0/Download/Public".to_string(),
            "/storage/emulated/0/download/public".to_string(),
            "/storage/emulated/0/Download/Public/tmp".to_string(),
            "/storage/emulated/0/Download/Public/*".to_string(),
        ],
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
    };

    let roots = build_watch_roots(&spec);

    assert_eq!(roots.len(), 2);
    assert_eq!(
        roots[1].excluded_roots,
        vec![
            "/storage/emulated/0/Download/Public/tmp".to_string(),
            "/storage/emulated/0/Download/Public".to_string(),
        ]
    );
}

#[test]
fn allowed_watch_roots_resolve_relative_excludes() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.demo".to_string(),
        user_id: 10,
        is_enabled: true,
        is_mapping_mode_only: false,
        allowed_real_paths: vec!["Download/Public".to_string()],
        excluded_real_paths: vec!["Download/Public/tmp".to_string()],
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
    };

    let roots = build_watch_roots(&spec);

    assert_eq!(roots[1].source, "allowed_real_path");
    assert_eq!(
        roots[1].excluded_roots,
        vec!["/storage/emulated/10/Download/Public/tmp".to_string()]
    );
}

#[test]
fn builds_watch_roots_for_read_only_paths() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.demo".to_string(),
        user_id: 0,
        is_enabled: true,
        is_mapping_mode_only: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: vec!["/storage/emulated/0/Download/tmp".to_string()],
        sandboxed_paths: Vec::new(),
        read_only_paths: vec!["Download".to_string()],
        path_mappings: Vec::new(),
    };

    let roots = build_watch_roots(&spec);

    assert_eq!(roots.len(), 2);
    assert_eq!(roots[1].source, "read_only_path");
    assert_eq!(roots[1].backend_root, "/data/media/0/Download");
    assert_eq!(roots[1].display_root, "/storage/emulated/0/Download");
    assert_eq!(roots[1].record_display_root, "/storage/emulated/0/Download");
    assert_eq!(roots[1].record_from_root, "");
    assert_eq!(
        roots[1].excluded_roots,
        vec!["/storage/emulated/0/Download/tmp".to_string()]
    );
}

#[test]
fn read_only_watch_roots_resolve_relative_excludes() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.demo".to_string(),
        user_id: 10,
        is_enabled: true,
        is_mapping_mode_only: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: vec!["Download/system-tmp".to_string()],
        sandboxed_paths: Vec::new(),
        read_only_paths: vec![
            "Download".to_string(),
            "!Download/tmp".to_string(),
            "!Download/Nested/tmp".to_string(),
        ],
        path_mappings: Vec::new(),
    };

    let roots = build_watch_roots(&spec);

    assert_eq!(roots[1].source, "read_only_path");
    assert_eq!(
        roots[1].excluded_roots,
        vec![
            "/storage/emulated/10/Download/Nested/tmp".to_string(),
            "/storage/emulated/10/Download/system-tmp".to_string(),
            "/storage/emulated/10/Download/tmp".to_string(),
        ]
    );
}

#[test]
fn read_only_watch_root_wins_duplicate_allowed_root() {
    let mut roots = vec![
        WatchRoot {
            package_name: "org.srx.demo".to_string(),
            backend_root: "/data/media/0/Download".to_string(),
            display_root: "/storage/emulated/0/Download".to_string(),
            record_display_root: "/storage/emulated/0/Download".to_string(),
            record_from_root: String::new(),
            excluded_roots: Vec::new(),
            source: "allowed_real_path",
        },
        WatchRoot {
            package_name: "org.srx.demo".to_string(),
            backend_root: "/data/media/0/Download".to_string(),
            display_root: "/storage/emulated/0/Download".to_string(),
            record_display_root: "/storage/emulated/0/Download".to_string(),
            record_from_root: String::new(),
            excluded_roots: Vec::new(),
            source: "read_only_path",
        },
    ];

    dedup_roots(&mut roots);

    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].source, "read_only_path");
}

#[test]
fn prioritizes_mapping_roots_before_large_redirect_roots() {
    let mut roots = vec![
        WatchRoot {
            package_name: "org.srx.demo".to_string(),
            backend_root: "/data/media/0/Android/data/org.srx.demo/sdcard".to_string(),
            display_root: "/storage/emulated/0".to_string(),
            record_display_root: "/storage/emulated/0".to_string(),
            record_from_root: String::new(),
            excluded_roots: Vec::new(),
            source: "redirect_root",
        },
        WatchRoot {
            package_name: "org.srx.demo".to_string(),
            backend_root: "/data/media/0/Download/ThirdParty/QQ".to_string(),
            display_root: "/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
            record_display_root: "/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
            record_from_root: "/storage/emulated/0/Download/QQ".to_string(),
            excluded_roots: Vec::new(),
            source: "path_mapping",
        },
        WatchRoot {
            package_name: "org.srx.demo".to_string(),
            backend_root: "/data/media/0/Android/data/org.srx.demo/sdcard/Tasks".to_string(),
            display_root: "/storage/emulated/0/Tasks".to_string(),
            record_display_root: "/storage/emulated/0/Tasks".to_string(),
            record_from_root: String::new(),
            excluded_roots: Vec::new(),
            source: "sandbox_path",
        },
    ];

    sort_roots_by_monitor_priority(&mut roots);

    assert_eq!(roots[0].source, "path_mapping");
    assert_eq!(roots[1].source, "sandbox_path");
    assert_eq!(roots[2].source, "redirect_root");
}

#[test]
fn builds_private_owner_repair_roots_for_enabled_apps() {
    let spec = MonitorAppSpec {
        package_name: "com.tencent.mm".to_string(),
        user_id: 0,
        is_enabled: true,
        is_mapping_mode_only: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
    };

    let roots = build_private_owner_repair_roots(&spec);

    assert_eq!(roots.len(), 3);
    assert!(roots.iter().all(|root| root.source == "private_owner"));
    assert!(roots.iter().any(|root| {
        root.backend_root == "/data/media/0/Android/data/com.tencent.mm"
            && root.display_root == "/storage/emulated/0/Android/data/com.tencent.mm"
    }));
    assert!(roots.iter().any(|root| {
        root.backend_root == "/data/media/0/Android/media/com.tencent.mm"
            && root.display_root == "/storage/emulated/0/Android/media/com.tencent.mm"
    }));
}

#[test]
fn private_owner_repair_roots_are_prioritized_before_broad_allowed_roots() {
    let mut roots = vec![
        WatchRoot {
            package_name: "com.tencent.mm".to_string(),
            backend_root: "/data/media/0/Android".to_string(),
            display_root: "/storage/emulated/0/Android".to_string(),
            record_display_root: "/storage/emulated/0/Android".to_string(),
            record_from_root: String::new(),
            excluded_roots: Vec::new(),
            source: "allowed_real_path",
        },
        WatchRoot {
            package_name: "com.tencent.mm".to_string(),
            backend_root: "/data/media/0/Android/data/com.tencent.mm".to_string(),
            display_root: "/storage/emulated/0/Android/data/com.tencent.mm".to_string(),
            record_display_root: "/storage/emulated/0/Android/data/com.tencent.mm".to_string(),
            record_from_root: String::new(),
            excluded_roots: Vec::new(),
            source: "private_owner",
        },
    ];

    sort_roots_by_monitor_priority(&mut roots);

    assert_eq!(roots[0].source, "private_owner");
    assert_eq!(roots[1].source, "allowed_real_path");
}

#[test]
fn android_media_private_owner_roots_are_expanded_first() {
    let mut roots = vec![
        WatchRoot {
            package_name: "com.tencent.mm".to_string(),
            backend_root: "/data/media/0/Android/data/com.tencent.mm".to_string(),
            display_root: "/storage/emulated/0/Android/data/com.tencent.mm".to_string(),
            record_display_root: "/storage/emulated/0/Android/data/com.tencent.mm".to_string(),
            record_from_root: String::new(),
            excluded_roots: Vec::new(),
            source: "private_owner",
        },
        WatchRoot {
            package_name: "com.eg.android.AlipayGphone".to_string(),
            backend_root: "/data/media/0/Android/media/com.eg.android.AlipayGphone".to_string(),
            display_root: "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone"
                .to_string(),
            record_display_root: "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone"
                .to_string(),
            record_from_root: String::new(),
            excluded_roots: Vec::new(),
            source: "private_owner",
        },
        WatchRoot {
            package_name: "com.tencent.mm".to_string(),
            backend_root: "/data/media/0/Download/第三方下载/微信".to_string(),
            display_root: "/storage/emulated/0/Download/第三方下载/微信".to_string(),
            record_display_root: "/storage/emulated/0/Download/第三方下载/微信".to_string(),
            record_from_root: "/storage/emulated/0/Download/Weixin".to_string(),
            excluded_roots: Vec::new(),
            source: "path_mapping",
        },
    ];

    sort_roots_by_monitor_priority(&mut roots);

    assert_eq!(
        roots[0].display_root,
        "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone"
    );
    assert_eq!(roots[1].source, "private_owner");
    assert_eq!(roots[2].source, "path_mapping");
}

#[test]
fn disabled_apps_do_not_add_private_owner_repair_roots() {
    let spec = MonitorAppSpec {
        package_name: "bin.mt.plus".to_string(),
        user_id: 0,
        is_enabled: false,
        is_mapping_mode_only: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
    };

    assert!(build_private_owner_repair_roots(&spec).is_empty());
}

#[test]
fn path_mapping_priority_is_high_value() {
    assert!(is_high_value_monitor_source("path_mapping"));
    assert!(is_high_value_monitor_source("sandbox_path"));
    assert!(is_high_value_monitor_source("private_owner"));
    assert!(!is_high_value_monitor_source("redirect_root"));
    assert!(!is_high_value_monitor_source("allowed_real_path"));
}

#[test]
fn capacity_limited_monitor_does_not_retry_missing_roots() {
    let mut monitor = RegularAppMonitor::new();
    monitor.missing_roots = 3;
    monitor.last_rebuild_ms = 0;

    assert!(monitor.should_retry_missing_roots());

    monitor.capacity_limited = true;

    assert!(!monitor.should_retry_missing_roots());
}

#[test]
fn unchanged_config_retries_missing_roots_without_full_reset() {
    let hub = SettingsHub::new();
    let previous_monitor = hub.replace_test_file_monitor_enabled(true);
    let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
        "org.srx.demo".to_string(),
        AppProfile {
            user_profiles: HashMap::from([(0, profile_enabled(true))]),
        },
    )]));
    let version = hub.config_version();
    let mut monitor = RegularAppMonitor::new();
    monitor.config_version = version;
    monitor.needs_rebuild = false;
    monitor.missing_roots = 1;
    monitor.missing_watch_roots.push(WatchRoot {
        package_name: "org.srx.demo".to_string(),
        backend_root: "/definitely/missing/srx/demo".to_string(),
        display_root: "/storage/emulated/0/definitely/missing/srx/demo".to_string(),
        record_display_root: "/storage/emulated/0/definitely/missing/srx/demo".to_string(),
        record_from_root: String::new(),
        excluded_roots: Vec::new(),
        source: "path_mapping",
    });
    monitor.last_rebuild_ms = 0;
    monitor.watch_nodes.insert(
        42,
        vec![WatchNode {
            package_name: "org.srx.demo".to_string(),
            backend_dir: "/data/media/0".to_string(),
            display_dir: "/storage/emulated/0".to_string(),
            record_display_root: "/storage/emulated/0".to_string(),
            record_from_root: String::new(),
            excluded_roots: Vec::new(),
            source: "redirect_root",
        }],
    );

    monitor.reconfigure(&hub, false);

    hub.restore_test_apps(previous_apps, previous_loaded);
    hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);

    assert!(monitor.watch_nodes.contains_key(&42));
    assert_eq!(monitor.missing_roots, 1);
}

#[test]
fn fallback_parent_watch_only_descends_toward_record_root() {
    let node = WatchNode {
        package_name: "com.tencent.mobileqq".to_string(),
        backend_dir: "/data/media/0/Download".to_string(),
        display_dir: "/storage/emulated/0/Download".to_string(),
        record_display_root: "/storage/emulated/0/Download/第三方下载/QQ".to_string(),
        record_from_root: "/storage/emulated/0/Download/QQ".to_string(),
        excluded_roots: Vec::new(),
        source: "path_mapping",
    };

    assert!(should_descend_into_child(
        &node,
        "/storage/emulated/0/Download/第三方下载"
    ));
    assert!(!should_descend_into_child(
        &node,
        "/storage/emulated/0/Download/Camera"
    ));
    assert!(!should_record_display_path(
        "/storage/emulated/0/Download/第三方下载",
        &node.record_display_root
    ));
    assert!(should_record_display_path(
        "/storage/emulated/0/Download/第三方下载/QQ/a.jpg",
        &node.record_display_root
    ));
}

#[test]
fn fallback_parent_watch_maps_records_to_request_root() {
    let from_path = map_record_from_path(
        "/storage/emulated/0/Download/第三方下载/QQ/a.jpg",
        "/storage/emulated/0/Download/第三方下载/QQ",
        "/storage/emulated/0/Download/QQ",
    );

    assert_eq!(from_path, "/storage/emulated/0/Download/QQ/a.jpg");
    assert!(
        map_record_from_path(
            "/storage/emulated/0/Download/第三方下载",
            "/storage/emulated/0/Download/第三方下载/QQ",
            "/storage/emulated/0/Download/QQ",
        )
        .is_empty()
    );
}

#[test]
fn fallback_parent_watch_aligns_backend_and_display_ancestors() {
    let root = WatchRoot {
        package_name: "com.tencent.mobileqq".to_string(),
        backend_root: "/data/media/0/Download/第三方下载/QQ".to_string(),
        display_root: "/storage/emulated/0/Download/第三方下载/QQ".to_string(),
        record_display_root: "/storage/emulated/0/Download/第三方下载/QQ".to_string(),
        record_from_root: "/storage/emulated/0/Download/QQ".to_string(),
        excluded_roots: Vec::new(),
        source: "path_mapping",
    };

    assert_eq!(
        align_display_dir_to_backend_ancestor(&root, "/data/media/0/Download").as_deref(),
        Some("/storage/emulated/0/Download")
    );
    assert_eq!(
        align_display_dir_to_backend_ancestor(&root, "/data/media/0/Download/第三方下载")
            .as_deref(),
        Some("/storage/emulated/0/Download/第三方下载")
    );
}

#[test]
fn builds_watch_roots_for_map_only_sandbox_paths() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.demo".to_string(),
        user_id: 10,
        is_enabled: true,
        is_mapping_mode_only: true,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: Vec::new(),
        sandboxed_paths: vec!["Download/Tasks".to_string()],
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
    };

    let roots = build_watch_roots(&spec);
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].source, "sandbox_path");
    assert_eq!(
        roots[0].display_root,
        "/storage/emulated/10/Android/data/org.srx.demo/sdcard/Download/Tasks"
    );
    assert_eq!(
        roots[0].record_display_root,
        "/storage/emulated/10/Android/data/org.srx.demo/sdcard/Download/Tasks"
    );
    assert_eq!(
        roots[0].record_from_root,
        "/storage/emulated/10/Download/Tasks"
    );
    assert_eq!(
        roots[0].backend_root,
        "/data/media/10/Android/data/org.srx.demo/sdcard/Download/Tasks"
    );
}

#[test]
fn disabled_profiles_keep_explicit_monitor_roots_without_redirect_root() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.demo".to_string(),
        user_id: 0,
        is_enabled: false,
        is_mapping_mode_only: false,
        allowed_real_paths: vec!["Download/Public".to_string()],
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: vec!["Download/Locked".to_string()],
        path_mappings: vec![PathMapping::new(
            "/storage/emulated/0/Download/From".to_string(),
            "/storage/emulated/0/Download/To".to_string(),
        )],
    };

    let roots = build_watch_roots(&spec);
    assert_eq!(roots.len(), 4);
    assert_eq!(roots[0].source, "public_root");
    assert_eq!(roots[0].backend_root, "/data/media/0");
    assert_eq!(roots[1].source, "allowed_real_path");
    assert_eq!(roots[2].source, "read_only_path");
    assert_eq!(roots[3].source, "path_mapping");
    assert!(roots.iter().all(|root| root.source != "redirect_root"));
}

#[test]
fn disabled_profiles_monitor_public_storage_without_explicit_rules() {
    let spec = MonitorAppSpec {
        package_name: "org.srx.demo".to_string(),
        user_id: 0,
        is_enabled: false,
        is_mapping_mode_only: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
    };

    let roots = build_watch_roots(&spec);
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].source, "public_root");
    assert_eq!(roots[0].backend_root, "/data/media/0");
    assert_eq!(roots[0].display_root, "/storage/emulated/0");
    assert_eq!(roots[0].record_display_root, "/storage/emulated/0");
}

#[test]
fn sandbox_path_events_use_landing_path_and_request_from_path() {
    let node = WatchNode {
        package_name: "com.android.browser".to_string(),
        backend_dir: "/data/media/0/Android/data/com.android.browser/sdcard/Download/DLManager"
            .to_string(),
        display_dir:
            "/storage/emulated/0/Android/data/com.android.browser/sdcard/Download/DLManager"
                .to_string(),
        record_display_root:
            "/storage/emulated/0/Android/data/com.android.browser/sdcard/Download/DLManager"
                .to_string(),
        record_from_root: "/storage/emulated/0/Download/DLManager".to_string(),
        excluded_roots: Vec::new(),
        source: "sandbox_path",
    };

    let paths = MonitorEventPaths::from_node(&node, "thumbs");

    assert_eq!(
        paths.backend_path,
        "/data/media/0/Android/data/com.android.browser/sdcard/Download/DLManager/thumbs"
    );
    assert_eq!(
        paths.display_path,
        "/storage/emulated/0/Android/data/com.android.browser/sdcard/Download/DLManager/thumbs"
    );
    assert_eq!(
        paths.from_path,
        "/storage/emulated/0/Download/DLManager/thumbs"
    );
    assert!(should_record_display_path(
        &paths.display_path,
        &node.record_display_root
    ));
}

#[test]
fn monitor_event_paths_keep_empty_from_root() {
    let node = WatchNode {
        package_name: "org.srx.demo".to_string(),
        backend_dir: "/data/media/0/Download".to_string(),
        display_dir: "/storage/emulated/0/Download".to_string(),
        record_display_root: "/storage/emulated/0/Download".to_string(),
        record_from_root: String::new(),
        excluded_roots: Vec::new(),
        source: "allowed_real_path",
    };

    let paths = MonitorEventPaths::from_node(&node, "file.txt");
    assert_eq!(paths.backend_path, "/data/media/0/Download/file.txt");
    assert_eq!(paths.display_path, "/storage/emulated/0/Download/file.txt");
    assert!(paths.from_path.is_empty());
}

#[test]
fn monitor_event_paths_join_mapping_from_root() {
    let node = WatchNode {
        package_name: "org.srx.demo".to_string(),
        backend_dir: "/data/media/0/Pictures/Demo".to_string(),
        display_dir: "/storage/emulated/0/Pictures/Demo".to_string(),
        record_display_root: "/storage/emulated/0/Pictures/Demo".to_string(),
        record_from_root: "/storage/emulated/0/DCIM/Demo".to_string(),
        excluded_roots: Vec::new(),
        source: "path_mapping",
    };

    let paths = MonitorEventPaths::from_node(&node, "file.jpg");
    assert_eq!(paths.backend_path, "/data/media/0/Pictures/Demo/file.jpg");
    assert_eq!(
        paths.display_path,
        "/storage/emulated/0/Pictures/Demo/file.jpg"
    );
    assert_eq!(paths.from_path, "/storage/emulated/0/DCIM/Demo/file.jpg");
}

#[test]
fn close_write_is_observed_but_hidden_from_monitor_records() {
    assert!(inotify::is_relevant_event(libc::IN_CLOSE_WRITE));
    assert_eq!(
        monitor_operation_from_mask(libc::IN_CLOSE_WRITE),
        "close_write"
    );
    assert!(!should_emit_monitor_operation("close_write"));
    assert!(should_emit_monitor_operation("inotify"));
}

#[test]
fn configured_watch_sources_prefer_watch_package_for_system_writers() {
    assert!(should_prefer_watch_package_for_system_writer_owner(
        "path_mapping",
        "com.android.providers.media.module"
    ));
    assert!(!should_prefer_watch_package_for_system_writer_owner(
        "read_only_path",
        "com.android.providers.media.module"
    ));
    assert!(should_prefer_watch_package_for_system_writer_owner(
        "redirect_root",
        "com.android.providers.media.module"
    ));
    assert!(!should_prefer_watch_package_for_system_writer_owner(
        "allowed_real_path",
        "com.android.providers.media.module"
    ));
    assert!(!should_prefer_watch_package_for_system_writer_owner(
        "path_mapping",
        "com.tencent.mobileqq"
    ));
}

#[test]
fn private_owner_repair_scope_stays_in_package_tree() {
    let root = paths::android_private_data_media_root(
        "/storage/emulated/0/Android/media/xyz.nextalone.nnngram/Nnngram/a.jpg",
        "xyz.nextalone.nnngram",
        0,
    );

    assert_eq!(
        root.as_deref(),
        Some("/data/media/0/Android/media/xyz.nextalone.nnngram")
    );
    assert!(path_is_same_or_child(
        "/data/media/0/Android/media/xyz.nextalone.nnngram/Nnngram",
        root.as_deref().unwrap()
    ));
    assert!(!path_is_same_or_child(
        "/data/media/0/Android/media/xyz.nextalone.nnngram2",
        root.as_deref().unwrap()
    ));
}

#[test]
fn private_owner_repair_scope_skips_non_private_paths() {
    assert!(
        paths::android_private_data_media_root(
            "/storage/emulated/0/Download/Nnngram/a.jpg",
            "Nnngram",
            0
        )
        .is_none()
    );
    assert!(
        paths::android_private_data_media_root(
            "/storage/emulated/0/Android/media/xyz.nextalone.nnngram/a.jpg",
            "com.other.app",
            0,
        )
        .is_none()
    );
}

#[test]
fn private_owner_repair_scope_skips_unmanaged_android_data_owner() {
    with_test_app_config("com.xiaomi.market", 10155, false, || {
        assert!(
            android_private_owner_repair_scope(
                "/storage/emulated/0/Android/data/com.xiaomi.market/files/apk/com.aliyun.tongyi"
            )
            .is_none()
        );
    });
}

#[test]
fn private_owner_repair_scope_allows_enabled_android_data_owner() {
    with_test_app_config("com.example.enabled", 10321, true, || {
        let scope = android_private_owner_repair_scope(
            "/storage/emulated/0/Android/data/com.example.enabled/files/a.txt",
        )
        .expect("enabled private owner should be repairable");

        assert_eq!(scope.owner_package, "com.example.enabled");
        assert_eq!(scope.owner_uid, 10321);
        assert_eq!(
            scope.backend_root,
            "/data/media/0/Android/data/com.example.enabled"
        );
    });
}
