use super::*;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;

#[test]
fn redirected_mkdir_registers_only_a_usable_directory() {
    let root = std::env::temp_dir().join(format!("srx-mkdir-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let public = root.join("public");
    fs::create_dir(&public).unwrap();
    let identity = fs::metadata(&public).unwrap().ino();
    fs::write(root.join("file"), b"keep").unwrap();
    fs::set_permissions(root.join("file"), fs::Permissions::from_mode(0o600)).unwrap();
    let socket = UnixListener::bind(root.join("socket")).unwrap();
    fs::create_dir(root.join("existing")).unwrap();
    assert!(!platform::fs::is_directory(
        root.join("socket").to_str().unwrap()
    ));
    assert!(platform::fs::is_directory(public.to_str().unwrap()));
    for is_mapping in [false, true] {
        crate::hook::enter_provider_virtual_scope();
        for (index, (name, denied, expected)) in [
            ("file", false, libc::EEXIST),
            ("file/child", false, libc::ENOTDIR),
            ("absent/child", false, libc::ENOENT),
            ("socket", false, libc::EEXIST),
            ("absent", true, libc::EACCES),
            ("existing", true, 0),
            ("existing", false, 0),
            (if is_mapping { "mapped" } else { "sandbox" }, false, 0),
        ]
        .into_iter()
        .enumerate()
        {
            let target = root.join(name);
            let source = public.join(format!("{is_mapping}-{index}"));
            let source = source.to_str().unwrap();
            let decision = RedirectDecision {
                action: RedirectAction::Redirect,
                new_path: target.to_str().unwrap().into(),
                is_mapping,
            };
            let outcome =
                mkdir_redirect_target(InterceptHub::instance(), source, &decision, |path| {
                    if denied {
                        runtime::set_errno(libc::EACCES);
                        -1
                    } else {
                        // SAFETY: path 是本次回调借用的有效 C 字符串。
                        unsafe { libc::mkdir(path, 0o700) }
                    }
                });
            assert_eq!(
                outcome,
                (if expected == 0 { 0 } else { -1 }, expected),
                "{name}"
            );
            let registered = if is_mapping {
                crate::hook::process_mapped_dir_target(source)
            } else {
                crate::hook::provider_passthrough_virtual_dir_target(source)
            };
            assert_eq!(
                registered.as_deref(),
                (expected == 0).then_some(decision.new_path.as_str()),
                "{name}"
            );
        }
        crate::hook::exit_provider_virtual_scope();
    }
    assert_eq!(fs::metadata(&public).unwrap().ino(), identity);
    assert_eq!(fs::read_dir(&public).unwrap().count(), 0);
    assert_eq!(fs::read(root.join("file")).unwrap(), b"keep");
    assert_eq!(
        fs::metadata(root.join("file")).unwrap().mode() & 0o777,
        0o600
    );
    assert!(!root.join("absent").exists());
    drop(socket);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mapped_storage_mkdir_uses_the_backend_without_creating_the_source() {
    let name = format!("SrxMkdirBackend-{}", std::process::id());
    let view = format!("/storage/emulated/0/Download/{name}");
    let backend = std::path::Path::new("/data/media/0/Download").join(name);
    fs::create_dir(&backend).unwrap();
    fs::write(backend.join("file"), b"keep").unwrap();
    for (index, (target, expected)) in [("file", libc::EEXIST), ("dir", 0), ("dir", 0)]
        .into_iter()
        .enumerate()
    {
        let source = format!("{view}/source-{index}");
        let target_view = format!("{view}/{target}");
        let decision = RedirectDecision {
            action: RedirectAction::Redirect,
            new_path: target_view.clone(),
            is_mapping: true,
        };
        assert_eq!(
            mkdir_redirect_target(InterceptHub::instance(), &source, &decision, |_| {
                panic!("mapping unexpectedly invoked the view mkdir")
            }),
            (if expected == 0 { 0 } else { -1 }, expected)
        );
        assert_eq!(
            crate::hook::process_mapped_dir_target(&source),
            (expected == 0).then_some(target_view)
        );
        assert!(
            !backend
                .join(format!("source-{index}"))
                .try_exists()
                .unwrap()
        );
    }
    assert_eq!(fs::read(backend.join("file")).unwrap(), b"keep");
    fs::remove_dir(backend.join("dir")).unwrap();
    fs::remove_file(backend.join("file")).unwrap();
    fs::remove_dir(backend).unwrap();
}
