use super::*;

#[test]
fn fuse_create_dedup_keeps_different_executor_tasks() {
    let first = RequesterIdentity {
        uid: 10123,
        pid: 42,
        start_time: 55,
        ..Default::default()
    };
    let second = RequesterIdentity {
        pid: 43,
        ..first.clone()
    };
    let path = "/storage/emulated/0/Download/task-dedup";
    assert!(!should_skip_duplicate_monitor_create(
        "test.app",
        path,
        path,
        Some(&first)
    ));
    assert!(!should_skip_duplicate_monitor_create(
        "test.app",
        path,
        path,
        Some(&second)
    ));
    assert!(should_skip_duplicate_monitor_create(
        "test.app",
        path,
        path,
        Some(&first)
    ));
}

#[test]
fn denied_fuse_operations_keep_the_executor_and_actual_failure() {
    let policy = RedirectPolicy::new(FuseRedirectConfig {
        package_name: "test.host".into(),
        app_pid: 0,
        app_start_time_ticks: None,
        uid: 10123,
        app_data_dir: "/data/user/0/test.host".into(),
        redirect_target: "/storage/emulated/0".into(),
        mount_root: None,
        real_root_override: None,
        is_file_monitor_enabled: true,
        allowed_real_paths: vec![],
        excluded_real_paths: vec![],
        sandboxed_paths: vec![],
        read_only_paths: vec![],
        path_mappings: vec![],
        is_mapping_mode_only: false,
        is_monitor_only: true,
        is_passthrough_host: true,
        namespace_policy_only: false,
    })
    .unwrap();
    let backend = BackendPath {
        rel: "Download/boundary-test".into(),
        path: "/data/media/0/Download/boundary-test".into(),
        is_read_only: true,
        is_shared_public_backend: true,
        is_app_private_backend: false,
    };
    for uid in [0, 2000, 10123] {
        let identity = RequesterIdentity {
            native: "复制+worker|ret=0\n%7C".into(),
            ..RequesterIdentity::unresolved(uid, 42)
        };
        let line = policy
            .monitor_read_only_deny_line(
                "open:write",
                &backend,
                None,
                libc::EACCES,
                Some(&identity),
            )
            .unwrap();
        let (label, method, reliability) = identity.monitor_identity();
        assert_eq!(
            line.split('|').nth(2),
            Some(crate::platform::log_fields::encode(label).as_ref())
        );
        assert_eq!(
            line.split('|').nth(1),
            Some(crate::platform::log_fields::encode(&identity.native).as_ref())
        );
        assert!(!line.contains(['\r', '\n']));
        for field in [
            "ret=-1".to_owned(),
            "errno=13".to_owned(),
            format!("identify_method={method}"),
            format!("identify_reliability={reliability}"),
            format!("caller_uid={uid}"),
            "caller_pid=42".to_owned(),
            "deny_reason=read_only_rule".to_owned(),
        ] {
            assert!(line.split('|').any(|value| value == field), "{line}");
        }
    }
}
