mod engine;
pub mod policy;
mod router;
mod thumbnail_diag;
pub(crate) mod writer;

pub use engine::{
    process_dir_create_redirect_path, process_redirect_path, process_write_redirect_path,
    record_redirect_hit, resolve_mapping_request_owner_for_path,
};
pub use router::{PathRouter, RedirectAction, RedirectDecision};

/// 供 hook 配置监视线程在配置变更时重建进程内路由（见 hook::config_watcher）。
pub(crate) use engine::reconfigure_router_from_settings;
