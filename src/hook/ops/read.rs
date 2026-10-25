use super::super::context;
use super::super::fuse_fix;
use super::super::runtime;
use libc::{c_int, c_void, fstat, readlink};
use std::ffi::CString;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};

static CACHED_FUSE_FD: AtomicI32 = AtomicI32::new(-1);
static CACHED_FUSE_FD_DEV: AtomicU64 = AtomicU64::new(0);
static CACHED_FUSE_FD_INO: AtomicU64 = AtomicU64::new(0);
static FUSE_CACHE_INVALIDATE_COUNT: AtomicU64 = AtomicU64::new(0);
const FUSE_CACHE_LOG_SAMPLE_STEP: u64 = 128;

/// FUSE 请求读取方的候选库（按设备形态命中其一或全部）：
/// - MIUI 等分离形态：引擎在 DT_NEEDED 的 `libfuse.so`（fuse_session_loop），
///   libfuse_jni 的 read 仅用于 ReadFdToString 工具函数；
/// - AOSP 原生形态（真机/CI 模拟器）：fuse 引擎静态链接进 libfuse_jni，
///   请求循环的 read/__read_chk 就在其 PLT 里，不存在独立 libfuse.so；
/// - A13/A14 模拟器（sdk_gphone64 镜像）：MediaProvider 用旧版 FUSE 桥
///   `libappfuse.so`（/system/lib64 系统库，FuseAppfuse 直接 read /dev/fuse），
///   不加载 libfuse_jni——2026-10-25 本地模拟器实证，此形态此前无任何归因通道。
///
/// 前两库由 MediaProvider APK 内嵌（extractNativeLibs=false），maps 路径是
/// APK 本体，按「文件名后缀」匹配不到已加载段，必须走 ElfImg 的 APK 内嵌
/// ELF 通道定位；libappfuse 是普通系统库，走 maps 路径通道。
const LIB_FUSE_ENGINE: &str = "libfuse.so";
const LIB_FUSE_JNI: &str = "libfuse_jni.so";
const LIB_FUSE_APP: &str = "libappfuse.so";
static FUSE_ENGINE_READ_PATCHED: AtomicBool = AtomicBool::new(false);
static FUSE_JNI_READ_PATCHED: AtomicBool = AtomicBool::new(false);
static FUSE_APP_READ_PATCHED: AtomicBool = AtomicBool::new(false);
static FUSE_ENGINE_PATCH_CHECK_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);
const FUSE_ENGINE_PATCH_CHECK_INTERVAL_MS: i64 = 500;

/// 把 read 补丁装到 FUSE 请求读取方（libfuse.so / libfuse_jni.so / libappfuse.so
/// 都尝试）。
///
/// 两库均晚于 hook 安装按需加载，install 时的符号注册覆盖不到。沿用 fuse_fix
/// 的补装方式：ElfImg 定位（APK 内嵌 ELF 通道）后直接改写 read 与 __read_chk
/// 的 PLT 槽，包装函数直调 libc::read 并按严格校验解析 FUSE 协议头，**完全不
/// 进入 srx_hook 的 call_prev/with_prev_func 体系**——同一 proxy 地址被
/// hook_partial 与 hook_single 双重注册会破坏 prev 查找链，把 MediaProvider
/// 的全部 read 调用打崩（2026-10-24 真机事故：MP 崩溃循环，已回滚）。
/// libfuse_jni 的 read 也会被 ReadFdToString 等普通文件读取使用，因此解析
/// 前必须通过严格 FUSE 头校验（见 parse_fuse_header_after_read），非请求
/// 读取不会污染调用方信号。
pub(crate) fn ensure_fuse_engine_read_patch() {
    if FUSE_ENGINE_READ_PATCHED.load(Ordering::Relaxed)
        && FUSE_JNI_READ_PATCHED.load(Ordering::Relaxed)
        && FUSE_APP_READ_PATCHED.load(Ordering::Relaxed)
    {
        return;
    }
    let now_ms = crate::platform::paths::monotonic_ms();
    let last_ms = FUSE_ENGINE_PATCH_CHECK_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last_ms) < FUSE_ENGINE_PATCH_CHECK_INTERVAL_MS {
        return;
    }
    if FUSE_ENGINE_PATCH_CHECK_MS
        .compare_exchange(last_ms, now_ms, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    for (lib, patched_flag) in [
        (LIB_FUSE_ENGINE, &FUSE_ENGINE_READ_PATCHED),
        (LIB_FUSE_JNI, &FUSE_JNI_READ_PATCHED),
        (LIB_FUSE_APP, &FUSE_APP_READ_PATCHED),
    ] {
        if patched_flag.load(Ordering::Relaxed) {
            continue;
        }
        let Some(elf) = crate::platform::elf_img::ElfImg::load(lib) else {
            // 运维诊断：引擎库定位失败是静默重试路径（500ms 周期），补丁长期
            // 不安装时需要区分「库定位失败」与「PLT 槽位为零」（2026-10-25
            // CI 实证 A13/A14 的 read patch 从未安装且无任何日志）。
            static LAST_LOAD_FAIL_LOG_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);
            let now_ms = crate::platform::paths::monotonic_ms();
            let last_ms = LAST_LOAD_FAIL_LOG_MS.load(Ordering::Relaxed);
            if now_ms.saturating_sub(last_ms) >= 5000 {
                LAST_LOAD_FAIL_LOG_MS.store(now_ms, Ordering::Relaxed);
                log::warn!("fuse engine patch load failed lib={}", lib);
            }
            continue;
        };
        // SAFETY: patch_plt_slots 只改写目标库的 GOT 槽位并把槽页恢复只读；
        // 失败时保持原槽位不动。
        let read_slots = fuse_fix::patch_plt_slots(&elf, "read", hooked_read_fuse as *mut c_void);
        let read_chk_slots =
            fuse_fix::patch_plt_slots(&elf, "__read_chk", hooked_read_chk_fuse as *mut c_void);
        if read_slots > 0 || read_chk_slots > 0 {
            patched_flag.store(true, Ordering::Release);
            log::info!(
                "fuse read patch installed lib={} read={} read_chk={}",
                lib,
                read_slots,
                read_chk_slots
            );
        } else {
            // 运维诊断：库定位成功但 PLT 无可改槽位（A13/A14 实证的静默分支，
            // 引擎可能静态链接 read 或符号不走 PLT），长期重试需要可见。
            static LAST_ZERO_SLOT_LOG_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);
            let now_ms = crate::platform::paths::monotonic_ms();
            let last_ms = LAST_ZERO_SLOT_LOG_MS.load(Ordering::Relaxed);
            if now_ms.saturating_sub(last_ms) >= 5000 {
                LAST_ZERO_SLOT_LOG_MS.store(now_ms, Ordering::Relaxed);
                log::warn!(
                    "fuse engine patch slots zero lib={} image_len={}",
                    lib,
                    elf.image_len()
                );
            }
        }
    }
}

