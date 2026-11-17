use super::{AttributionBoundary, RequestIdentity, attribution_boundary, native_name};
use super::{
    parse_proc_uids, request_uid_is_task_credential, resolve_request_package,
    unique_request_package,
};

#[test]
fn executor_names_preserve_utf8_punctuation_and_byte_limits() {
    for name in ["复制+worker", "a@b", "worker!", "worker?", "|ret=0\r\n%7C"] {
        assert_eq!(
            native_name(&format!("/data/local/tmp/{name}\0ignored\0")),
            name
        );
    }
    assert_eq!(
        native_name(&format!("{}界", "a".repeat(254))),
        "a".repeat(254)
    );
    assert_eq!(native_name(&"界".repeat(100)), "界".repeat(85));
    let task = RequestIdentity {
        native: native_name("worker!\0"),
        ..Default::default()
    };
    let changed = RequestIdentity {
        native: native_name("worker?\0"),
        ..task.clone()
    };
    assert!(!task.same_task(&changed));
}

#[test]
fn deferred_task_validation_closes_changed_or_missing_tasks_without_losing_raw_evidence() {
    let captured = RequestIdentity {
        uid: 10123,
        pid: 42,
        start_time: 55,
        native: "original".into(),
        selinux_context: "u:r:untrusted_app:s0".into(),
        package: "test.app".into(),
        ..RequestIdentity::default()
    };
    let mut unchanged = captured.clone();
    unchanged.revalidate(Some(&captured));
    assert_eq!(unchanged, captured);
    for changed in [
        None,
        Some(RequestIdentity {
            start_time: 56,
            ..captured.clone()
        }),
        Some(RequestIdentity {
            selinux_context: "u:r:ksu:s0".into(),
            boundary: AttributionBoundary::Privileged,
            ..captured.clone()
        }),
        Some(RequestIdentity {
            uid: 10124,
            ..captured.clone()
        }),
        Some(RequestIdentity {
            native: "replacement".into(),
            ..captured.clone()
        }),
    ] {
        let mut restored = captured.clone();
        restored.revalidate(changed.as_ref());
        assert_eq!(restored.boundary, AttributionBoundary::Unverified);
        assert!(restored.package.is_empty());
        assert_eq!(restored.start_time, captured.start_time);
        assert_eq!(restored.selinux_context, captured.selinux_context);
        assert_eq!(restored.native, captured.native);
    }
    for boundary in [
        AttributionBoundary::Root,
        AttributionBoundary::Shell,
        AttributionBoundary::Privileged,
        AttributionBoundary::Unverified,
    ] {
        let mut terminal = RequestIdentity {
            boundary,
            ..captured.clone()
        };
        let original = terminal.clone();
        terminal.revalidate(None);
        assert_eq!(terminal, original);
    }
}

#[test]
fn monitor_labels_distinguish_verified_executors_groups_and_unverified_tasks() {
    assert_eq!(
        RequestIdentity::unresolved(0, 42).monitor_identity(),
        ("root", "executor", "high")
    );
    assert_eq!(
        RequestIdentity::unresolved(2000, 42).monitor_identity(),
        ("shell", "executor", "high")
    );
    assert_eq!(
        RequestIdentity::unresolved(10123, 42).monitor_identity(),
        ("-", "unknown", "none")
    );
    let mut identity = RequestIdentity {
        native: "sh".into(),
        shared_user_id: "android.uid.test".into(),
        ..RequestIdentity::default()
    };
    assert_eq!(
        identity.monitor_identity(),
        ("android.uid.test", "shared_uid", "fallback")
    );
    identity.package = "test.member".into();
    assert_eq!(
        identity.monitor_identity(),
        ("test.member", "caller", "high")
    );
    identity.boundary = AttributionBoundary::Privileged;
    assert_eq!(identity.monitor_identity(), ("sh", "executor", "high"));
}

#[test]
fn task_fields_replace_old_metadata_without_losing_operation_results() {
    let identity = RequestIdentity::unresolved(2000, 42);
    let result = identity.with_log_fields(
        "ret=-1|errno=13|caller_uid=10123|caller_pid=2|native=old|caller_boundary=|caller_uid=1",
    );
    assert!(result.starts_with("ret=-1|errno=13|"));
    assert!(result.contains("caller_uid=2000|caller_pid=42|"));
    assert_eq!(result.matches("caller_uid=").count(), 1);
    assert_eq!(result.matches("native=").count(), 1);
}

