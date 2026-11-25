use super::{CallerRowPlan, fast_row_action, should_hide_missing_rewritten_row};

#[test]
fn plain_item_visibility_requires_its_private_file() {
    let dir = std::env::temp_dir().join(format!("srx-cursor-item-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let private_file = dir.join("visible.txt");
    std::fs::write(&private_file, b"visible").unwrap();
    let plan = CallerRowPlan::Redirect {
        target: dir.to_str().unwrap().to_owned(),
    };
    let results = [false, true].map(|single_item| {
        fast_row_action(
            &plan,
            "/storage/emulated/0/visible.txt",
            false,
            10123,
            single_item,
        )
    });
    std::fs::remove_file(&private_file).unwrap();
    std::fs::remove_dir(dir).unwrap();
    for visible in results {
        assert_eq!(
            visible,
            Some(Some(private_file.to_str().unwrap().to_owned()))
        );
    }
    assert_eq!(
        fast_row_action(
            &plan,
            "/storage/emulated/0/.pending-1234-test.txt",
            false,
            10123,
            false
        ),
        None
    );
    assert_eq!(
        fast_row_action(
            &plan,
            "/storage/emulated/0/.pending-1234-test.txt",
            false,
            10123,
            true
        ),
        None
    );
}

#[test]
fn pending_rows_survive_a_missing_rewritten_target() {
    assert!(!should_hide_missing_rewritten_row(
        "/storage/emulated/0/.pending-1234-test.txt",
        false,
        false
    ));
    assert!(should_hide_missing_rewritten_row(
        "/storage/emulated/0/absent.txt",
        false,
        false
    ));
    assert!(!should_hide_missing_rewritten_row(
        "/storage/emulated/0/absent.txt",
        false,
        true
    ));
    assert!(!should_hide_missing_rewritten_row(
        "/storage/emulated/0/.pending-1234-test.txt",
        true,
        false
    ));
}
