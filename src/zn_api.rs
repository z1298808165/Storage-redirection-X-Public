use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

const ZYGISK_NEXT_API_VERSION: i32 = 4;
const ZYGISK_NEXT_HYOS_API_VERSION: i32 = 1;
const ZN_RUNTIME_HYOS: i32 = 1;

/// Zygisk Next API 表的前 10 个字段。顺序必须与 zygisk_next_api.h 完全一致；
/// 这里只保存函数指针，不解释其内部实现。未使用的字段保留为 `*mut c_void`，
/// 仅用于维持 ABI 布局。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ZygiskNextAPI {
    plt_hook: *mut c_void,
    inline_hook: *mut c_void,
    inline_unhook: *mut c_void,
    new_symbol_resolver: *mut c_void,
    free_symbol_resolver: *mut c_void,
    get_base_address: *mut c_void,
    symbol_lookup: *mut c_void,
    for_each_symbols: *mut c_void,
    connect_companion: *mut c_void,
    get_runtime: Option<unsafe extern "C" fn() -> *const ZygiskNextRuntime>,
}

#[repr(C)]
pub struct ZygiskNextRuntime {
    runtime_type: i32,
    api_version: i32,
    register_module: Option<unsafe extern "C" fn(*const c_void) -> i32>,
}

/// Hyos runtime 在 specialize 后传给模块的只读身份参数；字符串只在回调期间有效。
#[repr(C)]
pub struct ZnHyosAppSpecializeArgs {
    process_name: *const c_char,
    package_name: *const c_char,
    se_info: *const c_char,
}

/// Hyos runtime 模块注册结构；registerModule 会在返回前复制该结构。
#[repr(C)]
pub struct ZygiskNextHyosModule {
    target_api_version: i32,
    on_app_specialized: Option<unsafe extern "C" fn(*const ZnHyosAppSpecializeArgs)>,
}

/// 传给 Zygisk Next 的模块入口；导出符号名固定为 `zn_module`。
#[repr(C)]
pub struct ZygiskNextModule {
    pub target_api_version: i32,
    pub on_module_loaded: Option<unsafe extern "C" fn(*mut c_void, *const ZygiskNextAPI)>,
}

/// 复制后的 API 表。Zygisk Next 回调返回后原始指针不保证可继续使用，
/// 因此这里 `Box::leak` 一份到进程生命周期；每个进程只会加载一次。
static ZN_API: AtomicPtr<ZygiskNextAPI> = AtomicPtr::new(std::ptr::null_mut());
static HYOS_REGISTERED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_module_loaded(_handle: *mut c_void, api: *const ZygiskNextAPI) {
    if api.is_null() {
        return;
    }
    // SAFETY: api 当前有效；复制的是纯 POD 函数指针表。
    let api_copy = Box::into_raw(Box::new(unsafe { std::ptr::read(api) }));
    ZN_API.store(api_copy, Ordering::Release);
    // SAFETY: api_copy 指向刚复制且泄漏到进程生命周期的 API 表。
    let api_ref = unsafe { &*api_copy };
    log::info!("zygisk next api loaded");
    let _ = register_hyos_runtime(api_ref);
}

/// 查询当前进程是否返回了 Hyos runtime；只有 `getRuntime` 存在且类型/版本
/// 匹配时才注册。返回 false 不表示 Zygisk Next 不可用，只表示不是 Hyos。
fn register_hyos_runtime(api: &ZygiskNextAPI) -> bool {
    let Some(get_runtime) = api.get_runtime else {
        return false;
    };
    // SAFETY: getRuntime 的返回值按 API 契约在进程生命周期内有效。
    let runtime = unsafe { get_runtime() };
    if runtime.is_null() {
        return false;
    }
    // SAFETY: runtime 非空且由 Zygisk Next 返回。
    let runtime_ref = unsafe { &*runtime };
    if runtime_ref.runtime_type != ZN_RUNTIME_HYOS
        || runtime_ref.api_version < ZYGISK_NEXT_HYOS_API_VERSION
    {
        return false;
    }
    let Some(register_module) = runtime_ref.register_module else {
        return false;
    };
    crate::hook::hyos::prepare_pre_fork();
    // SAFETY: 结构体按 Hyos API 布局构造；registerModule 会在返回前复制。
    let result =
        unsafe { register_module(&HYOS_MODULE as *const ZygiskNextHyosModule as *const c_void) };
    let ok = result == 0;
    HYOS_REGISTERED.store(ok, Ordering::Release);
    log::info!("zygisk next hyos register result={result}");
    ok
}

