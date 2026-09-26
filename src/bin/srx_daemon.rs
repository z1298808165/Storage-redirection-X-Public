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
#[path = "../daemon_mount_diag.rs"]
mod daemon_mount_diag;
#[path = "../daemon_mount_reclaim.rs"]
mod daemon_mount_reclaim;
#[path = "../domain.rs"]
mod domain;
#[path = "../fuse_host.rs"]
mod fuse_host;
#[path = "../fuse_redirect/mod.rs"]
mod fuse_redirect;
#[path = "../fuse_supervisor.rs"]
mod fuse_supervisor;
#[path = "../log_daemon.rs"]
mod log_daemon;
#[path = "../logging.rs"]
mod logging;
#[path = "../metadata_repair.rs"]
mod metadata_repair;
#[path = "../module_mount_source.rs"]
mod module_mount_source;
#[path = "../mount.rs"]
mod mount;
#[path = "../mount_identity.rs"]
mod mount_identity;
#[path = "../mount_intent.rs"]
mod mount_intent;
#[path = "../mount_ledger.rs"]
mod mount_ledger;
#[path = "../platform.rs"]
mod platform;
#[path = "../redirect/policy.rs"]
mod redirect_policy;
mod redirect {
    pub(crate) use crate::redirect_policy as policy;
}
#[path = "../runtime_control.rs"]
mod runtime_control;
#[path = "../runtime_stats.rs"]
mod runtime_stats;
#[path = "../system_fuse_view.rs"]
mod system_fuse_view;

fn main() {
    let mut args = std::env::args();
    let _ = args.next();
    let command = args.next();
    if command.as_deref() == Some("cleanup-mounts") {
        std::process::exit(if daemon_mount::cleanup_all_mount_states() {
            0
        } else {
            1
        });
    }
    if command.as_deref() == Some("doctor") {
        let doctor_args: Vec<String> = args.collect();
        std::process::exit(daemon_mount::doctor_report(&doctor_args));
    }
    if command.as_deref() == Some("control") {
        let Some(command) = args.next() else {
            eprintln!("usage: srx_daemon control <command>");
            std::process::exit(2);
        };
        std::process::exit(if log_daemon::send_control(&command).is_ok() {
            0
        } else {
            1
        });
    }
    std::process::exit(daemon::main_entry());
}
