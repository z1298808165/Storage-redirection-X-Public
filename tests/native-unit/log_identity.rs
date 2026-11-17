use super::*;

#[test]
fn legacy_writer_names_require_a_package_manager_uid_match() {
    crate::redirect::policy::set_shared_uid_entries_for_tests(&[
        (10123, &["test.ordinary"]),
        (10124, &["com.android.providers.downloads", "test.other"]),
    ]);
    let mut identity = crate::platform::proc_identity::RequestIdentity {
        uid: 10123,
        native: "android.process.media".into(),
        ..Default::default()
    };
    assert!(!identity.is_system_writer());
    identity.uid = 10124;
    assert!(identity.is_system_writer());
    identity.boundary = crate::platform::proc_identity::AttributionBoundary::Unverified;
    assert!(!identity.is_system_writer());
}

#[test]
fn only_successful_verified_package_requests_can_feed_path_hints() {
    let base = "time|provider|com.test.app|OPEN|/storage/emulated/0/Download/file|event_role=request|ret=0|identify_method=caller|identify_reliability=high|op_filter=open:write|caller_boundary=";
    assert!(can_register_request_path_hint(base));
    for (from, to) in [
        ("ret=0", "ret=-1"),
        ("ret=0", "ret=bad"),
        ("event_role=request", "event_role=observation"),
        ("identify_method=caller", "identify_method=shared_uid"),
        ("identify_method=caller", "identify_method=executor"),
        ("identify_method=caller", "identify_method=path_config"),
        ("identify_reliability=high", "identify_reliability=medium"),
        ("identify_reliability=high", "identify_reliability=fallback"),
        ("caller_boundary=", "caller_boundary=root"),
        ("caller_boundary=", "caller_boundary=unverified"),
    ] {
        assert!(
            !can_register_request_path_hint(&base.replace(from, to)),
            "{to}"
        );
    }
}

#[test]
fn monitor_metadata_is_matched_as_complete_fields() {
    let line = "time|process|caller|OPEN|/path|op=open:write|caller_uid=10123|xcaller_pid=123";
    assert_eq!(monitor_field(line, "caller_uid"), Some("10123"));
    assert_eq!(monitor_field(line, "caller_pid"), None);
    assert_eq!(monitor_field(line, "op"), Some("open:write"));
}

#[test]
fn duplicate_identity_fields_cannot_gain_root_sender_trust() {
    let line = "time|provider|test.app|OPEN|/path|caller_uid=10123|caller_pid=42|ret=0";
    for extra in ["caller_uid=10124", "caller_pid=43", "ret=-1"] {
        let ambiguous = format!("{line}|{extra}");
        assert!(!monitor_sender_is_trusted(&ambiguous, Some((0, 1))));
    }
}

#[test]
fn ambiguous_results_stay_visible_but_cannot_claim_success_or_feed_path_hints() {
    let line = "time|provider|test.app|OPEN|/path|caller_uid=10123|event_role=request|ret=-1|errno=13|identify_method=caller|identify_reliability=high|op_filter=open:write|from=/file|ret=0";
    assert!(!monitor_sender_is_trusted(line, Some((0, 1))));
    let result = rewrite_monitor_identity(line, "-", "unknown", "none", None);
    assert_eq!(
        monitor_field(&result, "record_integrity"),
        Some("ambiguous")
    );
    assert!(result.contains("ret=-1") && result.contains("ret=0"));
    assert!(!can_register_request_path_hint(&result));
    let multiline = line.replace("/file", "/first\nforged");
    let result = rewrite_monitor_identity(&multiline, "-", "unknown", "none", None);
    assert!(!result.contains(['\r', '\n']));
    assert_eq!(
        monitor_field(&result, "record_integrity"),
        Some("ambiguous")
    );
}

