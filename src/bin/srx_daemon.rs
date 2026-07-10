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
mod redirect {
    pub(crate) use crate::redirect_policy as policy;
}
#[path = "../runtime_control.rs"]
mod runtime_control;

fn main() {
    std::process::exit(daemon::main_entry());
}
