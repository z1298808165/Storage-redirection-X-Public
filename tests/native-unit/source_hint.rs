use super::{PathCallerHint, infer_from_path_hints};
use crate::platform::paths;

#[test]
fn known_root_and_shell_uids_cannot_publish_application_path_hints_without_a_scope() {
    for uid in [0, 2000] {
        assert!(
            super::remember_public_path_caller_hint_with_op_filter(
                "/storage/emulated/0/Download/terminal-hint.txt",
                "com.android.shell",
                uid,
                "provider_open",
                "high",
                "provider_open:create",
                "",
            )
            .is_empty()
        );
    }
}

fn hint(package_name: &str, updated_ms: i64) -> PathCallerHint {
    PathCallerHint {
        user_id: 0,
        updated_ms,
        package_name: package_name.to_string(),
        path: "/storage/emulated/0/Download/shared.txt".to_string(),
        source: "provider_open",
        confidence: "high",
        op_filter: "provider_open:create",
        correlation_id: format!("{package_name}-{updated_ms}"),
    }
}

#[test]
fn conflicting_packages_for_one_path_remain_unresolved() {
    let now_ms = paths::monotonic_ms();
    let first = hint("com.example.first", now_ms);
    let second = hint("com.example.second", now_ms.saturating_sub(1));

    assert!(
        infer_from_path_hints(
            [&first, &second].into_iter(),
            0,
            "/storage/emulated/0/Download/shared.txt",
            false,
        )
        .is_none()
    );
}

#[test]
fn repeated_hints_for_one_package_keep_the_best_record() {
    let now_ms = paths::monotonic_ms();
    let older = hint("com.example.owner", now_ms.saturating_sub(1));
    let newer = hint("com.example.owner", now_ms);

    assert_eq!(
        infer_from_path_hints(
            [&older, &newer].into_iter(),
            0,
            "/storage/emulated/0/Download/shared.txt",
            false,
        )
        .map(|identity| identity.package_name),
        Some("com.example.owner".to_string())
    );
}
