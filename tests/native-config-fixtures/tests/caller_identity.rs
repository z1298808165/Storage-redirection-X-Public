use native_config_fixtures::platform::proc_identity::{AttributionBoundary, RequestIdentity};
use native_config_fixtures::{caller_context as context, shared_uid_policy as policy};

fn remember_cross_thread_caller(
    uid: i32,
    pid: i32,
    start_time: u64,
    package: &str,
    shared_user_id: &str,
) {
    context::remember_cross_thread_caller(&RequestIdentity {
        uid,
        pid,
        start_time,
        native: "test.task".into(),
        selinux_context: "u:r:untrusted_app:s0".into(),
        package: package.into(),
        shared_user_id: shared_user_id.into(),
        ..Default::default()
    });
}
fn cross_thread_evidence(uid: i32, pid: i32) -> Option<RequestIdentity> {
    context::set_monitor_caller_uid(uid);
    context::set_monitor_caller_pid(pid);
    let (record_uid, record_pid, evidence) = context::record_caller_identity_with_evidence();
    assert_eq!((record_uid, record_pid), (uid, pid));
    evidence
}

#[test]
fn terminal_executor_cannot_be_replaced_by_scope_uid_or_mapping_hints() {
    let executor = RequestIdentity {
        uid: 10123,
        pid: 42,
        boundary: AttributionBoundary::Privileged,
        ..RequestIdentity::default()
    };
    context::set_current_caller_from_external_signal("test.before", 10123);
    context::set_request_identity(Some(executor.clone()));
    context::set_current_caller_package("test.after");
    context::set_current_caller_uid(10124);
    context::set_current_caller_from_mapping_request_owner("test.mapping", 10124);
    assert_eq!(context::get_current_caller_package(), "");
    assert_eq!(context::get_current_caller_uid(), 10123);
    context::push_current_caller_scope("test.synthetic", 10124);
    assert!(context::is_caller_attribution_stopped());
    assert_eq!(context::get_current_caller_package(), "");
    assert_eq!(context::get_current_caller_uid(), 10123);
    context::set_monitor_caller_uid(10124);
    context::set_monitor_caller_pid(43);
    assert_eq!(
        context::record_caller_identity_with_evidence(),
        (10123, 42, None)
    );
    context::pop_current_caller_scope();
    assert_eq!(context::current_request_identity(), Some(executor));
    context::clear_current_caller();
    assert!(!context::is_caller_attribution_stopped());
}

#[test]
fn binder_restore_keeps_original_executor_boundary() {
    let executor = RequestIdentity::unresolved(0, 42);
    context::set_request_identity(Some(executor.clone()));
    let saved = context::binder_identity_snapshot();
    context::remember_binder_identity(42, saved);
    context::set_request_identity(None);
    context::restore_binder_identity(42);
    assert_eq!(context::current_request_identity(), Some(executor));
    context::clear_current_caller();
}

#[test]
fn nested_scopes_restore_all_request_evidence() {
    context::set_current_caller_from_mapping_request_owner("test.outer", 10123);
    context::set_monitor_caller_uid(10123);
    context::set_monitor_caller_pid(500);
    context::set_fuse_caller_uid(10123);
    context::set_fuse_caller_pid(501);
    context::set_binder_saved_caller_uid(10123);
    context::set_binder_saved_caller_package("test.outer");
    context::set_binder_identity_cleared(true);
    let before = context::get_current_caller_age_ms();
    context::push_current_caller_scope("test.inner", 10124);
    assert!(!context::is_binder_identity_cleared());
    assert_eq!(context::get_fuse_caller_uid(), -1);
    assert_eq!(context::get_binder_saved_caller_uid(), -1);
    context::set_monitor_caller_uid(10124);
    context::set_monitor_caller_pid(600);
    context::push_current_caller_scope("", 10124);
    assert_eq!(context::get_current_caller_package(), "");
    context::pop_current_caller_scope();
    assert_eq!(context::get_current_caller_package(), "test.inner");
    let (uid, pid, _) = context::record_caller_identity_with_evidence();
    assert_eq!((uid, pid), (10124, 600));
    context::pop_current_caller_scope();
    assert_eq!(context::get_current_caller_package(), "test.outer");
    let (uid, pid, _) = context::record_caller_identity_with_evidence();
    assert_eq!((uid, pid), (10123, 500));
    assert_eq!(context::get_fuse_caller_uid(), 10123);
    assert_eq!(context::get_fuse_caller_pid(), 501);
    assert_eq!(context::get_binder_saved_caller_package(), "test.outer");
    assert!(context::is_binder_identity_cleared());
    assert!(context::is_current_caller_from_mapping_request_owner());
    assert!(!context::is_current_caller_from_external_signal());
    assert!(context::get_current_caller_age_ms() >= before);
}

