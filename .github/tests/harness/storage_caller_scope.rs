use std::sync::atomic::{AtomicBool, Ordering};
static ACTIVE: AtomicBool = AtomicBool::new(true);
static TERMINAL: AtomicBool = AtomicBool::new(false);
static AMBIGUOUS: AtomicBool = AtomicBool::new(false);
static DOWNLOAD: AtomicBool = AtomicBool::new(false);
struct InterceptHub;
impl InterceptHub {
    fn instance() -> &'static Self { &Self }
    fn get_current_caller_uid(&self) -> i32 { 10301 }
    fn get_current_caller_package(&self) -> String {
        if AMBIGUOUS.load(Ordering::Relaxed) { String::new() } else { "test.second".into() }
    }
}
mod hook {
    pub fn is_caller_attribution_stopped() -> bool { crate::TERMINAL.load(crate::Ordering::Relaxed) }
    pub fn is_current_caller_scope_active() -> bool { crate::ACTIVE.load(crate::Ordering::Relaxed) }
}
mod platform { pub fn user_id_from_uid(uid: i32) -> i32 { uid / 100000 } }
mod paths { pub fn extract_android_private_path_owner(_: &str) -> String { String::new() } }
mod policy { pub fn is_system_writer_package(_: &str) -> bool { false } }
fn is_system_writer_uid(_: i32) -> bool { true }
fn resolve_proxied_write_caller_context(_: i32, _: i32, _: &str) -> Option<(String, i32)> { Some(("test.first".into(), 10301)) }
fn resolve_caller_package(_: i32, _: &str) -> String { "test.second".into() }
fn resolve_mapping_request_caller_context(_: i32, _: i32, _: &str, _: bool) -> Option<(String, i32)> { None }
fn resolve_download_owner_caller_context(_: i32, _: &str) -> Option<(String, i32)> {
    DOWNLOAD.load(Ordering::Relaxed).then(|| ("test.download.owner".into(), 10401))
}
// __STORAGE_CALLER_SCOPE__
fn main() {
    let path = "/storage/emulated/0/Download/shared.txt";
    assert_eq!(resolve_storage_caller_context(10301, path), ("test.second".into(), 10301));
    AMBIGUOUS.store(true, Ordering::Relaxed);
    assert_eq!(resolve_storage_caller_context(10301, path), (String::new(), 10301));
    DOWNLOAD.store(true, Ordering::Relaxed);
    assert_eq!(resolve_storage_caller_context(10301, path), ("test.download.owner".into(), 10401));
    ACTIVE.store(false, Ordering::Relaxed);
    assert_eq!(resolve_storage_caller_context(10301, path), ("test.first".into(), 10301));
    TERMINAL.store(true, Ordering::Relaxed);
    assert_eq!(resolve_storage_caller_context(10301, path), (String::new(), 10301));
    println!("STORAGE CALLER SCOPE CASES PASSED");
}
