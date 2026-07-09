// Hooker 类全局引用 + LSPlant native trampoline 绑定

use super::lsplant;
use crate::hook::{
    resolve_download_media_placeholder_path_for_caller, resolve_open_storage_path_for_caller,
    rewrite_cursor_storage_path_for_caller, rewrite_media_store_bucket_id_for_caller,
    rewrite_media_store_storage_path_for_caller, should_allow_public_mapping_target_access,
    should_hide_cursor_storage_path_for_caller,
};
use crate::zygisk::jni::{
    call_object_method_a, get_method_id, get_object_class, new_global_ref, new_jstring_utf8,
    register_natives,
};
use jni_sys::{_jobject, JNIEnv, JNINativeMethod, jclass, jobject, jobjectArray, jstring, jvalue};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

static HOOKER_CLASS_GLOBAL: AtomicPtr<_jobject> = AtomicPtr::new(std::ptr::null_mut());
static LSPLANT_INITIALIZED: AtomicBool = AtomicBool::new(false);
const HIDDEN_ROW_SENTINEL: &str = "\u{1F}SRX_HIDDEN_ROW";

const DO_HOOK_NAME: &[u8] = b"doHook\0";
const DO_HOOK_SIG: &[u8] =
    b"(Ljava/lang/reflect/Member;Ljava/lang/reflect/Method;)Ljava/lang/reflect/Method;\0";
const DO_UNHOOK_NAME: &[u8] = b"doUnhook\0";
const DO_UNHOOK_SIG: &[u8] = b"(Ljava/lang/reflect/Member;)Z\0";
const CALLBACK_NAME: &[u8] = b"onMediaProviderQuery\0";
const CALLBACK_SIG: &[u8] = b"(Lorg/srx/hook/Hooker;[Ljava/lang/Object;)Ljava/lang/Object;\0";
const FILTER_PATH_NAME: &[u8] = b"filterPath\0";
const FILTER_PATH_SIG: &[u8] = b"(Ljava/lang/String;I)Ljava/lang/String;\0";
const SHOULD_HIDE_CURSOR_PATH_NAME: &[u8] = b"shouldHideCursorPath\0";
const SHOULD_HIDE_CURSOR_PATH_SIG: &[u8] = b"(Ljava/lang/String;I)Z\0";
const RESOLVE_OPEN_PATH_NAME: &[u8] = b"resolveOpenPath\0";
const RESOLVE_OPEN_PATH_SIG: &[u8] = b"(Ljava/lang/String;I)Ljava/lang/String;\0";
const STORAGE_PATH_EXISTS_NAME: &[u8] = b"storagePathExistsBySyscall\0";
const STORAGE_PATH_EXISTS_SIG: &[u8] = b"(Ljava/lang/String;)Z\0";
const REWRITE_MEDIA_STORE_PATH_NAME: &[u8] = b"rewriteMediaStorePath\0";
const REWRITE_MEDIA_STORE_PATH_SIG: &[u8] = b"(Ljava/lang/String;I)Ljava/lang/String;\0";
const RESOLVE_DOWNLOAD_PLACEHOLDER_NAME: &[u8] = b"resolveDownloadPlaceholder\0";
const RESOLVE_DOWNLOAD_PLACEHOLDER_SIG: &[u8] =
    b"(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;ZI)Ljava/lang/String;\0";
const REWRITE_BUCKET_ID_NAME: &[u8] = b"rewriteMediaStoreBucketId\0";
const REWRITE_BUCKET_ID_SIG: &[u8] = b"(Ljava/lang/String;I)Ljava/lang/String;\0";
const RECORD_QUERY_ACCESS_PATH_NAME: &[u8] = b"recordQueryAccessPath\0";
const RECORD_QUERY_ACCESS_PATH_SIG: &[u8] = b"(Ljava/lang/String;I)V\0";
const RECORD_PROVIDER_OPEN_PATH_NAME: &[u8] = b"recordProviderOpenPath\0";
const RECORD_PROVIDER_OPEN_PATH_SIG: &[u8] = b"(Ljava/lang/String;ILjava/lang/String;)V\0";
const RECORD_READ_ONLY_FUSE_OPERATION_NAME: &[u8] = b"recordReadOnlyFuseOperation\0";
const RECORD_READ_ONLY_FUSE_OPERATION_SIG: &[u8] =
    b"(ILjava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;II)Z\0";
