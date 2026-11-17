use super::{FuseRedirectFs, metadata::StagedEntry};
use std::ffi::OsStr;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

#[test]
fn failed_staging_preserves_a_competing_destination() {
    let directory = std::env::temp_dir().join(format!(
        "srx-staged-entry-{}-{}",
        std::process::id(),
        NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let destination = directory.join("entry");
    let staged = StagedEntry::new(&destination).unwrap();
    let staging_directory = staged.path().parent().unwrap().to_path_buf();
    std::fs::write(staged.path(), b"staged").unwrap();
    std::fs::write(&destination, b"replacement").unwrap();

    drop(staged);
    assert_eq!(std::fs::read(&destination).unwrap(), b"replacement");
    assert!(!staging_directory.exists());

    let staged = StagedEntry::new(&destination).unwrap();
    let staging_directory = staged.path().parent().unwrap().to_path_buf();
    std::fs::write(staged.path(), b"staged").unwrap();
    assert!(staged.publish(&destination).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"replacement");
    assert!(!staging_directory.exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn staging_directory_names_are_reserved() {
    assert!(FuseRedirectFs::child_rel("", OsStr::new(".srx-staging-1-0")).is_err());
    assert!(FuseRedirectFs::child_rel("parent", OsStr::new(".srx-staging-2-0")).is_err());
    assert_eq!(
        FuseRedirectFs::child_rel("parent", OsStr::new("entry")).unwrap(),
        "parent/entry"
    );
}

#[test]
fn publishing_staged_entries_removes_only_the_staging_directory() {
    let directory = std::env::temp_dir().join(format!(
        "srx-staged-entry-{}-{}",
        std::process::id(),
        NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    for is_dir in [false, true] {
        let destination = directory.join(if is_dir { "directory" } else { "file" });
        let staged = StagedEntry::new(&destination).unwrap();
        let staging_directory = staged.path().parent().unwrap().to_path_buf();
        if is_dir {
            std::fs::create_dir(staged.path()).unwrap();
        } else {
            std::fs::write(staged.path(), b"published").unwrap();
        }
        staged.publish(&destination).unwrap();
        assert!(!staging_directory.exists());
        assert_eq!(destination.is_dir(), is_dir);
        if !is_dir {
            assert_eq!(std::fs::read(&destination).unwrap(), b"published");
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
