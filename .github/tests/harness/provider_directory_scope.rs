use std::cell::{Cell, RefCell};
use std::fs;
use std::path::Path;

const PROVIDER_PASSTHROUGH_VIRTUAL_DIR_MAX: usize = 16;
thread_local! {
    static PROVIDER_PASSTHROUGH_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PROVIDER_VIRTUAL_SCOPE_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PROVIDER_PASSTHROUGH_VIRTUAL_DIRS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

fn virtual_dir_key(path: &str) -> String { path.to_owned() }
fn is_current_caller_scope_active() -> bool { true }
fn clear_current_caller() { unreachable!() }
fn clear_binder_saved_caller_uid() { unreachable!() }
fn set_binder_identity_cleared(_: bool) { unreachable!() }

fn main() {
    fs::create_dir("sandbox").unwrap();
    fs::create_dir("preexisting").unwrap();
    let paths = ["preexisting", "created_by_other", "absent"];
    for virtual_first in [true, false] {
        let (enter_outer, exit_outer, enter_inner, exit_inner): (fn(), fn(), fn(), fn()) = if virtual_first {
            (enter_provider_virtual_scope, exit_provider_virtual_scope, enter_provider_passthrough, exit_provider_passthrough)
        } else {
            (enter_provider_passthrough, exit_provider_passthrough, enter_provider_virtual_scope, exit_provider_virtual_scope)
        };
        remember_provider_passthrough_virtual_dir("outside", "sandbox");
        enter_outer();
        assert_eq!(provider_passthrough_virtual_dir_target("outside"), None);
        enter_inner();
        enter_inner();
        for path in paths { remember_provider_passthrough_virtual_dir(path, "sandbox"); }
        fs::create_dir("created_by_other").unwrap();
        exit_inner();
        exit_inner();
        for path in paths { assert_eq!(provider_passthrough_virtual_dir_target(path).as_deref(), Some("sandbox")); }
        exit_outer();
        for path in paths { assert_eq!(provider_passthrough_virtual_dir_target(path), None); }
        assert!(Path::new("preexisting").is_dir() && Path::new("created_by_other").is_dir());
        assert!(!Path::new("absent").exists());
        fs::remove_dir("created_by_other").unwrap();
    }
}
