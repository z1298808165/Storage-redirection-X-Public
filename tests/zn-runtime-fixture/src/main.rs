#[allow(dead_code)]
#[path = "../../../src/zn_api/abi.rs"]
mod abi;

use abi::*;
use std::ffi::{CString, c_void};
use std::sync::atomic::{AtomicI32, AtomicPtr, Ordering};

static CALLBACK: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static CLIENT_FD: AtomicI32 = AtomicI32::new(-1);
const SELF_HANDLE: *mut c_void = std::ptr::dangling_mut::<c_void>();

unsafe extern "C" fn register(module: *const c_void) -> i32 {
    let module = unsafe { &*module.cast::<ZygiskNextHyosModule>() };
    if module.target_api_version != ZYGISK_NEXT_HYOS_API_VERSION {
        return 1;
    }
    CALLBACK.store(
        module
            .on_app_specialized
            .map_or(std::ptr::null_mut(), |f| f as *mut c_void),
        Ordering::Release,
    );
    0
}

unsafe extern "C" fn runtime() -> *const ZygiskNextRuntime {
    static RUNTIME: ZygiskNextRuntime = ZygiskNextRuntime {
        runtime_type: ZN_RUNTIME_HYOS,
        api_version: ZYGISK_NEXT_HYOS_API_VERSION,
        register_module: Some(register),
    };
    &RUNTIME
}

unsafe extern "C" fn connect(handle: *mut c_void) -> i32 {
    if handle != SELF_HANDLE {
        return -1;
    }
    unsafe { libc::fcntl(CLIENT_FD.load(Ordering::Relaxed), libc::F_DUPFD_CLOEXEC, 0) }
}

fn wait(child: i32) -> i32 {
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else {
        128 + libc::WTERMSIG(status)
    }
}

fn main() {
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) },
        0
    );
    let args = std::env::args().collect::<Vec<_>>();
    assert!(
        args.len() >= 5,
        "usage: zn-runtime-fixture <module.so> <package> <uid> <private|shared|disconnected|disabled> [write-path]"
    );
    let library = CString::new(args[1].as_str()).unwrap();
    let package = CString::new(args[2].as_str()).unwrap();
    let process = CString::new(format!("{}:zn_fixture", args[2])).unwrap();
    let uid = args[3].parse::<u32>().unwrap();
    let mode = args[4].as_str();
    assert!(matches!(
        mode,
        "private" | "shared" | "disconnected" | "disabled"
    ));
    let private = matches!(mode, "private" | "disconnected");
    let delay_ms = std::env::var("SRX_ZN_FIXTURE_DELAY_MS")
        .ok()
        .map(|value| value.parse::<u32>().unwrap())
        .unwrap_or(0);
    assert!(delay_ms <= 30_000);
    let path = args.get(5).map(|s| CString::new(s.as_str()).unwrap());
    let module = unsafe { libc::dlopen(library.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    assert!(!module.is_null(), "dlopen failed: {:?}", unsafe {
        std::ffi::CStr::from_ptr(libc::dlerror())
    });
    let entry = unsafe {
        libc::dlsym(module, c"zn_module".as_ptr())
            .cast::<ZygiskNextModule>()
            .as_ref()
    }
    .expect("zn_module export");
    let companion = unsafe {
        libc::dlsym(module, c"zn_companion_module".as_ptr())
            .cast::<ZygiskNextCompanionModule>()
            .as_ref()
    }
    .expect("zn_companion_module export");
    assert_eq!(entry.target_api_version, ZYGISK_NEXT_API_VERSION);
    let api = ZygiskNextAPI {
        plt_hook: std::ptr::null_mut(),
        inline_hook: std::ptr::null_mut(),
        inline_unhook: std::ptr::null_mut(),
        new_symbol_resolver: std::ptr::null_mut(),
        free_symbol_resolver: std::ptr::null_mut(),
        get_base_address: std::ptr::null_mut(),
        symbol_lookup: std::ptr::null_mut(),
        for_each_symbols: std::ptr::null_mut(),
        connect_companion: Some(connect),
        get_runtime: Some(runtime),
    };
    unsafe { entry.on_module_loaded.unwrap()(SELF_HANDLE, &api) };
    let callback = CALLBACK.load(Ordering::Acquire);
    assert!(!callback.is_null(), "Hyos callback registration");
    const DISABLED: &str = "/data/adb/modules/storage.redirect.x/.runtime_disabled";
    if mode == "disabled" {
        std::fs::write(DISABLED, b"fixture").unwrap();
    }
    let mut pair = [-1; 2];
    assert_eq!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                0,
                pair.as_mut_ptr(),
            )
        },
        0
    );
    CLIENT_FD.store(
        if mode == "disconnected" { -1 } else { pair[0] },
        Ordering::Release,
    );
    let server = unsafe { libc::fork() };
    assert!(server >= 0);
    if server == 0 {
        unsafe {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            libc::close(pair[0]);
            companion.on_module_connected.unwrap()(pair[1]);
            libc::_exit(0);
        }
    }
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        // 特化夹具在降权前建独立命名空间；回调本身只运行真实模块代码。
        unsafe {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            libc::close(pair[1]);
            if private && libc::unshare(libc::CLONE_NEWNS) != 0 {
                libc::_exit(120);
            }
            let storage_groups: [libc::gid_t; 3] = [1015, 1023, 9997];
            if libc::setgroups(storage_groups.len(), storage_groups.as_ptr()) != 0
                || libc::setgid(uid) != 0
                || libc::setuid(uid) != 0
            {
                libc::_exit(121);
            }
            let callback: unsafe extern "C" fn(*const ZnHyosAppSpecializeArgs) =
                std::mem::transmute(callback);
            callback(&ZnHyosAppSpecializeArgs {
                process_name: process.as_ptr(),
                package_name: package.as_ptr(),
                se_info: c"fixture".as_ptr(),
            });
            if delay_ms > 0 {
                libc::write(
                    libc::STDOUT_FILENO,
                    b"ZN_FIXTURE_READY\n".as_ptr().cast(),
                    17,
                );
                libc::usleep(delay_ms * 1000);
            }
            if let Some(path) = path.as_ref() {
                let fd = libc::open(
                    path.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
                    0o600,
                );
                if fd < 0 {
                    libc::perror(c"zn fixture open".as_ptr());
                    libc::_exit(122);
                }
                let written = libc::write(fd, b"srx-zn-fixture\n".as_ptr().cast(), 15);
                libc::close(fd);
                if written != 15 {
                    libc::_exit(123);
                }
            }
            libc::_exit(0);
        }
    }
    unsafe {
        libc::close(pair[0]);
        libc::close(pair[1]);
    }
    let child_status = wait(child);
    let server_status = wait(server);
    if mode == "disabled" {
        std::fs::remove_file(DISABLED).unwrap();
    }
    println!("ZN_FIXTURE_RESULT child={child_status} companion={server_status}");
    std::process::exit(child_status.max(server_status));
}
