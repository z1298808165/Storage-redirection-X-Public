#![cfg(target_os = "android")]
#![allow(clippy::missing_safety_doc)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_const_for_thread_local)]

mod config;
mod domain;
mod fuse_redirect;
mod hook;
mod java_hook;
mod lifecycle;
mod logging;
mod monitor;
mod mount;
mod mount_status_marker;
mod platform;
mod redirect;
mod runtime_control;
mod runtime_stats;
mod zygisk;