unsafe extern "C" fn on_hyos_app_specialized(args: *const ZnHyosAppSpecializeArgs) {
    if args.is_null() {
        return;
    }
    // SAFETY: args 由 Zygisk Next 在回调期间保持有效。
    let args = unsafe { &*args };
    let process_name = cstr_to_string(args.process_name);
    let package_name = cstr_to_string(args.package_name);
    if !hyos_registered() {
        log::warn!(
            "hyos specialize ignored: module not registered pkg={} proc={}",
            package_name,
            process_name
        );
        return;
    }
    if !valid_hyos_identity(&package_name, &process_name) {
        log::warn!(
            "hyos specialize ignored pkg={} proc={}",
            package_name,
            process_name
        );
        return;
    }
    log::info!("hyos specialize pkg={} proc={}", package_name, process_name);
    crate::hook::hyos::install_for_hyos(&package_name, &process_name);
}

fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: API 保证字符串以 NUL 结尾。
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

fn valid_hyos_identity(package_name: &str, process_name: &str) -> bool {
    valid_package_name(package_name) && (process_name.is_empty() || valid_component(process_name))
}

/// 包名不允许 `:`；进程名允许 `:remote` 等后缀，因此两者分开校验。
fn valid_package_name(value: &str) -> bool {
    valid_component(value) && !value.contains(':')
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

#[unsafe(no_mangle)]
#[used]
pub static zn_module: ZygiskNextModule = ZygiskNextModule {
    target_api_version: ZYGISK_NEXT_API_VERSION,
    on_module_loaded: Some(on_module_loaded),
};

/// `registerModule` 会复制该结构；静态存储避免在 post-fork 回调里再分配。
static HYOS_MODULE: ZygiskNextHyosModule = ZygiskNextHyosModule {
    target_api_version: ZYGISK_NEXT_HYOS_API_VERSION,
    on_app_specialized: Some(on_hyos_app_specialized),
};

/// 当前进程是否已经成功注册 Hyos 模块；用于诊断/后续按 runtime 分支。
pub fn available() -> bool {
    !ZN_API.load(Ordering::Acquire).is_null()
}

pub fn hyos_registered() -> bool {
    HYOS_REGISTERED.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    macro_rules! test_require {
        ($result:expr, $context:literal) => {
            match $result {
                Ok(value) => value,
                Err(error) => panic!("{}: {:?}", $context, error),
            }
        };
    }

    static REGISTER_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn mock_register(module: *const c_void) -> i32 {
        assert!(!module.is_null());
        // SAFETY: mock 只由 register_hyos_runtime 用 HYOS_MODULE 调用。
        let module = unsafe { &*(module as *const ZygiskNextHyosModule) };
        assert_eq!(module.target_api_version, ZYGISK_NEXT_HYOS_API_VERSION);
        REGISTER_CALLS.fetch_add(1, Ordering::AcqRel);
        0
    }

    static VALID_RUNTIME: ZygiskNextRuntime = ZygiskNextRuntime {
        runtime_type: ZN_RUNTIME_HYOS,
        api_version: ZYGISK_NEXT_HYOS_API_VERSION,
        register_module: Some(mock_register),
    };
    static WRONG_TYPE_RUNTIME: ZygiskNextRuntime = ZygiskNextRuntime {
        runtime_type: 2,
        api_version: ZYGISK_NEXT_HYOS_API_VERSION,
        register_module: Some(mock_register),
    };
    static OLD_VERSION_RUNTIME: ZygiskNextRuntime = ZygiskNextRuntime {
        runtime_type: ZN_RUNTIME_HYOS,
        api_version: 0,
        register_module: Some(mock_register),
    };

    unsafe extern "C" fn get_valid_runtime() -> *const ZygiskNextRuntime {
        &VALID_RUNTIME
    }
    unsafe extern "C" fn get_wrong_type_runtime() -> *const ZygiskNextRuntime {
        &WRONG_TYPE_RUNTIME
    }
    unsafe extern "C" fn get_old_version_runtime() -> *const ZygiskNextRuntime {
        &OLD_VERSION_RUNTIME
    }
    unsafe extern "C" fn get_null_runtime() -> *const ZygiskNextRuntime {
        std::ptr::null()
    }

    fn api_with_runtime(
        get_runtime: unsafe extern "C" fn() -> *const ZygiskNextRuntime,
    ) -> ZygiskNextAPI {
        ZygiskNextAPI {
            plt_hook: std::ptr::null_mut(),
            inline_hook: std::ptr::null_mut(),
            inline_unhook: std::ptr::null_mut(),
            new_symbol_resolver: std::ptr::null_mut(),
            free_symbol_resolver: std::ptr::null_mut(),
            get_base_address: std::ptr::null_mut(),
            symbol_lookup: std::ptr::null_mut(),
            for_each_symbols: std::ptr::null_mut(),
            connect_companion: std::ptr::null_mut(),
            get_runtime: Some(get_runtime),
        }
    }

    #[test]
    fn hyos_identity_validation() {
        assert!(valid_hyos_identity("com.example.app", "com.example.app"));
        assert!(valid_hyos_identity(
            "com.example.app",
            "com.example.app:remote"
        ));
        assert!(valid_hyos_identity("com.example.app", ""));
        assert!(!valid_hyos_identity("", "com.example.app"));
        assert!(!valid_hyos_identity("com.example:bad", "com.example:bad"));
        assert!(!valid_hyos_identity("com.example.app", "bad process"));
        assert!(!valid_hyos_identity("x".repeat(256).as_str(), "x"));
        assert!(!valid_hyos_identity(
            "com.example.app",
            "x".repeat(256).as_str()
        ));
    }

    #[test]
    fn hyos_struct_layout_matches_api() {
        assert_eq!(std::mem::size_of::<ZygiskNextRuntime>(), 16);
        assert_eq!(std::mem::size_of::<ZygiskNextHyosModule>(), 16);
        assert_eq!(std::mem::size_of::<ZnHyosAppSpecializeArgs>(), 24);
        assert_eq!(std::mem::size_of::<ZygiskNextAPI>(), 80);
        assert_eq!(std::mem::size_of::<ZygiskNextModule>(), 16);
        assert_eq!(std::mem::offset_of!(ZygiskNextRuntime, register_module), 8);
        assert_eq!(std::mem::offset_of!(ZygiskNextAPI, get_runtime), 72);
        assert_eq!(
            std::mem::offset_of!(ZygiskNextHyosModule, on_app_specialized),
            8
        );
        assert_eq!(std::mem::offset_of!(ZygiskNextModule, on_module_loaded), 8);
    }

    #[test]
    fn hyos_registration_accepts_only_matching_runtime() {
        REGISTER_CALLS.store(0, Ordering::Release);
        assert!(register_hyos_runtime(&api_with_runtime(get_valid_runtime)));
        assert_eq!(REGISTER_CALLS.load(Ordering::Acquire), 1);
        assert!(hyos_registered());

        assert!(!register_hyos_runtime(&api_with_runtime(
            get_wrong_type_runtime
        )));
        assert!(!register_hyos_runtime(&api_with_runtime(
            get_old_version_runtime
        )));
        assert!(!register_hyos_runtime(&api_with_runtime(get_null_runtime)));
    }

    #[test]
    fn null_specialize_args_is_ignored() {
        // 防御性检查：API 不应传 null，但即使发生也不能解引用。
        // SAFETY: null 指针由回调入口显式拒绝，不会解引用。
        unsafe { on_hyos_app_specialized(std::ptr::null()) };
    }

    #[test]
    fn missing_get_runtime_is_rejected() {
        let api = ZygiskNextAPI {
            plt_hook: std::ptr::null_mut(),
            inline_hook: std::ptr::null_mut(),
            inline_unhook: std::ptr::null_mut(),
            new_symbol_resolver: std::ptr::null_mut(),
            free_symbol_resolver: std::ptr::null_mut(),
            get_base_address: std::ptr::null_mut(),
            symbol_lookup: std::ptr::null_mut(),
            for_each_symbols: std::ptr::null_mut(),
            connect_companion: std::ptr::null_mut(),
            get_runtime: None,
        };
        assert!(!register_hyos_runtime(&api));
    }

    #[test]
    fn cstr_to_string_handles_null_and_text() {
        assert!(cstr_to_string(std::ptr::null()).is_empty());
        let text = test_require!(std::ffi::CString::new("com.example.app"), "cstring");
        assert_eq!(cstr_to_string(text.as_ptr()), "com.example.app");
    }
}
