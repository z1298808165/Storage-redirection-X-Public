use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

mod abi;
use abi::*;

/// ZN API 表由加载器保持到进程退出；handle 供子进程连接本模块的 companion。
static ZN_API: AtomicPtr<ZygiskNextAPI> = AtomicPtr::new(std::ptr::null_mut());
static SELF_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static HYOS_REGISTERED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_module_loaded(handle: *mut c_void, api: *const ZygiskNextAPI) {
    if api.is_null() {
        return;
    }
    crate::logging::Logger::init(Some("hyos_spawner"));
    if !crate::runtime_control::is_module_runtime_enabled() {
        return;
    }
    if crate::platform::android_api_level() < crate::platform::MIN_SUPPORTED_API_LEVEL {
        return;
    }
    SELF_HANDLE.store(handle, Ordering::Relaxed);
    ZN_API.store(api.cast_mut(), Ordering::Release);
    // SAFETY: 非空 API 表由 ZN 保持到进程退出。
    let api_ref = unsafe { &*api };
    log::info!("zygisk next api loaded");
    let _ = register_hyos_runtime(api_ref);
}

/// 查询当前进程是否返回了 Hyos runtime；只有 `getRuntime` 存在且类型/版本
/// 匹配时才注册。返回 false 不表示 Zygisk Next 不可用，只表示不是 Hyos。
fn register_hyos_runtime(api: &ZygiskNextAPI) -> bool {
    HYOS_REGISTERED.store(false, Ordering::Release);
    if api.connect_companion.is_none() {
        return false;
    }
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
    if !hyos_registered() {
        return;
    }
    // SAFETY: 字符串由 runtime 保持到回调结束，仅借用且限制扫描长度。
    let (Some(process_name), Some(package_name)) =
        (unsafe { cstr_to_str(args.process_name) }, unsafe {
            cstr_to_str(args.package_name)
        })
    else {
        return;
    };
    let Some(payload) = crate::lifecycle::hyos_request::encode(package_name, process_name) else {
        return;
    };
    let api = ZN_API.load(Ordering::Acquire);
    if api.is_null() {
        return;
    }
    // SAFETY: API 表在进程生命周期内有效，connectCompanion 可用于 Hyos 回调。
    let Some(connect) = (unsafe { &*api }).connect_companion else {
        return;
    };
    // SAFETY: handle 由加载器传入并在注册回调前保存，connect 来自有效 API 表。
    let fd = unsafe { connect(SELF_HANDLE.load(Ordering::Relaxed)) };
    if fd < 0 || !crate::lifecycle::specialize_hyos(fd, &payload) {
        // 不能把传输失败或未建立的视图当作直通许可，也不能在 fork 子进程走日志锁。
        let message = b"SRX: Hyos storage view setup failed\n";
        // SAFETY: 固定缓冲在 write 期间有效；_exit 终止当前特化子进程且不执行析构。
        unsafe {
            libc::write(libc::STDERR_FILENO, message.as_ptr().cast(), message.len());
            libc::_exit(126);
        }
    }
}

unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: runtime 保证 NUL 结尾；过长输入只扫描固定上限，不复制字符串。
    let length = unsafe { libc::strnlen(ptr, crate::lifecycle::hyos_request::IDENTITY_CAPACITY) };
    if length >= crate::lifecycle::hyos_request::IDENTITY_CAPACITY {
        return None;
    }
    // SAFETY: strnlen 已找到上限内的 NUL，切片只覆盖该字符串及终止符。
    unsafe {
        CStr::from_bytes_with_nul_unchecked(std::slice::from_raw_parts(ptr.cast(), length + 1))
    }
    .to_str()
    .ok()
}

#[unsafe(no_mangle)]
#[used]
pub static zn_module: ZygiskNextModule = ZygiskNextModule {
    target_api_version: ZYGISK_NEXT_API_VERSION,
    on_module_loaded: Some(on_module_loaded),
};

unsafe extern "C" fn on_companion_connected(fd: i32) {
    crate::lifecycle::run_companion_pipeline(fd);
    // SAFETY: ZN 将连接 FD 的所有权交给回调，每条返回路径均在此关闭。
    unsafe { libc::close(fd) };
}

#[unsafe(no_mangle)]
#[used]
pub static zn_companion_module: ZygiskNextCompanionModule = ZygiskNextCompanionModule {
    target_api_version: ZYGISK_NEXT_API_VERSION,
    on_companion_loaded: None,
    on_module_connected: Some(on_companion_connected),
};

/// `registerModule` 会复制该结构；静态存储避免在 post-fork 回调里再分配。
static HYOS_MODULE: ZygiskNextHyosModule = ZygiskNextHyosModule {
    target_api_version: ZYGISK_NEXT_HYOS_API_VERSION,
    on_app_specialized: Some(on_hyos_app_specialized),
};

/// 当前进程是否收到 ZN API 表；不代表 Hyos 注册或存储视图已经就绪。
pub fn available() -> bool {
    !ZN_API.load(Ordering::Acquire).is_null()
}