/// libfuse.so 的 read 包装：直调原函数并解析 FUSE 请求头。
///
/// 只读 buf 的协议头字段并更新调用方信号（thread-local 与系统 uid 接力槽），
/// 不改写请求内容、不影响返回值；buf 有效性与长度由 libc::read 的返回值
/// 保证（仅在成功读到至少一个协议头长度时解析）。
unsafe extern "C" fn hooked_read_fuse(fd: c_int, buf: *mut c_void, count: usize) -> isize {
    // SAFETY: 本模块 so 自身的 read 引用不在 hook 范围（install 时 add_ignore
    // 了 SELF_MODULE_PATH），libc::read 直达原实现。
    let result = libc::read(fd, buf, count);
    parse_fuse_header_after_read(fd, buf, result);
    result
}

/// libfuse.so 的 __read_chk 包装（FORTIFY 加固版，多一个 buf_size 参数）。
///
/// 直调 libc::read 跳过 abort 检查：buf_size 检查是编译期加固，调用方传入的
/// count 不会超过自身缓冲区，行为与原实现等价。
unsafe extern "C" fn hooked_read_chk_fuse(
    fd: c_int,
    buf: *mut c_void,
    count: usize,
    _buf_size: usize,
) -> isize {
    // SAFETY: 同 hooked_read_fuse，libc::read 直达原实现。
    let result = libc::read(fd, buf, count);
    parse_fuse_header_after_read(fd, buf, result);
    result
}

/// FUSE 操作码上限：内核 fuse_common.h 的合法操作码都在此范围内。
const FUSE_OPCODE_MAX: u32 = 45;

unsafe fn parse_fuse_header_after_read(_fd: c_int, buf: *mut c_void, result: isize) {
    if result < std::mem::size_of::<FuseInHeader>() as isize || buf.is_null() {
        return;
    }
    // SAFETY: read 成功返回至少 FuseInHeader 大小的字节，buf 指向可读缓冲。
    let header = std::ptr::read_unaligned(buf as *const FuseInHeader);
    // 严格 FUSE 请求头校验：libfuse_jni 的 read 也会被普通文件读取
    // （ReadFdToString 等）使用，误读会把文件内容当请求头污染调用方信号。
    // 内核 FUSE 的单请求读取恰好返回 header.len 字节，加上 unique 非零与
    // 操作码范围约束，非请求读取同时通过三项校验的概率可忽略。
    if header.len as isize != result || header.unique == 0 || header.opcode > FUSE_OPCODE_MAX {
        return;
    }
    context::set_fuse_caller_uid(header.uid as i32);
    if header.pid > 0 && header.pid <= i32::MAX as u32 {
        context::set_fuse_caller_pid(header.pid as i32);
        if (header.uid as i32) < crate::redirect::writer::ANDROID_APP_UID_START {
            context::set_system_fuse_request(header.uid as i32, header.pid as i32);
        }
    }
}

#[repr(C)]
struct FuseInHeader {
    len: u32,
    opcode: u32,
    unique: u64,
    nodeid: u64,
    uid: u32,
    gid: u32,
    pid: u32,
}