const ALLOW_PUBLIC_MAPPING_TARGET_NAME: &[u8] = b"allowsPublicMappingTarget\0";
const ALLOW_PUBLIC_MAPPING_TARGET_SIG: &[u8] = b"(Ljava/lang/String;I)Z\0";
const CAPTURE_BINDER_CALLER_NAME: &[u8] = b"captureBinderCaller\0";
const CAPTURE_BINDER_CALLER_SIG: &[u8] = b"(II)V\0";
const ENTER_CALLER_SCOPE_NAME: &[u8] = b"enterCallerScope\0";
const EXIT_CALLER_SCOPE_NAME: &[u8] = b"exitCallerScope\0";
const ENTER_CALLER_SCOPE_SIG: &[u8] = b"(II)V\0";
const EXIT_CALLER_SCOPE_SIG: &[u8] = b"()V\0";
const ENTER_PROVIDER_PASSTHROUGH_NAME: &[u8] = b"enterProviderPassthrough\0";
const EXIT_PROVIDER_PASSTHROUGH_NAME: &[u8] = b"exitProviderPassthrough\0";
const PROVIDER_PASSTHROUGH_SIG: &[u8] = b"()V\0";
const DEBUG_LOGGING_ENABLED_NAME: &[u8] = b"isDebugLoggingEnabled\0";
const DEBUG_LOGGING_ENABLED_SIG: &[u8] = b"()Z\0";

const IS_REDIRECT_ENABLED_NAME: &[u8] = b"isRedirectEnabledForCallerUid\0";
const IS_REDIRECT_ENABLED_SIG: &[u8] = b"(I)Z\0";