#[test]
fn privileged_boundaries_apply_before_app_fsuid_attribution() {
    for uids in [
        [0, 0, 0, 10123],
        [10123, 0, 10123, 10123],
        [10123, 10123, 0, 10123],
        [10123, 10123, 10123, 0],
    ] {
        assert_eq!(
            attribution_boundary(&uids, "u:r:untrusted_app:s0"),
            AttributionBoundary::Root
        );
    }
    for domain in ["su", "magisk", "ksu", "kernelsu"] {
        assert_eq!(
            attribution_boundary(&[10123; 4], &format!("u:r:{domain}:s0")),
            AttributionBoundary::Privileged
        );
    }
    assert_eq!(
        attribution_boundary(&[2000; 4], "u:r:shell:s0"),
        AttributionBoundary::Shell
    );
    assert_eq!(
        attribution_boundary(&[10123; 4], "u:r:shell:s0"),
        AttributionBoundary::Shell
    );
}

#[test]
fn ordinary_native_children_and_nonprivileged_domain_changes_remain_attributable() {
    for context in [
        "u:r:untrusted_app:s0:c1,c2",
        "u:r:isolated_app:s0",
        "u:r:system_app:s0",
        "u:r:super_app:s0",
    ] {
        assert_eq!(
            attribution_boundary(&[10123; 4], context),
            AttributionBoundary::None
        );
    }
    assert_eq!(native_name("/system/bin/sh\0-c\0com.other.app\0"), "sh");
    assert_eq!(
        attribution_boundary(&[10123; 4], ""),
        AttributionBoundary::Unverified
    );
}

#[test]
fn cached_identity_rejects_pid_reuse_exec_credential_and_domain_changes() {
    let identity = RequestIdentity {
        uid: 10123,
        pid: 31,
        start_time: 42,
        native: "sh".into(),
        selinux_context: "u:r:untrusted_app:s0".into(),
        ..RequestIdentity::default()
    };
    assert!(identity.same_task(&identity));
    for changed in [
        RequestIdentity {
            start_time: 43,
            ..identity.clone()
        },
        RequestIdentity {
            uid: 10124,
            ..identity.clone()
        },
        RequestIdentity {
            native: "other".into(),
            ..identity.clone()
        },
        RequestIdentity {
            selinux_context: "u:r:su:s0".into(),
            ..identity.clone()
        },
        RequestIdentity {
            boundary: AttributionBoundary::Root,
            ..identity.clone()
        },
    ] {
        assert!(!identity.same_task(&changed));
    }
}

#[test]
fn native_arguments_do_not_select_a_package() {
    assert_eq!(
        resolve_request_package("/system/bin/sh\0com.other.app\0", 10123, |_| 10123),
        None
    );
    assert_eq!(
        resolve_request_package("helper\0@com.other.app-1\0", 10123, |_| 10123),
        None
    );
}

#[test]
fn native_request_uses_only_the_uids_unique_installed_package() {
    let packages = |names: &[&str]| names.iter().map(|name| name.to_string()).collect();
    assert_eq!(
        unique_request_package(packages(&["com.android.shell"])),
        Some("com.android.shell".into())
    );
    assert_eq!(unique_request_package(packages(&[])), None);
    assert_eq!(
        unique_request_package(packages(&["test.first", "test.second"])),
        None
    );
    assert_eq!(unique_request_package(packages(&["mkdir"])), None);
}

#[test]
fn proc_status_credentials_keep_fsuid_for_operation_matching() {
    let credentials = parse_proc_uids("Name:\ttest\nUid:\t10123\t10123\t0\t10124\n").unwrap();
    assert!(request_uid_is_task_credential(&credentials, 10123));
    assert!(request_uid_is_task_credential(&credentials, 0));
    assert!(request_uid_is_task_credential(&credentials, 10124));
    assert!(!request_uid_is_task_credential(&credentials, 10125));
}

#[test]
fn malformed_proc_uid_rows_are_rejected() {
    assert!(parse_proc_uids("Uid:\t10123 10123 10123\n").is_none());
    assert!(parse_proc_uids("Uid:\t10123 10123 10123 bad\n").is_none());
}

#[test]
fn request_package_must_match_the_uid_used_by_that_operation() {
    let credentials = parse_proc_uids("Uid:\t10123\t10123\t0\t10124\n").unwrap();
    let package_uid = |package: &str| match package {
        "srx.test.uid.alpha" => 10123,
        "srx.test.uid.beta" => 10124,
        _ => -1,
    };

    assert!(request_uid_is_task_credential(&credentials, 10123));
    assert!(request_uid_is_task_credential(&credentials, 10124));
    assert_eq!(
        resolve_request_package("srx.test.uid.alpha\0", 10123, package_uid),
        Some("srx.test.uid.alpha".to_string())
    );
    assert_eq!(
        resolve_request_package("srx.test.uid.alpha\0", 10124, package_uid),
        None
    );
    assert_eq!(
        resolve_request_package("srx.test.uid.beta\0", 10124, package_uid),
        Some("srx.test.uid.beta".to_string())
    );
}
