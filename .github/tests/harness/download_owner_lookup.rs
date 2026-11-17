use std::sync::atomic::{AtomicUsize, Ordering};

static MODE: AtomicUsize = AtomicUsize::new(0);
static QUERIES: AtomicUsize = AtomicUsize::new(0);
const STORAGE_PREFIX: &str = "/storage/emulated/";

mod paths {
    pub fn starts_with(path: &str, prefix: &str) -> bool {
        path.starts_with(prefix)
    }
    pub fn parent(path: &str) -> String {
        path.rsplit_once('/').unwrap().0.to_owned()
    }
}
mod platform {
    pub fn user_id_from_uid(uid: i32) -> i32 {
        uid / 100_000
    }
}
mod redirect {
    pub mod writer {
        pub const ANDROID_APP_UID_START: i32 = 10_000;
    }
}
mod policy {
    pub fn is_system_writer_package(package: &str) -> bool {
        package == "com.android.providers.downloads"
    }
    pub fn get_fresh_uid_for_package_in_user(package: &str, _: i32) -> i32 {
        match package {
            "com.example.a" => 10_123,
            "com.example.b" => 10_124,
            "com.example.c" => 10_125,
            _ => -1,
        }
    }
}
fn host_process_may_query_downloads() -> bool {
    true
}
fn query_download_owner_identity(path: &str) -> Option<Option<(String, i32)>> {
    QUERIES.fetch_add(1, Ordering::Relaxed);
    if !path.ends_with("/.file.js") {
        return Some(Some(("com.example.c".into(), 10_125)));
    }
    let a = Some(("com.example.a".to_owned(), 10_123));
    match MODE.load(Ordering::Relaxed) {
        0 => unique_download_owner([a, Some(("com.example.b".into(), 10_124))]),
        1 => unique_download_owner([]),
        2 => unique_download_owner([a.clone(), a]),
        3 => unique_download_owner([a, None]),
        4 => None,
        5 => Some(Some(("com.example.a".into(), 10_124))),
        6 => Some(Some(("com.android.providers.downloads".into(), 10_123))),
        _ => Some(Some(("com.example.a".into(), 110_123))),
    }
}

// __DOWNLOAD_OWNER_LOOKUP__

fn main() {
    for mode in 0..8 {
        MODE.store(mode, Ordering::Relaxed);
        QUERIES.store(0, Ordering::Relaxed);
        let owner = infer_download_owner_identity_by_path(
            "/storage/emulated/0/Download/.file.js",
            0,
        );
        let expected = match mode {
            1 => Some(("com.example.c".into(), 10_125)),
            2 => Some(("com.example.a".into(), 10_123)),
            _ => None,
        };
        assert_eq!(owner, expected, "lookup mode {mode}");
        assert_eq!(QUERIES.load(Ordering::Relaxed), if mode == 1 { 2 } else { 1 });
    }
    println!("DOWNLOAD OWNER LOOKUP CASES PASSED");
}