// 借用 FUSE 协议头提取真实调用方 UID/PID
pub unsafe extern "C" fn hooked_read(fd: c_int, buf: *mut c_void, count: usize) -> isize {
    let self_ptr = hooked_read as *mut c_void;
    let result = runtime::call_prev_lazy(
        self_ptr,
        || libc::read(fd, buf, count),
        |prev| {
            let f: unsafe extern "C" fn(c_int, *mut c_void, usize) -> isize =
                std::mem::transmute(prev);
            f(fd, buf, count)
        },
    );

    if result < std::mem::size_of::<FuseInHeader>() as isize || buf.is_null() {
        return result;
    }

    let cached_fd = CACHED_FUSE_FD.load(Ordering::Relaxed);
    if cached_fd == -2 {
        return result;
    }

    if cached_fd == -1 {
        if !is_fuse_fd(fd) || !cache_fuse_fd(fd) {
            return result;
        }
    } else if fd == cached_fd && !is_cached_fuse_fd_match(fd) {
        invalidate_fuse_cache(fd, "fd_identity_changed");
        return result;
    }

    if fd != CACHED_FUSE_FD.load(Ordering::Relaxed) {
        return result;
    }

    crate::hook::refresh_runtime_config_throttled();
    fuse_fix::retry_if_target_enabled();

    let header = unsafe { std::ptr::read_unaligned(buf as *const FuseInHeader) };
    if !is_valid_fuse_header(&header, result as usize) {
        invalidate_fuse_cache(fd, "invalid_header");
        return result;
    }

    context::set_fuse_caller_uid(header.uid as i32);
    if header.pid > 0 && header.pid <= i32::MAX as u32 {
        context::set_fuse_caller_pid(header.pid as i32);
        // 系统 uid 的请求头同步写入跨线程接力槽：MediaProvider 读线程与请求
        // 处理线程可能分离，thread-local 信号传不过去（见 context.rs 槽注释）。
        if (header.uid as i32) < crate::redirect::writer::ANDROID_APP_UID_START {
            context::set_system_fuse_request(header.uid as i32, header.pid as i32);
        }
    }

    result
}

fn cache_fuse_fd(fd: c_int) -> bool {
    let Some((dev, ino)) = get_fd_identity(fd) else {
        return false;
    };

    CACHED_FUSE_FD.store(fd, Ordering::Relaxed);
    CACHED_FUSE_FD_DEV.store(dev, Ordering::Relaxed);
    CACHED_FUSE_FD_INO.store(ino, Ordering::Relaxed);
    log::info!("fuse fd lazy detected fd={} dev={} ino={}", fd, dev, ino);
    true
}

fn invalidate_fuse_cache(fd: c_int, reason: &str) {
    CACHED_FUSE_FD.store(-1, Ordering::Relaxed);
    CACHED_FUSE_FD_DEV.store(0, Ordering::Relaxed);
    CACHED_FUSE_FD_INO.store(0, Ordering::Relaxed);
    context::clear_fuse_caller_uid();

    let count = FUSE_CACHE_INVALIDATE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if should_log_sample(count) {
        log::warn!(
            "fuse cache invalidated reason={} fd={} n={}",
            reason,
            fd,
            count
        );
    }
}

fn is_cached_fuse_fd_match(fd: c_int) -> bool {
    let expected_dev = CACHED_FUSE_FD_DEV.load(Ordering::Relaxed);
    let expected_ino = CACHED_FUSE_FD_INO.load(Ordering::Relaxed);
    if expected_dev == 0 || expected_ino == 0 {
        return false;
    }

    let Some((current_dev, current_ino)) = get_fd_identity(fd) else {
        return false;
    };

    current_dev == expected_dev && current_ino == expected_ino
}

fn is_valid_fuse_header(header: &FuseInHeader, read_len: usize) -> bool {
    let min_len = std::mem::size_of::<FuseInHeader>() as u32;
    if header.len < min_len || header.len as usize > read_len {
        return false;
    }
    if header.opcode == 0 || header.unique == 0 {
        return false;
    }
    header.uid <= i32::MAX as u32
}

fn is_fuse_fd(fd: c_int) -> bool {
    let link_path = format!("/proc/self/fd/{}", fd);
    let Ok(c_path) = CString::new(link_path) else {
        return false;
    };

    let mut link_buf = [0u8; 96];
    let len = unsafe {
        readlink(
            c_path.as_ptr(),
            link_buf.as_mut_ptr() as *mut _,
            link_buf.len() - 1,
        )
    };
    if len <= 0 {
        return false;
    }
    link_buf[len as usize] = 0;
    let text = String::from_utf8_lossy(&link_buf[..len as usize]);
    text == "/dev/fuse"
}

fn get_fd_identity(fd: c_int) -> Option<(u64, u64)> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    let ret = unsafe { fstat(fd, stat.as_mut_ptr()) };
    if ret != 0 {
        return None;
    }

    let stat = unsafe { stat.assume_init() };
    Some((stat.st_dev, stat.st_ino))
}

#[inline]
fn should_log_sample(count: u64) -> bool {
    count == 1 || count.is_multiple_of(FUSE_CACHE_LOG_SAMPLE_STEP)
}
