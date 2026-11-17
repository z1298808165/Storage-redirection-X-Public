use super::*;

#[test]
fn native_dedup_retains_distinct_tasks_and_results() {
    let mut state = AuditState::new();
    let source = SourceIdentity::new("test.app".into(), "caller", "high");
    let first = crate::platform::proc_identity::RequestIdentity {
        uid: 10123,
        pid: 42,
        start_time: 55,
        native: "test.app".into(),
        ..Default::default()
    };
    let second = crate::platform::proc_identity::RequestIdentity {
        pid: 43,
        ..first.clone()
    };
    let key = |identity, result| {
        build_event_key_locked(
            &AuditState::new(),
            &source,
            OpKind::Open,
            "open:write",
            "/storage/emulated/0/Download/shared.txt",
            result,
            Some(identity),
        )
    };
    assert!(!should_skip_duplicate_event_locked(
        &mut state,
        &key(&first, (-1, 13))
    ));
    assert!(!should_skip_duplicate_event_locked(
        &mut state,
        &key(&second, (-1, 13))
    ));
    assert!(!should_skip_duplicate_event_locked(
        &mut state,
        &key(&first, (0, 0))
    ));
    assert!(should_skip_duplicate_event_locked(
        &mut state,
        &key(&first, (-1, 13))
    ));
}

#[test]
fn runtime_request_snapshot_replaces_all_untrusted_identity_and_result_metadata() {
    let identity = crate::platform::proc_identity::RequestIdentity::unresolved(2000, 42);
    let extra = monitor_request_extra(
        "op=rename|from=/Download/file%7Cret=0|ret=0|errno=0|caller_uid=10123|caller_pid=99|caller_boundary=|caller_evidence=fake|identify_method=caller|identify_reliability=high|field_encoding=fake|event_role=observation|record_integrity=fake",
        Some(&identity),
        true,
    );
    for forbidden in [
        "ret=",
        "errno=",
        "identify_method=",
        "identify_reliability=",
        "field_encoding=",
        "event_role=",
        "record_integrity=",
    ] {
        assert!(
            !extra.split('|').any(|field| field.starts_with(forbidden)),
            "{forbidden}"
        );
    }
    assert_eq!(extract_kv_value(&extra, "caller_uid="), Some("2000"));
    assert_eq!(extract_kv_value(&extra, "caller_pid="), Some("42"));
    assert_eq!(extract_kv_value(&extra, "caller_boundary="), Some("shell"));
    assert_eq!(
        extract_kv_value(&extra, "caller_evidence="),
        Some("cross_thread")
    );
    assert_eq!(
        extract_kv_value(&extra, "from="),
        Some("/Download/file%7Cret=0")
    );
}

#[test]
fn identity_metadata_ignores_key_text_inside_paths_and_prefixed_fields() {
    let extra = "from=/storage/emulated/0/Download/caller_uid=2000|xcaller_uid=0|caller_uid=10123|caller_pid=42";
    assert_eq!(extract_kv_value(extra, "caller_uid="), Some("10123"));
    assert_eq!(extract_kv_value(extra, "caller_pid="), Some("42"));
    assert_eq!(
        extract_kv_value("from=/Download/op_filter=mkdir|op=open", "op_filter="),
        None
    );
    assert_eq!(extract_kv_value("caller_uid=", "caller_uid="), None);
}

#[test]
fn terminal_executor_ignores_explicit_packages_query_access_and_path_history() {
    crate::hook::push_current_caller_scope("com.fake.app", 10123);
    crate::hook::set_request_identity(Some(
        crate::platform::proc_identity::RequestIdentity::unresolved(0, 42),
    ));
    let state = CallerResolveSnapshot::capture(&AuditState::new());
    let identity = resolve_caller_info_for_path(
        &state,
        "com.fake.app",
        CallerRequestIdentity {
            uid: 0,
            shared_user_id: "android.uid.system",
            task: None,
        },
        "/storage/emulated/0/Download/test",
        false,
        true,
        Some(SourceIdentity::new(
            "com.other.app".into(),
            "provider_open",
            "high",
        )),
    );
    assert_eq!(identity.source, "unknown");
    assert!(identity.package_name.is_empty());
    assert!(
        AuditTrail::instance()
            .record_recent_path_caller(
                "/storage/emulated/0/Download/test",
                10123,
                "com.fake.app",
                "provider_open",
                "high",
            )
            .is_none()
    );
    crate::hook::pop_current_caller_scope();
}

