use std::ffi::{c_char, c_void};

pub const ZYGISK_NEXT_API_VERSION: i32 = 4;
pub const ZYGISK_NEXT_HYOS_API_VERSION: i32 = 1;
pub const ZN_RUNTIME_HYOS: i32 = 1;

/// Zygisk Next API 表的前 10 个字段。顺序必须与 zygisk_next_api.h 完全一致；
/// 这里只保存函数指针，不解释其内部实现。未使用的字段保留为 `*mut c_void`，
/// 仅用于维持 ABI 布局。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ZygiskNextAPI {
    pub plt_hook: *mut c_void,
    pub inline_hook: *mut c_void,
    pub inline_unhook: *mut c_void,
    pub new_symbol_resolver: *mut c_void,
    pub free_symbol_resolver: *mut c_void,
    pub get_base_address: *mut c_void,
    pub symbol_lookup: *mut c_void,
    pub for_each_symbols: *mut c_void,
    pub connect_companion: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    pub get_runtime: Option<unsafe extern "C" fn() -> *const ZygiskNextRuntime>,
}

#[repr(C)]
pub struct ZygiskNextRuntime {
    pub runtime_type: i32,
    pub api_version: i32,
    pub register_module: Option<unsafe extern "C" fn(*const c_void) -> i32>,
}

/// Hyos runtime 在 specialize 后传给模块的只读身份参数；字符串只在回调期间有效。
#[repr(C)]
pub struct ZnHyosAppSpecializeArgs {
    pub process_name: *const c_char,
    pub package_name: *const c_char,
    pub se_info: *const c_char,
}

/// Hyos runtime 模块注册结构；registerModule 会在返回前复制该结构。
#[repr(C)]
pub struct ZygiskNextHyosModule {
    pub target_api_version: i32,
    pub on_app_specialized: Option<unsafe extern "C" fn(*const ZnHyosAppSpecializeArgs)>,
}

/// 传给 Zygisk Next 的模块入口；导出符号名固定为 `zn_module`。
#[repr(C)]
pub struct ZygiskNextModule {
    pub target_api_version: i32,
    pub on_module_loaded: Option<unsafe extern "C" fn(*mut c_void, *const ZygiskNextAPI)>,
}

#[repr(C)]
pub struct ZygiskNextCompanionModule {
    pub target_api_version: i32,
    pub on_companion_loaded: Option<unsafe extern "C" fn()>,
    pub on_module_connected: Option<unsafe extern "C" fn(i32)>,
}