pub fn init(env: *mut JNIEnv, hooker_class: jclass) -> bool {
    if env.is_null() || hooker_class.is_null() {
        return false;
    }

    let methods = [
        JNINativeMethod {
            name: DO_HOOK_NAME.as_ptr() as *mut _,
            signature: DO_HOOK_SIG.as_ptr() as *mut _,
            fnPtr: do_hook as *mut _,
        },
        JNINativeMethod {
            name: DO_UNHOOK_NAME.as_ptr() as *mut _,
            signature: DO_UNHOOK_SIG.as_ptr() as *mut _,
            fnPtr: do_unhook as *mut _,
        },
        JNINativeMethod {
            name: CALLBACK_NAME.as_ptr() as *mut _,
            signature: CALLBACK_SIG.as_ptr() as *mut _,
            fnPtr: on_media_provider_query as *mut _,
        },
        JNINativeMethod {
            name: FILTER_PATH_NAME.as_ptr() as *mut _,
            signature: FILTER_PATH_SIG.as_ptr() as *mut _,
            fnPtr: filter_path as *mut _,
        },
        JNINativeMethod {
            name: SHOULD_HIDE_CURSOR_PATH_NAME.as_ptr() as *mut _,
            signature: SHOULD_HIDE_CURSOR_PATH_SIG.as_ptr() as *mut _,
            fnPtr: should_hide_cursor_path as *mut _,
        },
        JNINativeMethod {
            name: RESOLVE_OPEN_PATH_NAME.as_ptr() as *mut _,
            signature: RESOLVE_OPEN_PATH_SIG.as_ptr() as *mut _,
            fnPtr: resolve_open_path as *mut _,
        },
        JNINativeMethod {
            name: STORAGE_PATH_EXISTS_NAME.as_ptr() as *mut _,
            signature: STORAGE_PATH_EXISTS_SIG.as_ptr() as *mut _,
            fnPtr: storage_path_exists as *mut _,
        },
        JNINativeMethod {
            name: REWRITE_MEDIA_STORE_PATH_NAME.as_ptr() as *mut _,
            signature: REWRITE_MEDIA_STORE_PATH_SIG.as_ptr() as *mut _,
            fnPtr: rewrite_media_store_path as *mut _,
        },
        JNINativeMethod {
            name: RESOLVE_DOWNLOAD_PLACEHOLDER_NAME.as_ptr() as *mut _,
            signature: RESOLVE_DOWNLOAD_PLACEHOLDER_SIG.as_ptr() as *mut _,
            fnPtr: resolve_download_placeholder as *mut _,
        },
        JNINativeMethod {
            name: REWRITE_BUCKET_ID_NAME.as_ptr() as *mut _,
            signature: REWRITE_BUCKET_ID_SIG.as_ptr() as *mut _,
            fnPtr: rewrite_media_store_bucket_id as *mut _,
        },
        JNINativeMethod {
            name: RECORD_QUERY_ACCESS_PATH_NAME.as_ptr() as *mut _,
            signature: RECORD_QUERY_ACCESS_PATH_SIG.as_ptr() as *mut _,
            fnPtr: record_query_access_path as *mut _,
        },
        JNINativeMethod {
            name: RECORD_PROVIDER_OPEN_PATH_NAME.as_ptr() as *mut _,
            signature: RECORD_PROVIDER_OPEN_PATH_SIG.as_ptr() as *mut _,
            fnPtr: record_provider_open_path as *mut _,
        },
        JNINativeMethod {
            name: RECORD_READ_ONLY_FUSE_OPERATION_NAME.as_ptr() as *mut _,
            signature: RECORD_READ_ONLY_FUSE_OPERATION_SIG.as_ptr() as *mut _,
            fnPtr: record_read_only_fuse_operation as *mut _,
        },
        JNINativeMethod {
            name: ALLOW_PUBLIC_MAPPING_TARGET_NAME.as_ptr() as *mut _,
            signature: ALLOW_PUBLIC_MAPPING_TARGET_SIG.as_ptr() as *mut _,
            fnPtr: allows_public_mapping_target as *mut _,
        },
        JNINativeMethod {
            name: CAPTURE_BINDER_CALLER_NAME.as_ptr() as *mut _,
            signature: CAPTURE_BINDER_CALLER_SIG.as_ptr() as *mut _,
            fnPtr: capture_binder_caller as *mut _,
        },
        JNINativeMethod {
            name: ENTER_CALLER_SCOPE_NAME.as_ptr() as *mut _,
            signature: ENTER_CALLER_SCOPE_SIG.as_ptr() as *mut _,
            fnPtr: enter_caller_scope as *mut _,
        },
        JNINativeMethod {
            name: EXIT_CALLER_SCOPE_NAME.as_ptr() as *mut _,
            signature: EXIT_CALLER_SCOPE_SIG.as_ptr() as *mut _,
            fnPtr: exit_caller_scope as *mut _,
        },
        JNINativeMethod {
            name: ENTER_PROVIDER_PASSTHROUGH_NAME.as_ptr() as *mut _,
            signature: PROVIDER_PASSTHROUGH_SIG.as_ptr() as *mut _,
            fnPtr: enter_provider_passthrough as *mut _,
        },
        JNINativeMethod {
            name: EXIT_PROVIDER_PASSTHROUGH_NAME.as_ptr() as *mut _,
            signature: PROVIDER_PASSTHROUGH_SIG.as_ptr() as *mut _,
            fnPtr: exit_provider_passthrough as *mut _,
        },
        JNINativeMethod {
            name: DEBUG_LOGGING_ENABLED_NAME.as_ptr() as *mut _,
            signature: DEBUG_LOGGING_ENABLED_SIG.as_ptr() as *mut _,
            fnPtr: is_debug_logging_enabled as *mut _,
        },
        JNINativeMethod {
            name: IS_REDIRECT_ENABLED_NAME.as_ptr() as *mut _,
            signature: IS_REDIRECT_ENABLED_SIG.as_ptr() as *mut _,
            fnPtr: is_redirect_enabled_for_caller_uid as *mut _,
        },
    ];
    if !register_natives(env, hooker_class, &methods) {
        log::warn!("java hook register natives failed");
        return false;
    }
    log::info!("java hook natives registered");

    // 立即初始化 LSPlant（带重试机制）
    // 这样可以在 specialize_pre 阶段完成所有初始化，避免延迟初始化时的问题
    if !ensure_lsplant_initialized_with_retry(env) {
        log::error!("java hook lsplant init failed after retries");
        return false;
    }

    let gref = new_global_ref(env, hooker_class);
    if gref.is_null() {
        log::error!("java hook new_global_ref failed");
        return false;
    }
    HOOKER_CLASS_GLOBAL.store(gref, Ordering::Release);
    log::info!("java hook init complete");
    true
}

pub fn hooker_class() -> jclass {
    HOOKER_CLASS_GLOBAL.load(Ordering::Acquire)
}

