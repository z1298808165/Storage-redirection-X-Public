#![cfg(target_os = "android")]
#![allow(clippy::missing_safety_doc)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_const_for_thread_local)]

mod config;
mod domain;
mod fuse_redirect;
mod hook;
mod java_hook;
mod legacy_mount_marker;
mod lifecycle;
mod logging;
mod metadata_repair;
mod module_mount_source;
mod monitor;
mod mount;
mod mount_intent;
mod mount_ledger;
mod platform;
mod redirect;
mod runtime_control;
mod runtime_stats;
mod zygisk;