#[test]
fn binder_tokens_restore_nested_identity_without_clearing_outer_request() {
    context::push_current_caller_scope("test.outer", 10123);
    let original = context::binder_identity_snapshot();
    context::set_binder_saved_caller_uid(10123);
    context::set_binder_saved_caller_package("test.outer");
    context::set_binder_identity_cleared(true);
    context::remember_binder_identity(10, original);
    let outer = context::binder_identity_snapshot();
    context::remember_binder_identity(20, outer);
    context::restore_binder_identity(20);
    assert!(context::is_binder_identity_cleared());
    assert_eq!(context::get_binder_saved_caller_uid(), 10123);
    context::restore_binder_identity(99);
    assert!(context::is_binder_identity_cleared());
    context::restore_binder_identity(10);
    assert!(!context::is_binder_identity_cleared());
    assert_eq!(context::get_binder_saved_caller_uid(), -1);
    context::pop_current_caller_scope();
}

#[test]
fn shared_uid_scopes_preserve_each_explicit_package() {
    context::push_current_caller_scope("test.shared.first", 10123);
    assert_eq!(context::get_current_caller_package(), "test.shared.first");
    context::push_current_caller_scope("test.shared.second", 10123);
    assert_eq!(context::get_current_caller_package(), "test.shared.second");
    context::pop_current_caller_scope();
    assert_eq!(context::get_current_caller_package(), "test.shared.first");
    assert_eq!(context::get_current_caller_uid(), 10123);
    context::pop_current_caller_scope();
}

#[test]
fn nested_scopes_restore_package_manager_shared_user_id() {
    context::push_current_caller_scope_with_shared_user("", 10123, "android.uid.outer");
    assert_eq!(
        context::get_current_caller_shared_user_id(),
        "android.uid.outer"
    );
    context::push_current_caller_scope_with_shared_user("", 10124, "android.uid.inner");
    assert_eq!(
        context::get_current_caller_shared_user_id(),
        "android.uid.inner"
    );
    context::pop_current_caller_scope();
    assert_eq!(
        context::get_current_caller_shared_user_id(),
        "android.uid.outer"
    );
    context::pop_current_caller_scope();
    assert_eq!(context::get_current_caller_shared_user_id(), "");
}

#[test]
fn cross_thread_shared_user_id_survives_package_ambiguity() {
    remember_cross_thread_caller(40123, 765432, 55, "test.shared.first", "android.uid.test");
    remember_cross_thread_caller(40123, 765432, 55, "test.shared.second", "android.uid.test");
    let evidence = cross_thread_evidence(40123, 765432).unwrap();
    assert!(evidence.package.is_empty());
    assert_eq!(evidence.start_time, 55);
    assert_eq!(evidence.shared_user_id, "android.uid.test");
    assert_eq!(cross_thread_evidence(40123, 765433), None);
}

#[test]
fn cross_thread_cache_preserves_the_full_task_and_closes_same_epoch_changes() {
    let original = RequestIdentity {
        uid: 40126,
        pid: 765436,
        start_time: 55,
        native: "test.original".into(),
        selinux_context: "u:r:untrusted_app:s0".into(),
        package: "test.shared.first".into(),
        shared_user_id: "android.uid.test".into(),
        ..Default::default()
    };
    context::remember_cross_thread_caller(&original);
    assert_eq!(
        cross_thread_evidence(original.uid, original.pid),
        Some(original.clone())
    );
    let changed = RequestIdentity {
        native: "test.replacement".into(),
        selinux_context: "u:r:platform_app:s0".into(),
        package: "test.shared.second".into(),
        ..original.clone()
    };
    context::remember_cross_thread_caller(&changed);
    let captured = cross_thread_evidence(original.uid, original.pid).unwrap();
    let mut expected = original.clone();
    expected.invalidate();
    assert_eq!(captured, expected);
    context::remember_cross_thread_caller(&original);
    assert_eq!(
        cross_thread_evidence(original.uid, original.pid),
        Some(expected)
    );
}

