use super::{
    FuseRedirectFs,
    config::FuseRedirectConfig,
    policy::{OperationKind, RedirectPolicy},
};

#[test]
fn inode_backend_cache_reroutes_after_policy_and_path_changes() {
    let mut config = FuseRedirectConfig {
        package_name: "test.cache".into(),
        app_pid: 0,
        app_start_time_ticks: None,
        uid: 10123,
        app_data_dir: "/data/user/0/test.cache".into(),
        redirect_target: "/storage/emulated/0".into(),
        mount_root: Some("/storage/emulated/0".into()),
        real_root_override: None,
        is_file_monitor_enabled: false,
        allowed_real_paths: vec![],
        excluded_real_paths: vec![],
        sandboxed_paths: vec![],
        read_only_paths: vec![],
        path_mappings: vec![],
        is_mapping_mode_only: false,
        is_monitor_only: false,
        is_passthrough_host: true,
        namespace_policy_only: false,
    };
    let fs = FuseRedirectFs::new(config.clone()).unwrap();
    let before = RedirectPolicy::new(config.clone()).unwrap();
    let ino = {
        let mut state = fs.state.write().unwrap();
        FuseRedirectFs::ino_for_path_locked(&mut state, "0/Download/probe")
    };
    let backend = fs
        .backend_for_ino(&before, ino, OperationKind::Read)
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &backend,
        &fs.backend_for_ino(&before, ino, OperationKind::Read)
            .unwrap()
    ));
    config.path_mappings = vec![crate::domain::PathMapping::new(
        "/storage/emulated/0/Download/probe".into(),
        "/storage/emulated/0/Pictures/probe".into(),
    )];
    let after = RedirectPolicy::new(config.clone()).unwrap();
    assert_ne!(before.fingerprint, after.fingerprint);
    let resolved = fs
        .backend_for_ino(&after, ino, OperationKind::Read)
        .unwrap();
    assert!(resolved.path.ends_with("Pictures/probe"));
    assert_ne!(backend.path, resolved.path);
    assert!(!std::sync::Arc::ptr_eq(&backend, &resolved));
    assert!(std::sync::Arc::ptr_eq(
        &resolved,
        &fs.backend_for_ino(&after, ino, OperationKind::Read)
            .unwrap()
    ));
    config.uid += 1;
    let mut other_uid = RedirectPolicy::new(config).unwrap();
    // 独立验证 UID 守卫，避免策略指纹变化掩盖跨 UID 的缓存误命中。
    other_uid.fingerprint = after.fingerprint;
    assert!(!std::sync::Arc::ptr_eq(
        &resolved,
        &fs.backend_for_ino(&other_uid, ino, OperationKind::Read)
            .unwrap()
    ));
    let cached = fs
        .backend_for_ino(&after, ino, OperationKind::Read)
        .unwrap();
    let stale_generation = super::backend_overlay::snapshot_generation().wrapping_sub(1);
    fs.state
        .write()
        .unwrap()
        .inode_backends
        .get_mut(&ino.0)
        .unwrap()
        .overlay_generation = stale_generation;
    assert!(!std::sync::Arc::ptr_eq(
        &cached,
        &fs.backend_for_ino(&after, ino, OperationKind::Read)
            .unwrap()
    ));
    fs.state
        .write()
        .unwrap()
        .paths_by_inode
        .insert(ino.0, "0/Download/renamed".into());
    assert_eq!(
        fs.backend_for_ino(&before, ino, OperationKind::Read)
            .unwrap()
            .rel,
        "Download/renamed"
    );
}