#[test]
fn replacing_identity_keeps_operation_and_original_request_fields() {
    let line = "time|provider|test.fake|MKDIR|/storage/emulated/0/path|ret=-1|errno=13|event_role=request|caller_uid=0|caller_pid=23|identify_method=caller|identify_reliability=high";
    let identity = crate::platform::proc_identity::RequestIdentity::unresolved(0, 23);
    let result = rewrite_monitor_identity(line, "su", "executor", "high", Some(&identity));
    assert_eq!(result.split('|').nth(2), Some("su"));
    assert_eq!(monitor_field(&result, "ret"), Some("-1"));
    assert_eq!(monitor_field(&result, "errno"), Some("13"));
    assert_eq!(monitor_field(&result, "caller_boundary"), Some("root"));
    assert_eq!(result.matches("identify_method=").count(), 1);
    assert_eq!(result.matches("caller_uid=").count(), 1);
    assert_eq!(result.matches("caller_pid=").count(), 1);
}

#[test]
fn monitor_sender_cannot_claim_another_uid_without_provider_evidence() {
    let line = "time|provider|test.other|OPEN|/path|caller_uid=10123";
    assert!(!monitor_sender_is_trusted(line, None));
    assert!(!monitor_sender_is_trusted(line, Some((10124, -1))));
    assert!(monitor_sender_is_trusted(line, Some((10123, -1))));
    assert!(monitor_sender_is_trusted(line, Some((0, -1))));
}

#[test]
fn rewriting_short_records_never_panics() {
    assert_eq!(
        rewrite_monitor_identity("invalid", "-", "unknown", "none", None),
        "invalid"
    );
    assert_eq!(enrich_monitor_record_identity("invalid"), None);
}

#[test]
fn late_enrichment_cannot_reopen_terminal_or_shared_user_identity() {
    for metadata in [
        "identify_method=shared_uid|identify_reliability=fallback",
        "identify_method=executor|caller_boundary=root",
        "identify_method=executor|caller_boundary=privileged",
    ] {
        let line = format!(
            "time|provider|android.uid.system|OPEN|/path|event_role=request|caller_uid=10123|caller_pid=42|{metadata}"
        );
        assert_eq!(enrich_monitor_record_identity(&line), None);
    }
}

#[test]
fn late_record_with_another_task_start_time_cannot_take_the_current_identity() {
    let pid = std::process::id() as i32;
    // SAFETY: getuid 无参数，只读取当前测试进程的 UID。
    let uid = unsafe { libc::getuid() };
    let start = crate::platform::process_start_time_ticks(pid).unwrap();
    let line = format!(
        "time|provider|test.previous|OPEN|/path|event_role=request|caller_uid={uid}|caller_pid={pid}|caller_start_time={}|identify_method=caller|identify_reliability=high",
        start + 1
    );
    let enriched = enrich_monitor_record_identity(&line).unwrap();
    assert_eq!(monitor_field(&enriched, "identify_method"), Some("unknown"));
    assert_eq!(
        monitor_field(&enriched, "caller_start_time"),
        Some((start + 1).to_string().as_str())
    );
}

#[test]
fn late_record_with_changed_native_or_context_preserves_the_original_unknown_task() {
    let pid = std::process::id() as i32;
    let uid = unsafe { libc::getuid() } as i32;
    let current = crate::platform::proc_identity::read_proc_identity_for_request(pid, uid).unwrap();
    for (native, context) in [
        ("previous-native", current.selinux_context.as_str()),
        (current.native.as_str(), "u:r:previous_domain:s0"),
    ] {
        let line = format!(
            "time|provider|test.previous|OPEN|/path|event_role=request|caller_uid={uid}|caller_pid={pid}|caller_start_time={}|caller_context={context}|native={native}|identify_method=caller|identify_reliability=high",
            current.start_time
        );
        let enriched = enrich_monitor_record_identity(&line).unwrap();
        assert_eq!(
            monitor_field(&enriched, "caller_boundary"),
            Some("unverified")
        );
        assert_eq!(monitor_field(&enriched, "identify_method"), Some("unknown"));
        assert_eq!(monitor_field(&enriched, "native"), Some(native));
        assert_eq!(monitor_field(&enriched, "caller_context"), Some(context));
    }
}