#[test]
fn executor_and_shared_user_labels_never_infer_a_redirect_backend() {
    let state = MonitorStateSnapshot {
        package_name: "com.android.providers.media.module".into(),
        uid: 10123,
        shared_uid_packages: String::new(),
    };
    for source in ["executor", "shared_uid"] {
        let identity = SourceIdentity::new("android.uid.system".into(), source, "fallback");
        assert!(
            infer_monitor_redirect_backend(
                &state,
                &identity,
                OpKind::Open,
                "open:write",
                "/storage/emulated/0/Download/test",
                1,
                ""
            )
            .is_none()
        );
    }
}

#[test]
fn direct_uid_does_not_inherit_another_request_path_identity() {
    policy::set_shared_uid_entries_for_tests(&[(10_301, &["com.a.first", "com.b.second"])]);
    let state = CallerResolveSnapshot::capture(&AuditState::new());
    let identity = resolve_caller_info_for_path(
        &state,
        "",
        CallerRequestIdentity {
            uid: 2000,
            shared_user_id: "",
            task: None,
        },
        "/storage/emulated/0/Download/previous.txt",
        false,
        false,
        Some(SourceIdentity::new(
            "com.a.first".into(),
            "provider_open",
            "high",
        )),
    );
    assert!(identity.package_name.is_empty());
    assert_eq!(identity.source, "unknown");
}

#[test]
fn ambiguous_shared_uid_scope_uses_the_package_managers_shared_user_id() {
    policy::set_shared_uid_entries_for_tests(&[(10_301, &["com.a.first", "com.b.second"])]);
    crate::hook::push_current_caller_scope("", 10_301);
    let mut audit_state = AuditState::new();
    audit_state.multi_package_group = policy::get_shared_uid_packages_string(10_301);
    let state = CallerResolveSnapshot::capture(&audit_state);
    let identity = resolve_caller_info_for_path(
        &state,
        "",
        CallerRequestIdentity {
            uid: 10_301,
            shared_user_id: "android.uid.test_shared",
            task: None,
        },
        "/storage/emulated/0/Download/shared.txt",
        false,
        false,
        Some(SourceIdentity::new(
            "com.a.first".into(),
            "provider_open",
            "high",
        )),
    );
    crate::hook::pop_current_caller_scope();
    assert_eq!(identity.package_name, "android.uid.test_shared");
    assert_eq!(identity.source, "shared_uid");
    assert_eq!(identity.confidence, "fallback");
}

#[test]
fn shared_user_fallback_rejects_invalid_labels_and_keeps_exact_packages() {
    assert_eq!(
        shared_user_fallback(SourceIdentity::unknown(), "not-a-shared-id").source,
        "unknown"
    );
    let exact = SourceIdentity::new("com.example.app".into(), "caller", "high");
    assert_eq!(
        shared_user_fallback(exact, "android.uid.test").package_name,
        "com.example.app"
    );
}

#[test]
fn verified_shared_task_without_a_scope_does_not_borrow_another_members_history() {
    let state = CallerResolveSnapshot::capture(&AuditState::new());
    let mut task = crate::platform::proc_identity::RequestIdentity {
        uid: 10301,
        pid: 42,
        start_time: 55,
        shared_user_id: "android.uid.test_shared".into(),
        ..Default::default()
    };
    for expected in ["android.uid.test_shared", "com.b.second"] {
        if expected == "com.b.second" {
            task.package = expected.into();
        }
        let identity = resolve_caller_info_for_path(
            &state,
            "",
            CallerRequestIdentity {
                uid: task.uid,
                shared_user_id: &task.shared_user_id,
                task: Some(&task),
            },
            "/storage/emulated/0/Download/shared.txt",
            false,
            true,
            Some(SourceIdentity::new(
                "com.a.first".into(),
                "provider_open",
                "high",
            )),
        );
        assert_eq!(identity.package_name, expected);
        assert_eq!(
            identity.source,
            if task.package.is_empty() {
                "shared_uid"
            } else {
                "caller"
            }
        );
    }
}

#[test]
fn missing_request_identity_can_still_use_a_provider_path_hint() {
    let state = CallerResolveSnapshot::capture(&AuditState::new());
    let identity = resolve_caller_info_for_path(
        &state,
        "",
        CallerRequestIdentity {
            uid: -1,
            shared_user_id: "",
            task: None,
        },
        "/storage/emulated/0/Download/delegated.txt",
        false,
        false,
        Some(SourceIdentity::new(
            "com.a.first".into(),
            "provider_open",
            "high",
        )),
    );
    assert_eq!(identity.package_name, "com.a.first");
    assert_eq!(identity.source, "provider_open");
}