pub fn hyos_registered() -> bool {
    HYOS_REGISTERED.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::hyos_request::valid_identity;
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

    unsafe extern "C" fn mock_connect(_: *mut c_void) -> i32 {
        -1
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
            connect_companion: Some(mock_connect),
            get_runtime: Some(get_runtime),
        }
    }

    #[test]
    fn hyos_identity_validation() {
        assert!(valid_identity("com.example.app", "com.example.app"));
        assert!(valid_identity("com.example.app", "com.example.app:remote"));
        assert!(valid_identity("com.example.app", ""));
        assert!(!valid_identity("", "com.example.app"));
        assert!(!valid_identity("com.example:bad", "com.example:bad"));
        assert!(!valid_identity("com.example.app", "bad process"));
        assert!(!valid_identity("x".repeat(256).as_str(), "x"));
        assert!(!valid_identity("com.example.app", "x".repeat(256).as_str()));
    }

    #[test]
    fn hyos_struct_layout_matches_api() {
        assert_eq!(std::mem::size_of::<ZygiskNextRuntime>(), 16);
        assert_eq!(std::mem::size_of::<ZygiskNextHyosModule>(), 16);
        assert_eq!(std::mem::size_of::<ZnHyosAppSpecializeArgs>(), 24);
        assert_eq!(std::mem::size_of::<ZygiskNextAPI>(), 80);
        assert_eq!(std::mem::size_of::<ZygiskNextModule>(), 16);
        assert_eq!(std::mem::size_of::<ZygiskNextCompanionModule>(), 24);
        assert_eq!(std::mem::offset_of!(ZygiskNextRuntime, register_module), 8);
        assert_eq!(std::mem::offset_of!(ZygiskNextAPI, get_runtime), 72);
        assert_eq!(
            std::mem::offset_of!(ZygiskNextHyosModule, on_app_specialized),
            8
        );
        assert_eq!(std::mem::offset_of!(ZygiskNextModule, on_module_loaded), 8);
        assert_eq!(
            std::mem::offset_of!(ZygiskNextCompanionModule, on_module_connected),
            16
        );
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
        assert!(!hyos_registered());
        assert_eq!(REGISTER_CALLS.load(Ordering::Acquire), 1);
        let mut no_companion = api_with_runtime(get_valid_runtime);
        no_companion.connect_companion = None;
        assert!(!register_hyos_runtime(&no_companion));
        assert_eq!(REGISTER_CALLS.load(Ordering::Acquire), 1);
        let mut no_runtime = api_with_runtime(get_valid_runtime);
        no_runtime.get_runtime = None;
        assert!(!register_hyos_runtime(&no_runtime));
        unsafe extern "C" fn missing_register() -> *const ZygiskNextRuntime {
            static RUNTIME: ZygiskNextRuntime = ZygiskNextRuntime {
                runtime_type: ZN_RUNTIME_HYOS,
                api_version: ZYGISK_NEXT_HYOS_API_VERSION,
                register_module: None,
            };
            &RUNTIME
        }
        unsafe extern "C" fn failed_register(_: *const c_void) -> i32 {
            -1
        }
        unsafe extern "C" fn rejected_registration() -> *const ZygiskNextRuntime {
            static RUNTIME: ZygiskNextRuntime = ZygiskNextRuntime {
                runtime_type: ZN_RUNTIME_HYOS,
                api_version: ZYGISK_NEXT_HYOS_API_VERSION,
                register_module: Some(failed_register),
            };
            &RUNTIME
        }
        assert!(!register_hyos_runtime(&api_with_runtime(missing_register)));
        assert!(!register_hyos_runtime(&api_with_runtime(
            rejected_registration
        )));
        assert!(!hyos_registered());
    }

    #[test]
    fn null_specialize_args_is_ignored() {
        // 防御性检查：API 不应传 null，但即使发生也不能解引用。
        // SAFETY: null 指针由回调入口显式拒绝，不会解引用。
        unsafe { on_hyos_app_specialized(std::ptr::null()) };
    }

    #[test]
    fn cstr_to_str_rejects_null_invalid_utf8_and_long_inputs() {
        // SAFETY: null 输入由函数拒绝，不解引用。
        assert!(unsafe { cstr_to_str(std::ptr::null()) }.is_none());
        let text = test_require!(std::ffi::CString::new("com.example.app"), "cstring");
        assert_eq!(
            // SAFETY: CString 在调用期间有效且包含 NUL。
            unsafe { cstr_to_str(text.as_ptr()) },
            Some("com.example.app")
        );
        let invalid = [0xffu8, 0];
        // SAFETY: 数组含 NUL，函数仅校验 UTF-8。
        assert!(unsafe { cstr_to_str(invalid.as_ptr().cast()) }.is_none());
        let long = [b'x'; 256];
        // SAFETY: 数组覆盖 strnlen 的整个扫描上限，无 NUL 时不再构造切片。
        assert!(unsafe { cstr_to_str(long.as_ptr().cast()) }.is_none());
    }
}