unsafe extern "C" fn do_hook(
    env: *mut JNIEnv,
    thiz: jobject,
    target: jobject,
    callback: jobject,
) -> jobject {
    if !ensure_lsplant_initialized(env) {
        log::error!("[Hook] do_hook failed: LSPlant not initialized");
        return std::ptr::null_mut();
    }
    let result = lsplant::hook(env, target, thiz, callback);
    if result.is_null() {
        log::error!("[Hook] do_hook failed: lsplant::hook returned null");
    }
    result
}

unsafe extern "C" fn do_unhook(
    env: *mut JNIEnv,
    _thiz: jobject,
    target: jobject,
) -> jni_sys::jboolean {
    if !ensure_lsplant_initialized(env) {
        return jni_sys::JNI_FALSE;
    }
    if lsplant::unhook(env, target) {
        jni_sys::JNI_TRUE
    } else {
        jni_sys::JNI_FALSE
    }
}

// 带重试机制的 LSPlant 初始化，用于 init() 中的提前初始化
// 这可以处理瞬时失败（资源竞争、ART 运行时尚未完全就绪等）
fn ensure_lsplant_initialized_with_retry(env: *mut JNIEnv) -> bool {
    if LSPLANT_INITIALIZED.load(Ordering::Acquire) {
        return true;
    }
    if env.is_null() {
        return false;
    }

    const MAX_ATTEMPTS: usize = 3;
    const RETRY_DELAY_MS: u64 = 50;

    for attempt in 1..=MAX_ATTEMPTS {
        if lsplant::init(env) {
            LSPLANT_INITIALIZED.store(true, Ordering::Release);
            if attempt > 1 {
                log::info!(
                    "[LSPlant] init succeeded on attempt {}/{}",
                    attempt,
                    MAX_ATTEMPTS
                );
            }
            return true;
        }

        log::warn!("[LSPlant] init attempt {}/{} failed", attempt, MAX_ATTEMPTS);

        if attempt < MAX_ATTEMPTS {
            std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
            log::info!("[LSPlant] retrying after {}ms delay...", RETRY_DELAY_MS);
        }
    }

    log::error!("[LSPlant] init FAILED after {} attempts", MAX_ATTEMPTS);
    false
}

fn ensure_lsplant_initialized(env: *mut JNIEnv) -> bool {
    if LSPLANT_INITIALIZED.load(Ordering::Acquire) {
        return true;
    }
    // 不应该到这里，因为已经在 init() 中初始化了
    log::warn!(
        "[LSPlant] ensure_lsplant_initialized called but not initialized - this should not happen"
    );
    ensure_lsplant_initialized_with_retry(env)
}

unsafe extern "C" fn on_media_provider_query(
    env: *mut JNIEnv,
    _class: jclass,
    hooker: jobject,
    args: jobjectArray,
) -> jobject {
    call_backup(env, hooker, args)
}

unsafe extern "C" fn filter_path(
    env: *mut JNIEnv,
    _class: jclass,
    path: jstring,
    caller_uid: jni_sys::jint,
) -> jstring {
    if env.is_null() || path.is_null() {
        return std::ptr::null_mut();
    }
    let original = crate::zygisk::jni::get_jstring_utf8(env, path);
    let Some(rewritten) = rewrite_cursor_storage_path_for_caller(&original, caller_uid) else {
        return path;
    };
    if rewritten.is_empty() {
        return new_jstring_utf8(env, HIDDEN_ROW_SENTINEL);
    }
    new_jstring_utf8(env, &rewritten)
}

unsafe extern "C" fn should_hide_cursor_path(
    env: *mut JNIEnv,
    _class: jclass,
    path: jstring,
    caller_uid: jni_sys::jint,
) -> jni_sys::jboolean {
    if env.is_null() || path.is_null() {
        return jni_sys::JNI_FALSE;
    }
    let original = crate::zygisk::jni::get_jstring_utf8(env, path);
    if should_hide_cursor_storage_path_for_caller(&original, caller_uid) {
        jni_sys::JNI_TRUE
    } else {
        jni_sys::JNI_FALSE
    }
}

unsafe extern "C" fn resolve_open_path(
    env: *mut JNIEnv,
    _class: jclass,
    path: jstring,
    caller_uid: jni_sys::jint,
) -> jstring {
    if env.is_null() || path.is_null() {
        return std::ptr::null_mut();
    }
    let original = crate::zygisk::jni::get_jstring_utf8(env, path);
    let Some(rewritten) = resolve_open_storage_path_for_caller(&original, caller_uid) else {
        return std::ptr::null_mut();
    };
    if rewritten.is_empty() || rewritten == original {
        return std::ptr::null_mut();
    }
    new_jstring_utf8(env, &rewritten)
}

