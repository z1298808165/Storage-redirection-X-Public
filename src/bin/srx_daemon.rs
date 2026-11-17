#![allow(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]
#![allow(clippy::missing_const_for_thread_local)]

#[cfg(target_os = "android")]
#[path = "../config.rs"]
mod config;
#[cfg(target_os = "android")]
#[path = "../daemon.rs"]
mod daemon;
#[cfg(target_os = "android")]
#[path = "../daemon_identity_query.rs"]
mod daemon_identity_query;
#[cfg(target_os = "android")]
#[path = "../daemon_monitor.rs"]
mod daemon_monitor;
#[cfg(target_os = "android")]
#[path = "../daemon_mount.rs"]
mod daemon_mount;
#[cfg(target_os = "android")]
#[path = "../daemon_mount_diag.rs"]
mod daemon_mount_diag;
#[cfg(target_os = "android")]
#[path = "../daemon_mount_reclaim.rs"]
mod daemon_mount_reclaim;
#[cfg(target_os = "android")]
#[path = "../daemon_watchdog.rs"]
mod daemon_watchdog;
#[cfg(target_os = "android")]
#[path = "../domain.rs"]
mod domain;
#[cfg(target_os = "android")]
#[path = "../fuse_host.rs"]
mod fuse_host;
#[cfg(target_os = "android")]
#[path = "../fuse_host_control.rs"]
mod fuse_host_control;
#[cfg(target_os = "android")]
#[path = "../fuse_redirect/mod.rs"]
mod fuse_redirect;
#[cfg(target_os = "android")]
#[path = "../fuse_session.rs"]
mod fuse_session;
#[cfg(target_os = "android")]
#[path = "../fuse_supervisor.rs"]
mod fuse_supervisor;
#[cfg(target_os = "android")]
#[path = "../fuse_terminate.rs"]
mod fuse_terminate;
#[cfg(target_os = "android")]
#[path = "../log_daemon.rs"]
mod log_daemon;
#[cfg(target_os = "android")]
#[path = "../logging.rs"]
mod logging;
#[cfg(target_os = "android")]
#[path = "../metadata_repair.rs"]
mod metadata_repair;
#[cfg(target_os = "android")]
#[path = "../module_mount_source.rs"]
mod module_mount_source;
#[cfg(target_os = "android")]
#[path = "monitor/mod.rs"]
mod monitor;
#[cfg(target_os = "android")]
#[path = "../mount.rs"]
mod mount;
#[cfg(target_os = "android")]
#[path = "../mount_identity.rs"]
mod mount_identity;
#[cfg(target_os = "android")]
#[path = "../mount_intent.rs"]
mod mount_intent;
#[cfg(target_os = "android")]
#[path = "../mount_ledger.rs"]
mod mount_ledger;
#[cfg(target_os = "android")]
#[path = "../owner_migration.rs"]
mod owner_migration;
#[cfg(target_os = "android")]
#[path = "../platform.rs"]
mod platform;
#[cfg(target_os = "android")]
#[path = "../redirect/policy.rs"]
mod redirect_policy;
#[cfg(target_os = "android")]
mod redirect {
    pub(crate) use crate::redirect_policy as policy;
}
#[cfg(target_os = "android")]
#[path = "../runtime_control.rs"]
mod runtime_control;
#[cfg(target_os = "android")]
#[path = "../runtime_stats.rs"]
mod runtime_stats;
#[cfg(target_os = "android")]
#[path = "../system_fuse_view.rs"]
mod system_fuse_view;

#[cfg(target_os = "android")]
fn main() {
    let mut args = std::env::args();
    let _ = args.next();
    let command = args.next();
    if command.as_deref() == Some("package-query") {
        let fields: Vec<String> = args.collect();
        let request = if fields.is_empty() {
            "L".to_owned()
        } else {
            fields.join("\t")
        };
        match platform::packages::query(&request).and_then(|reply| {
            platform::packages::parse_uids(&reply)?;
            Ok(reply)
        }) {
            Ok(reply) => print!("{reply}"),
            Err(error) => {
                eprintln!("package query failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
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
    if command.as_deref() == Some("watchdog") {
        // 守护进程入口：由 daemon fork+exec 出来的独立进程，进程名已改写，
        // 不会与主 daemon 的 pidof/存活检测混淆。参数为被观察的 daemon pid。
        let target_pid = args.next().and_then(|pid| pid.parse::<i32>().ok());
        std::process::exit(daemon_watchdog::entry(target_pid));
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

#[cfg(not(target_os = "android"))]
fn main() {
    eprintln!("srx_daemon requires Android");
    std::process::exit(1);
}