#[test]
fn complete_nonterminal_log_snapshots_can_be_revalidated_but_missing_fields_stay_unknown() {
    let original = RequestIdentity {
        uid: 10123,
        pid: 42,
        start_time: 55,
        native: "test.original".into(),
        selinux_context: "u:r:untrusted_app:s0".into(),
        ..Default::default()
    };
    let restored =
        RequestIdentity::from_log_fields(original.uid, original.pid, &original.log_fields());
    assert_eq!(restored, original);
    for fields in [
        "caller_boundary=",
        "caller_boundary=|caller_start_time=55",
        "caller_boundary=|caller_start_time=55|native=test.original",
        "caller_boundary=|caller_start_time=55|native=test.original|caller_context=u:r:ksu:s0",
    ] {
        assert!(RequestIdentity::from_log_fields(original.uid, original.pid, fields).is_terminal());
    }
}

#[test]
fn invalidated_log_snapshots_keep_unknown_attribution_and_original_task_fields() {
    for uid in [0, 2000, 10123] {
        let mut captured = RequestIdentity {
            uid,
            pid: 42,
            start_time: 55,
            native: "original".into(),
            selinux_context: "u:r:untrusted_app:s0".into(),
            package: "test.app".into(),
            shared_user_id: "android.uid.test".into(),
            ..RequestIdentity::default()
        };
        captured.invalidate();
        let restored = RequestIdentity::from_log_fields(uid, 42, &captured.log_fields());
        assert_eq!(restored, captured);
        assert_eq!(restored.monitor_identity(), ("-", "unknown", "none"));
    }
}
#[test]
fn file_names_round_trip_through_the_production_codec() {
    use crate::platform::log_fields::encode;
    for value in [
        "普通文件.txt",
        "|ret=0|caller_uid=0",
        "%7C%0A%25",
        "line\r\nnext",
        "\\n",
    ] {
        let encoded = encode(value);
        assert!(!encoded.contains(['|', '\r', '\n']));
        assert_eq!(decode(&encoded), value);
    }
    assert!(matches!(
        encode("ordinary.txt"),
        std::borrow::Cow::Borrowed(_)
    ));
    assert!(matches!(
        decode("ordinary.txt"),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn identity_rewrite_upgrades_legacy_literals_and_encodes_executor_columns() {
    let identity = RequestIdentity {
        native: "复制+worker|ret=0\r\n%7C".into(),
        ..RequestIdentity::unresolved(0, 42)
    };
    let original = "time|provider|old|OPEN|/Download/literal%7C.txt|from=/source/%0A|ret=-1|errno=13|event_role=request";
    let rewritten = rewrite_monitor_identity(
        original,
        &identity.native,
        "executor",
        "high",
        Some(&identity),
    );
    assert_eq!(
        rewritten
            .split('|')
            .filter(|field| field.starts_with("ret="))
            .count(),
        1
    );
    assert_eq!(
        decode(rewritten.split('|').nth(2).unwrap()),
        identity.native
    );
    assert_eq!(
        decode(rewritten.split('|').nth(4).unwrap()),
        "/Download/literal%7C.txt"
    );
    assert_eq!(
        decode(monitor_field(&rewritten, "from").unwrap()),
        "/source/%0A"
    );
    assert_eq!(monitor_field(&rewritten, "field_encoding"), Some("percent"));
    assert_eq!(monitor_field(&rewritten, "ret"), Some("-1"));
    assert!(!rewritten.contains(['\r', '\n']));
    assert_eq!(
        RequestIdentity::from_log_fields(0, 42, &rewritten).native,
        identity.native
    );
    assert_eq!(
        rewrite_monitor_identity(
            &rewritten,
            &identity.native,
            "executor",
            "high",
            Some(&identity)
        ),
        rewritten
    );
}
