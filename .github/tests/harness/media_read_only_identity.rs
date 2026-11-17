extern crate self as log;
#[macro_export]
macro_rules! debug { ($($args:tt)*) => {}; }
use std::sync::atomic::{AtomicU8, Ordering};
static CASE: AtomicU8 = AtomicU8::new(0);
const READ_ONLY_DENY_REASON: &str = "deny_reason=read_only_rule";
fn normalize_storage_path(path: &str) -> String { path.into() }
fn read_only_audit_user_id(_: &[&String; 2]) -> i32 { 0 }
fn is_system_writer_uid(_: i32) -> bool { false }
fn infer_read_only_caller_package_by_path(_: i32, _: &[&String; 2]) -> Option<String> { None }
fn kind_from_code(_: i32) -> Option<i32> { Some(1) }
fn source_from_op_name(_: &str) -> &'static str { "media_fuse" }
fn sanitize_extra_value<'a>(value: &'a str, _: &str) -> &'a str { value }
mod libc { pub const EROFS: i32 = 30; }
mod context {
    pub fn get_effective_caller_pid_for_monitor() -> i32 { 42 }
    pub fn is_caller_attribution_stopped() -> bool { crate::CASE.load(crate::Ordering::Relaxed) == 2 }
    pub fn is_current_caller_scope_active() -> bool { true }
}
mod platform {
    pub const ANDROID_USER_ID_OFFSET: i32 = 100000;
    pub mod log_fields { pub fn encode(value: &str) -> &str { value } }
}
mod paths { pub fn extract_user_id_from_storage_path(_: &str) -> i32 { 0 } }
mod writer {
    pub const ANDROID_APP_UID_START: i32 = 10000;
    pub fn is_path_or_mapped_target_read_only_by_caller_paths(_: &str, pkg: &str, _: i32) -> bool { pkg == "test.first" }
}
mod policy {
    pub fn get_packages_for_uid(_: i32) -> Vec<String> { vec!["test.first".into(), "test.second".into()] }
    pub fn is_system_writer_package(_: &str) -> bool { false }
    pub fn get_fresh_uid_for_package_in_user(_: &str, _: i32) -> i32 { 10123 }
}
mod monitor {
    pub struct Identity { pub package_name: String }
    pub fn infer_recent_path_caller_identity(_: &str, _: i32) -> Option<Identity> {
        if crate::CASE.load(crate::Ordering::Relaxed) == 1 { return None; }
        Some(Identity { package_name: "test.first".into() })
    }
}
struct InterceptHub;
impl InterceptHub {
    fn instance() -> &'static Self { &Self }
    fn get_current_caller_package(&self) -> String {
        match CASE.load(Ordering::Relaxed) {
            3 => "test.first".into(),
            4 => String::new(),
            _ => "test.second".into(),
        }
    }
    fn get_current_caller_uid(&self) -> i32 { 10123 }
}
struct SettingsHub;
impl SettingsHub {
    fn instance() -> &'static Self { &Self }
    fn has_enabled_read_only_paths_for_user(&self, _: i32) -> bool { true }
    fn should_redirect(&self, pkg: &str, _: i32) -> bool { pkg == "test.first" }
}
struct AuditTrail;
impl AuditTrail {
    fn instance() -> &'static Self { &Self }
    fn record_operation_result(&self, _: i32, pkg: &str, _: &str, _: (i32, i32), _: &str, _: (i32, i32, Option<()>)) {
        assert_eq!(CASE.load(Ordering::Relaxed), 3);
        assert_eq!(pkg, "test.first");
    }
}
// __MEDIA_READ_ONLY_IMPLEMENTATION__

fn main() {
    for (case, expected) in [(0, false), (1, false), (2, false), (3, true), (4, false)] {
        CASE.store(case, Ordering::Relaxed);
        assert_eq!(record_read_only_fuse_operation(1, "open", "open:write", "/storage/emulated/0/Download/a.txt", "", 10123, 1), expected, "case {case}");
    }
    println!("MEDIA READ-ONLY IDENTITY CASES PASSED");
}
