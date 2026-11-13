use native_config_fixtures::registered_process::decode;

const RECORD: &str =
    "state=native_registered\npackage=com.example.app\npid=123\nuid=10123\napp_start_time=456\n";

#[test]
fn identities_require_current_uid_start_time_and_package_owner() {
    let identity = decode(RECORD).unwrap();
    assert_eq!(identity.package, "com.example.app");
    assert!(identity.matches(10123, Some(456), 10123));
    assert!(!identity.matches(10124, Some(456), 10124));
    assert!(!identity.matches(10123, Some(457), 10123));
    assert!(!identity.matches(10123, None, 10123));
    assert!(!identity.matches(10123, Some(456), -1));
    assert!(!identity.matches(10123, Some(456), 10124));
    let multi_user = decode(&RECORD.replace("uid=10123", "uid=1010123")).unwrap();
    assert!(multi_user.matches(1010123, Some(456), 10123));
    assert!(!multi_user.matches(10123, Some(456), 10123));
}

#[test]
fn malformed_ambiguous_and_incomplete_records_are_rejected() {
    for changed in [
        RECORD.replace("pid=123", "pid=0"),
        RECORD.replace("uid=10123", "uid=-1"),
        RECORD.replace("app_start_time=456", "app_start_time=0"),
        RECORD.replace("app_start_time=456", "app_start_time=not-a-time"),
        RECORD.replace("package=com.example.app", "package=../escape"),
        RECORD.replace("package=com.example.app", "package="),
        RECORD.replace(
            "package=com.example.app",
            &format!("package={}", "x".repeat(256)),
        ),
        format!("{RECORD}pid=123\n"),
        format!("{RECORD}uid=10123\n"),
        format!("{RECORD}package=com.other.app\n"),
        RECORD.replace("app_start_time=456\n", ""),
    ] {
        assert!(decode(&changed).is_none(), "{changed:?}");
    }
    assert!(decode(&"x".repeat(4097)).is_none());
}
