mod boot;
mod companion;
mod companion_mount;
mod companion_request;
mod mount_timing;
mod specialize_post;
mod specialize_pre;

use crate::java_hook;
use crate::logging::Logger;
use crate::platform::module_paths;
use crate::zygisk::{abi, jni};
use serde::Deserialize;

#[derive(Default, Deserialize)]
struct ServerGlobalConfig {
    #[serde(default)]
    auto_enable_redirect_for_new_apps: bool,
}

pub use companion::run_companion_pipeline;

pub struct RuntimeFlow {
    api: Option<abi::Api>,
    env: *mut jni_sys::JNIEnv,
    package_name: String,
    app_data_dir: String,
    app_pid: i32,
    app_uid: i32,
    should_redirect: bool,
    should_monitor: bool,
    is_mount_applied: bool,
    is_mount_request_sent: bool,
    deferred_mount_payload: String,
    is_system_writer_hook_redirect: bool,
    should_install_app_redirect_hook: bool,
    is_system_writer_boot_lite: bool,
    is_file_monitor_ui: bool,
    should_install_fuse_fix: bool,
    should_skip_post_work: bool,
    should_keep_module_loaded: bool,
    module_dir_fd: i32,
}

// SAFETY: 实例只通过全局 Mutex 串行访问，裸指针只做句柄透传
unsafe impl Send for RuntimeFlow {}
// SAFETY: 共享访问由 Mutex 同步，当前实现不在并发路径解引用裸指针
unsafe impl Sync for RuntimeFlow {}

impl RuntimeFlow {
    pub fn new() -> Self {
        Self {
            api: None,
            env: std::ptr::null_mut(),
            package_name: String::new(),
            app_data_dir: String::new(),
            app_pid: -1,
            app_uid: -1,
            should_redirect: false,
            should_monitor: false,
            is_mount_applied: false,
            is_mount_request_sent: false,
            deferred_mount_payload: String::new(),
            is_system_writer_hook_redirect: false,
            should_install_app_redirect_hook: false,
            is_system_writer_boot_lite: false,
            is_file_monitor_ui: false,
            should_install_fuse_fix: false,
            should_skip_post_work: false,
            should_keep_module_loaded: false,
            module_dir_fd: -1,
        }
    }

    pub fn on_load(&mut self, api: abi::Api, env: *mut jni_sys::JNIEnv) {
        Logger::init(Some("zygisk"));
        self.api = Some(api);
        self.env = env;
        self.module_dir_fd = -1;
        jni::init_java_vm(env);
        boot::log_boot_summary_once();
    }

    pub fn pre_server_specialize(&mut self) {
        // system_server 在 specialize 后可能无法再直接获取模块 fd。
        // 跨 specialize 保留该 fd，使接收器路径可通过 /proc/self/fd
        // 读取配置并写入包事件。
        self.open_server_module_dir_fd();
    }

    pub fn post_server_specialize(&mut self, _args: *const abi::ServerSpecializeArgs) {
        Logger::init(Some("system_server"));
        let module_dir = self.server_module_dir_path();
        if should_install_package_event_receiver(&module_dir) {
            if java_hook::install_package_event_receiver(self.env, &module_dir) {
                log::info!("package event receiver installed");
            } else {
                log::warn!("package event receiver install failed");
                self.close_server_module_dir_fd();
            }
        } else {
            log::info!("package event receiver skipped: auto new apps disabled");
            self.close_server_module_dir_fd();
        }
        if let Some(api) = self.api.as_ref() {
            api.set_option(abi::ZygiskOption::DlcloseModuleLibrary);
        }
    }

    fn server_module_dir_path(&mut self) -> String {
        self.open_server_module_dir_fd();

        if self.module_dir_fd >= 0 {
            format!("/proc/self/fd/{}", self.module_dir_fd)
        } else {
            module_paths::MODULE_DIR.to_string()
        }
    }

    fn open_server_module_dir_fd(&mut self) {
        if self.module_dir_fd >= 0 {
            return;
        }
        let Some(api) = self.api.as_ref() else {
            return;
        };
        let fd = api.get_module_dir();
        if fd >= 0 {
            let _ = api.exempt_fd(fd);
            self.module_dir_fd = fd;
        }
    }

    fn close_server_module_dir_fd(&mut self) {
        if self.module_dir_fd < 0 {
            return;
        }
        let fd = self.module_dir_fd;
        self.module_dir_fd = -1;
        unsafe {
            libc::close(fd);
        }
    }
}

fn should_install_package_event_receiver(module_dir: &str) -> bool {
    let path = if module_dir.is_empty() {
        format!("{}/global.json", module_paths::CONFIG_DIR)
    } else {
        format!("{module_dir}/config/global.json")
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    parse_auto_enable_redirect_for_new_apps(&content)
}

fn parse_auto_enable_redirect_for_new_apps(content: &str) -> bool {
    serde_json::from_str::<ServerGlobalConfig>(content)
        .map(|config| config.auto_enable_redirect_for_new_apps)
        .unwrap_or(false)
}