#[test]
fn cross_thread_pid_reuse_replaces_the_entire_task_entry() {
    remember_cross_thread_caller(40124, 765434, 55, "test.old.member", "android.uid.old");
    remember_cross_thread_caller(40124, 765434, 56, "test.new.member", "android.uid.new");
    let evidence = cross_thread_evidence(40124, 765434).unwrap();
    assert_eq!(evidence.package, "test.new.member");
    assert_eq!(evidence.start_time, 56);
    assert_eq!(evidence.shared_user_id, "android.uid.new");
}

#[test]
fn captured_cross_thread_evidence_does_not_change_when_the_cache_is_replaced() {
    remember_cross_thread_caller(40125, 765435, 55, "test.old.member", "android.uid.old");
    let captured = cross_thread_evidence(40125, 765435).unwrap();
    std::thread::spawn(|| {
        remember_cross_thread_caller(40125, 765435, 56, "test.new.member", "android.uid.new");
    })
    .join()
    .unwrap();
    let current = cross_thread_evidence(40125, 765435).unwrap();
    assert_eq!(
        (
            captured.package.as_str(),
            captured.start_time,
            captured.shared_user_id.as_str()
        ),
        ("test.old.member", 55, "android.uid.old")
    );
    assert_eq!(
        (
            current.package.as_str(),
            current.start_time,
            current.shared_user_id.as_str()
        ),
        ("test.new.member", 56, "android.uid.new")
    );
}

