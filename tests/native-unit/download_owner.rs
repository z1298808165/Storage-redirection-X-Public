use super::{package_uid_matches_download_row, unique_download_owner};

#[test]
fn duplicate_path_rows_must_agree_on_the_concrete_owner() {
    let first = Some(("com.example.first".to_string(), 10_042));
    let second = Some(("com.example.second".to_string(), 10_042));
    assert_eq!(
        unique_download_owner([first.clone(), first.clone()]),
        Some(first.clone())
    );
    assert_eq!(unique_download_owner([first.clone(), second]), None);
    assert_eq!(unique_download_owner([first, None]), None);
    assert_eq!(unique_download_owner([]), Some(None));
}

#[test]
fn download_owner_uid_must_match_package_and_user() {
    assert!(package_uid_matches_download_row(10_042, 10_042, 0));
    assert!(!package_uid_matches_download_row(10_042, 10_043, 0));
    assert!(!package_uid_matches_download_row(110_042, 110_042, 0));
    assert!(!package_uid_matches_download_row(1_000, 1_000, 0));
}