unsafe extern "C" fn storage_path_exists(
    env: *mut JNIEnv,
    _class: jclass,
    path: jstring,
) -> jni_sys::jboolean {
    if env.is_null() || path.is_null() {
        return jni_sys::JNI_FALSE;
    }
    let path_text = crate::zygisk::jni::get_jstring_utf8(env, path);
    if crate::hook::storage_path_exists_by_syscall(&path_text) {
        jni_sys::JNI_TRUE
    } else {
        jni_sys::JNI_FALSE
    }
}

unsafe extern "C" fn rewrite_media_store_path(
    env: *mut JNIEnv,
    _class: jclass,
    path: jstring,
    caller_uid: jni_sys::jint,
) -> jstring {
    if env.is_null() || path.is_null() {
        return std::ptr::null_mut();
    }
    let original = crate::zygisk::jni::get_jstring_utf8(env, path);
    let Some(rewritten) = rewrite_media_store_storage_path_for_caller(&original, caller_uid) else {
        return std::ptr::null_mut();
    };
    if rewritten.is_empty() || rewritten == original {
        return std::ptr::null_mut();
    }
    new_jstring_utf8(env, &rewritten)
}

unsafe extern "C" fn resolve_download_placeholder(
    env: *mut JNIEnv,
    _class: jclass,
    original_path: jstring,
    relative_path: jstring,
    display_name: jstring,
    video: jni_sys::jboolean,
    caller_uid: jni_sys::jint,
) -> jstring {
    if env.is_null() {
        return std::ptr::null_mut();
    }
    let original = if original_path.is_null() {
        String::new()
    } else {
        crate::zygisk::jni::get_jstring_utf8(env, original_path)
    };
    let relative = if relative_path.is_null() {
        String::new()
    } else {
        crate::zygisk::jni::get_jstring_utf8(env, relative_path)
    };
    let name = if display_name.is_null() {
        String::new()
    } else {
        crate::zygisk::jni::get_jstring_utf8(env, display_name)
    };
    let Some(mapped) = resolve_download_media_placeholder_path_for_caller(
        &original,
        &relative,
        &name,
        video == jni_sys::JNI_TRUE,
        caller_uid,
    ) else {
        return std::ptr::null_mut();
    };
    if mapped.is_empty() {
        return std::ptr::null_mut();
    }
    new_jstring_utf8(env, &mapped)
}

unsafe extern "C" fn rewrite_media_store_bucket_id(
    env: *mut JNIEnv,
    _class: jclass,
    bucket_id: jstring,
    caller_uid: jni_sys::jint,
) -> jstring {
    if env.is_null() || bucket_id.is_null() {
        return std::ptr::null_mut();
    }
    let original = crate::zygisk::jni::get_jstring_utf8(env, bucket_id);
    let Some(rewritten) = rewrite_media_store_bucket_id_for_caller(&original, caller_uid) else {
        return std::ptr::null_mut();
    };
    if rewritten.is_empty() || rewritten == original {
        return std::ptr::null_mut();
    }
    new_jstring_utf8(env, &rewritten)
}

unsafe extern "C" fn record_query_access_path(
    env: *mut JNIEnv,
    _class: jclass,
    path: jstring,
    caller_uid: jni_sys::jint,
) {
    if env.is_null() || path.is_null() {
        return;
    }
    let path_text = crate::zygisk::jni::get_jstring_utf8(env, path);
    crate::monitor::AuditTrail::instance().record_query_access_path(&path_text, caller_uid);
}

unsafe extern "C" fn record_provider_open_path(
    env: *mut JNIEnv,
    _class: jclass,
    path: jstring,
    caller_uid: jni_sys::jint,
    caller_package: jstring,
) {
    if env.is_null() || path.is_null() {
        return;
    }
    let path_text = crate::zygisk::jni::get_jstring_utf8(env, path);
    let package_text = if caller_package.is_null() {
        String::new()
    } else {
        crate::zygisk::jni::get_jstring_utf8(env, caller_package)
    };
    crate::monitor::AuditTrail::instance().record_provider_open_path(
        &path_text,
        caller_uid,
        &package_text,
    );
}