#[test]
fn concurrent_cross_thread_callers_keep_distinct_entries() {
    let start = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let start = &start;
                scope.spawn(move || {
                    let uid = 50100 + worker;
                    let pid = 900000 + worker;
                    start.wait();
                    for _ in 0..1024 {
                        remember_cross_thread_caller(uid, pid, 55, "test.concurrent", "");
                        let evidence = cross_thread_evidence(uid, pid).unwrap();
                        assert_eq!(evidence.package, "test.concurrent");
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
}

#[test]
fn concurrent_and_reused_workers_keep_packages_and_uids_separate() {
    let workers: Vec<_> = (0..8)
        .map(|worker| {
            std::thread::spawn(move || {
                for request in 0..100 {
                    let uid = 10123 + request % 3;
                    let package = format!("test.member{worker}_{}", request % 2);
                    context::push_current_caller_scope(&package, uid);
                    context::set_monitor_caller_uid(uid);
                    context::set_monitor_caller_pid(500);
                    std::thread::yield_now();
                    assert_eq!(context::get_current_caller_package(), package);
                    let (record_uid, record_pid, _) =
                        context::record_caller_identity_with_evidence();
                    assert_eq!((record_uid, record_pid), (uid, 500));
                    context::pop_current_caller_scope();
                    assert_eq!(context::get_current_caller_uid(), -1);
                    assert_eq!(context::get_current_caller_package(), "");
                    assert!(!context::is_current_caller_scope_active());
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn cross_thread_identity_is_record_only_and_keeps_uid_pid_together() {
    let self_uid = unsafe { libc::getuid() as i32 };
    let caller_uid = self_uid + 1;
    context::set_current_caller_uid(self_uid);
    context::set_monitor_caller_uid(self_uid);
    context::set_fuse_caller_uid(caller_uid);
    context::set_fuse_caller_pid(700);
    remember_cross_thread_caller(caller_uid, 700, 55, "test.cross_thread", "");

    let (uid, pid, evidence) = context::record_caller_identity_with_evidence();
    assert_eq!((uid, pid), (caller_uid, 700));
    let evidence = evidence.unwrap();
    assert_eq!(evidence.package, "test.cross_thread");
    assert_eq!(evidence.start_time, 55);
    assert_eq!(context::get_current_caller_uid(), self_uid);
    assert_eq!(context::get_effective_caller_uid_for_monitor(), self_uid);
}

#[test]
fn cross_thread_identity_matches_both_uid_and_pid_and_rejects_conflicts() {
    remember_cross_thread_caller(20123, 701, 55, "test.identity.first", "");
    remember_cross_thread_caller(20124, 701, 55, "test.identity.second", "");
    assert_eq!(
        cross_thread_evidence(20123, 701).unwrap().package,
        "test.identity.first"
    );
    assert_eq!(
        cross_thread_evidence(20124, 701).unwrap().package,
        "test.identity.second"
    );
    assert_eq!(cross_thread_evidence(20123, 702), None);

    remember_cross_thread_caller(20123, 701, 55, "test.identity.third", "");
    assert_eq!(cross_thread_evidence(20123, 701), None);
}

#[test]
fn explicit_scope_keeps_its_identity_instead_of_cross_thread_evidence() {
    remember_cross_thread_caller(30123, 801, 55, "test.previous", "");
    context::push_current_caller_scope("test.current", 30124);
    context::set_monitor_caller_uid(30124);
    context::set_monitor_caller_pid(802);
    context::set_fuse_caller_uid(30123);
    context::set_fuse_caller_pid(801);

    assert_eq!(
        context::record_caller_identity_with_evidence(),
        (30124, 802, None)
    );
    assert_eq!(context::get_current_caller_package(), "test.current");
    context::pop_current_caller_scope();
}

#[test]
fn direct_caller_identity_is_not_replaced_by_another_fuse_request() {
    remember_cross_thread_caller(30125, 803, 55, "test.fuse", "");
    context::set_current_caller_from_external_signal("test.direct", 30126);
    context::set_monitor_caller_uid(30126);
    context::set_monitor_caller_pid(804);
    context::set_fuse_caller_uid(30125);
    context::set_fuse_caller_pid(803);

    assert_eq!(
        context::record_caller_identity_with_evidence(),
        (30126, 804, None)
    );
}

#[test]
fn ambiguous_scope_does_not_borrow_previous_package_from_shared_process() {
    remember_cross_thread_caller(30127, 805, 55, "test.previous.member", "");
    context::push_current_caller_scope("", 30127);
    context::set_monitor_caller_uid(30127);
    context::set_monitor_caller_pid(805);

    assert_eq!(
        context::record_caller_identity_with_evidence(),
        (30127, 805, None)
    );
    assert_eq!(context::get_current_caller_package(), "");
    context::pop_current_caller_scope();
}

#[test]
fn uid_inventory_keeps_users_and_preserves_last_complete_snapshot() {
    let dir = std::env::temp_dir().join(format!("srx-uid-inventory-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let filename = std::path::Path::new(
        native_config_fixtures::platform::module_paths::SYSTEM_WRITER_UIDS_FILE,
    )
    .file_name()
    .unwrap();
    let file = dir.join(filename);
    std::fs::write(
        &file,
        "# package:uid\ntest.a:10123\ntest.b:10123\ntest.a:1010123\ntest.c:1010124\n",
    )
    .unwrap();
    policy::set_shared_uid_config_dir(dir.to_str().unwrap());
    assert_eq!(policy::get_packages_for_uid(10123), ["test.a", "test.b"]);
    assert_eq!(policy::get_packages_for_uid(1010123), ["test.a"]);
    assert_eq!(policy::get_uid_for_package_in_user("test.a", 10), 1010123);
    assert_eq!(policy::get_uid_for_package_in_user("test.b", 10), -1);
    assert_eq!(policy::get_uid_for_package_in_user("test.c", 0), -1);
    assert_eq!(policy::get_uid_for_package_in_user("test.a", -1), -1);
    for invalid in ["test.a:10123\nbroken", "test.a:10123\ntest.a:10124\n"] {
        std::fs::write(&file, invalid).unwrap();
        policy::refresh_shared_uid_cache();
        assert_eq!(policy::get_uid_for_package_in_user("test.a", 10), 1010123);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
