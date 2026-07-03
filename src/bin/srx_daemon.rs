#![cfg(target_os = "android")]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]
#![allow(clippy::missing_const_for_thread_local)]

#[path = "../config.rs"]
mod config;
#[path = "../daemon.rs"]
mod daemon;
#[path = "../daemon_monitor.rs"]
mod daemon_monitor;
#[path = "../daemon_mount.rs"]
mod daemon_mount;
#[path = "../domain.rs"]
mod domain;
#[path = "../fuse_redirect.rs"]
mod fuse_redirect;
#[path = "../logging.rs"]
mod logging;
#[path = "../mount.rs"]
mod mount;
#[path = "../mount_status_marker.rs"]
mod mount_status_marker;
#[path = "../platform.rs"]
mod platform;
#[path = "../redirect/policy.rs"]
mod redirect_policy;
#[cfg(test)]
#[path = "../redirect/writer.rs"]
mod redirect_writer;
mod redirect {
    pub(crate) use crate::redirect_policy as policy;
    #[cfg(test)]
    pub(crate) use crate::redirect_writer as writer;
}
#[path = "../runtime_control.rs"]
mod runtime_control;

#[cfg(test)]
mod hook {
    use std::cell::Cell;

    thread_local! {
        static PATH_OWNER_INFERENCE_DISABLED_DEPTH: Cell<u32> = const { Cell::new(0) };
    }

    pub struct PathOwnerInferenceGuard;

    pub fn enter_path_owner_inference_disabled() -> PathOwnerInferenceGuard {
        PATH_OWNER_INFERENCE_DISABLED_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        PathOwnerInferenceGuard
    }

    impl Drop for PathOwnerInferenceGuard {
        fn drop(&mut self) {
            PATH_OWNER_INFERENCE_DISABLED_DEPTH.with(|depth| {
                let current = depth.get();
                if current > 0 {
                    depth.set(current - 1);
                }
            });
        }
    }

    pub fn is_path_owner_inference_disabled() -> bool {
        PATH_OWNER_INFERENCE_DISABLED_DEPTH.with(|depth| depth.get() > 0)
    }
}

fn main() {
    std::process::exit(daemon::main_entry());
}