unsafe extern "C" fn record_read_only_fuse_operation(
    env: *mut JNIEnv,
    _class: jclass,
    kind: jni_sys::jint,
    op_name: jstring,
    op_filter: jstring,
    path: jstring,
    from_path: jstring,
    caller_uid: jni_sys::jint,
    flags: jni_sys::jint,
) -> jni_sys::jboolean {
    if env.is_null() || path.is_null() {
        return jni_sys::JNI_FALSE;
    }
    let op_name_text = if op_name.is_null() {
        String::new()
    } else {
        crate::zygisk::jni::get_jstring_utf8(env, op_name)
    };
    let op_filter_text = if op_filter.is_null() {
        String::new()
    } else {
        crate::zygisk::jni::get_jstring_utf8(env, op_filter)
    };
    let path_text = crate::zygisk::jni::get_jstring_utf8(env, path);
    let from_path_text = if from_path.is_null() {
        String::new()
    } else {
        crate::zygisk::jni::get_jstring_utf8(env, from_path)
    };
    if crate::hook::record_read_only_fuse_operation(
        kind,
        &op_name_text,
        &op_filter_text,
        &path_text,
        &from_path_text,
        caller_uid,
        flags,
    ) {
        jni_sys::JNI_TRUE
    } else {
        jni_sys::JNI_FALSE
    }
}

unsafe extern "C" fn allows_public_mapping_target(
    env: *mut JNIEnv,
    _class: jclass,
    path: jstring,
    caller_uid: jni_sys::jint,
) -> jni_sys::jboolean {
    if env.is_null() || path.is_null() {
        return jni_sys::JNI_FALSE;
    }
    let path_text = crate::zygisk::jni::get_jstring_utf8(env, path);
    if should_allow_public_mapping_target_access(&path_text, caller_uid) {
        jni_sys::JNI_TRUE
    } else {
        jni_sys::JNI_FALSE
    }
}

unsafe extern "C" fn capture_binder_caller(
    _env: *mut JNIEnv,
    _class: jclass,
    caller_uid: jni_sys::jint,
    caller_pid: jni_sys::jint,
) {
    crate::hook::capture_java_binder_caller(caller_uid, caller_pid);
}

unsafe extern "C" fn enter_caller_scope(
    _env: *mut JNIEnv,
    _class: jclass,
    caller_uid: jni_sys::jint,
    caller_pid: jni_sys::jint,
) {
    crate::hook::enter_java_caller_scope(caller_uid, caller_pid);
}

unsafe extern "C" fn exit_caller_scope(_env: *mut JNIEnv, _class: jclass) {
    crate::hook::exit_java_caller_scope();
}

unsafe extern "C" fn enter_provider_passthrough(_env: *mut JNIEnv, _class: jclass) {
    crate::hook::enter_provider_passthrough();
}

unsafe extern "C" fn exit_provider_passthrough(_env: *mut JNIEnv, _class: jclass) {
    crate::hook::exit_provider_passthrough();
}

unsafe extern "C" fn is_debug_logging_enabled(
    _env: *mut JNIEnv,
    _class: jclass,
) -> jni_sys::jboolean {
    crate::hook::refresh_runtime_config_throttled();
    if crate::logging::is_debug_logging_enabled() {
        jni_sys::JNI_TRUE
    } else {
        jni_sys::JNI_FALSE
    }
}

unsafe extern "C" fn is_redirect_enabled_for_caller_uid(
    _env: *mut JNIEnv,
    _class: jclass,
    caller_uid: jni_sys::jint,
) -> jni_sys::jboolean {
    if crate::hook::is_redirect_enabled_for_caller_uid(caller_uid) {
        jni_sys::JNI_TRUE
    } else {
        jni_sys::JNI_FALSE
    }
}

fn call_backup(env: *mut JNIEnv, hooker: jobject, args: jobjectArray) -> jobject {
    if env.is_null() || hooker.is_null() || args.is_null() {
        return std::ptr::null_mut();
    }
    let hooker_class = get_object_class(env, hooker);
    if hooker_class.is_null() {
        return std::ptr::null_mut();
    }
    let backup_mid = get_method_id(
        env,
        hooker_class,
        "callBackup",
        "([Ljava/lang/Object;)Ljava/lang/Object;",
    );
    if backup_mid.is_null() {
        return std::ptr::null_mut();
    }
    call_object_method_a(env, hooker, backup_mid, &[jvalue { l: args }])
}
