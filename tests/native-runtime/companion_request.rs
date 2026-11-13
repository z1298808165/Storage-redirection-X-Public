use super::*;
use crate::fuse_redirect::config::{
    mount_request_has_explicit_rules, mount_request_has_policy_rules,
};

#[test]
fn monitor_only_requires_enabled_capture_but_empty_isolation_is_a_policy() {
    let mut request = CompanionMountRequest {
        is_monitor_only: true,
        ..Default::default()
    };
    assert!(!mount_request_has_explicit_rules(&request));
    assert!(!mount_request_has_policy_rules(&request));
    request.is_file_monitor_enabled = true;
    assert!(mount_request_has_explicit_rules(&request));
    assert!(mount_request_has_policy_rules(&request));
    request.is_monitor_only = false;
    assert!(!mount_request_has_explicit_rules(&request));
    assert!(mount_request_has_policy_rules(&request));
    request.is_mapping_mode_only = true;
    assert!(!mount_request_has_policy_rules(&request));
    request.read_only_paths.push("Download/Locked".to_owned());
    assert!(mount_request_has_explicit_rules(&request));
    assert!(mount_request_has_policy_rules(&request));
}

#[test]
fn mount_identity_rejects_truncating_integers() {
    for (pid, uid) in [
        (1i64, 10000i64),
        (4294967297, 10000),
        (1, 4294977296),
        (0, 10000),
        (1, -1),
    ] {
        let payload = format!(
            r#"{{"pid":{pid},"uid":{uid},"package":"com.example.app","redirect_target":"/storage/emulated/0/Example"}}"#
        );
        assert_eq!(
            parse_companion_mount_request(&payload).is_ok(),
            pid == 1 && uid == 10000
        );
    }
}
