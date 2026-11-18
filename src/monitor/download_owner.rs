use crate::platform::paths;
use crate::redirect::policy;
use crate::zygisk::jni;
use jni_sys::{JNIEnv, jclass, jint, jmethodID, jobject, jobjectArray, jstring, jvalue};
use std::ffi::CString;

const DOWNLOADS_URI: &str = "content://downloads/all_downloads";
const STORAGE_PREFIX: &str = "/storage/emulated/";
const DOWNLOAD_PROVIDER_NAME: &str = "com.android.providers.downloads";
const FILE_SCHEME_PREFIX: &str = "file://";

// 按下载目标路径反查下载行中的包名和调用 UID。
pub fn infer_download_owner_identity_by_path(
    normalized_path: &str,
    expected_user_id: i32,
) -> Option<(String, i32)> {
    if !should_lookup_download_owner(normalized_path) {
        return None;
    }
    if expected_user_id < 0 || !host_process_may_query_downloads() {
        return None;
    }

    let candidates = build_lookup_candidates(normalized_path);
    for candidate in candidates {
        // 目标路径可被其它下载复用，每次读取当前下载行的归属。
        let Some(owner) = query_download_owner_identity(&candidate)? else {
            continue;
        };
        // 已存在的行不再尝试别名，冲突或失效证据不能借用另一文件的归属。
        return (!policy::is_system_writer_package(&owner.0)
            && download_owner_identity_matches(&owner.0, owner.1, expected_user_id))
        .then_some(owner);
    }

    None
}

fn download_owner_identity_matches(
    package_name: &str,
    row_uid: i32,
    expected_user_id: i32,
) -> bool {
    package_uid_matches_download_row(
        row_uid,
        policy::get_fresh_uid_for_package_in_user(package_name, expected_user_id),
        expected_user_id,
    )
}

fn package_uid_matches_download_row(row_uid: i32, package_uid: i32, expected_user_id: i32) -> bool {
    row_uid >= crate::redirect::writer::ANDROID_APP_UID_START
        && crate::platform::user_id_from_uid(row_uid) == expected_user_id
        && package_uid == row_uid
}

fn unique_download_owner(
    rows: impl IntoIterator<Item = Option<(String, i32)>>,
) -> Option<Option<(String, i32)>> {
    let mut owner = None;
    for row in rows {
        let row = row?;
        if owner.as_ref().is_some_and(|current| current != &row) {
            return None;
        }
        owner = Some(row);
    }
    Some(owner)
}

/// 调用方 uid 是否属于包含下载提供器的共享 uid 组。
///
/// DownloadProvider 引擎（XL）对公共路径的写入全部以 MediaStore insert 形式
/// 到达 MediaProvider 进程，而不是在下载进程内直接 open；「按行数据反查发起者」
/// 因此必须在 MediaProvider 侧的归因链上可用（2026-10-24 真机取证：查表只在
/// 下载进程侧启用时，MediaProvider 侧对组 uid 调用方的归因恒落空）。
pub fn caller_uid_shares_download_group(caller_uid: i32) -> bool {
    caller_uid >= 0
        && policy::get_packages_for_uid(caller_uid)
            .iter()
            .any(|pkg| pkg == DOWNLOAD_PROVIDER_NAME)
}

/// 仅在宿主自身 UID 组包含 DownloadProvider 时查询下载表；请求者属于该组不足以授权宿主。
fn host_process_may_query_downloads() -> bool {
    // SAFETY: libc::getuid 仅读取本进程真实凭据，无前置条件，不会失败。
    let host_uid = unsafe { libc::getuid() } as i32;
    policy::get_packages_for_uid(host_uid)
        .iter()
        .any(|pkg| pkg == DOWNLOAD_PROVIDER_NAME)
}

// 下载行的 destination 可指向任意用户存储路径（公共目录或应用私有目录），
// 归因链只在调用方包名未解析时查询当前下载行，因此不按
// Download 段收窄：MIUI 引擎把商店等应用的下载目标写在私有目录时，
// 按 Download 段收窄会让「行数据→发起者」关联在这些路径上全部落空。
fn should_lookup_download_owner(path: &str) -> bool {
    paths::starts_with(path, STORAGE_PREFIX)
}

fn build_lookup_candidates(path: &str) -> Vec<String> {
    let mut candidates = vec![path.to_string()];
    if let Some(companion_main_path) = normalize_companion_download_path(path)
        && companion_main_path != path
    {
        candidates.push(companion_main_path);
    }
    candidates
}

fn normalize_companion_download_path(path: &str) -> Option<String> {
    let file_name = path.rsplit('/').next()?;
    if !file_name.starts_with('.') || !file_name.ends_with(".js") || file_name.len() <= 4 {
        return None;
    }

    let parent = paths::parent(path);
    if parent.is_empty() {
        return None;
    }

    let normalized_name = &file_name[1..file_name.len() - 3];
    Some(format!("{}/{}", parent, normalized_name))
}

// Some(None) 仅表示没有下载行；查询失败、行无效或归属冲突返回 None。
fn query_download_owner_identity(path: &str) -> Option<Option<(String, i32)>> {
    // SAFETY: with_env 提供当前已附加线程的有效 JNIEnv，查询不在闭包外保存局部引用。
    jni::with_env(|env| unsafe { query_download_owner_identity_impl(env, path) }).flatten()
}

unsafe fn query_download_owner_identity_impl(
    env: *mut JNIEnv,
    path: &str,
) -> Option<Option<(String, i32)>> {
    let activity_thread_class = find_class(env, "android/app/ActivityThread")?;
    let application_class = find_class(env, "android/app/Application")?;
    let content_resolver_class = find_class(env, "android/content/ContentResolver")?;
    let uri_class = find_class(env, "android/net/Uri")?;
    let cursor_class = find_class(env, "android/database/Cursor")?;
    let string_class = find_class(env, "java/lang/String")?;

    let current_application = get_static_method_id(
        env,
        activity_thread_class,
        "currentApplication",
        "()Landroid/app/Application;",
    )?;
    let application =
        call_static_object_method(env, activity_thread_class, current_application, &[])?;

    let get_content_resolver = get_method_id(
        env,
        application_class,
        "getContentResolver",
        "()Landroid/content/ContentResolver;",
    )?;
    let resolver = call_object_method(env, application, get_content_resolver, &[])?;

    let uri_parse = get_static_method_id(
        env,
        uri_class,
        "parse",
        "(Ljava/lang/String;)Landroid/net/Uri;",
    )?;
    let uri_text = jni::new_jstring_utf8(env, DOWNLOADS_URI);
    let uri = call_static_object_method(
        env,
        uri_class,
        uri_parse,
        &[jvalue {
            l: uri_text as jobject,
        }],
    )?;

    let projection = new_string_array(env, string_class, &["notificationpackage", "uid"])?;
    let selection = jni::new_jstring_utf8(env, "_data=? OR hint=?");
    let hint_path = format!("{}{}", FILE_SCHEME_PREFIX, path);
    let selection_args = new_string_array(env, string_class, &[path, &hint_path])?;
    let sort_order = jni::new_jstring_utf8(env, "_id DESC");

    let query = get_method_id(
        env,
        content_resolver_class,
        "query",
        "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
    )?;
    let cursor = call_object_method(
        env,
        resolver,
        query,
        &[
            jvalue { l: uri },
            jvalue {
                l: projection as jobject,
            },
            jvalue {
                l: selection as jobject,
            },
            jvalue {
                l: selection_args as jobject,
            },
            jvalue {
                l: sort_order as jobject,
            },
        ],
    )?;

    let owner = read_download_owner_rows(env, cursor_class, cursor);
    close_cursor(env, cursor_class, cursor);
    clear_local_refs(
        env,
        &[
            uri_text as jobject,
            selection as jobject,
            sort_order as jobject,
            application,
            resolver,
            uri,
            projection as jobject,
            selection_args as jobject,
            cursor,
            activity_thread_class as jobject,
            application_class as jobject,
            content_resolver_class as jobject,
            uri_class as jobject,
            cursor_class as jobject,
            string_class as jobject,
        ],
    );

    owner
}

unsafe fn read_download_owner_rows(
    env: *mut JNIEnv,
    cursor_class: jclass,
    cursor: jobject,
) -> Option<Option<(String, i32)>> {
    let move_to_first = get_method_id(env, cursor_class, "moveToFirst", "()Z")?;
    let move_to_next = get_method_id(env, cursor_class, "moveToNext", "()Z")?;
    let get_string = get_method_id(env, cursor_class, "getString", "(I)Ljava/lang/String;")?;
    let get_int = get_method_id(env, cursor_class, "getInt", "(I)I")?;
    if !call_boolean_method(env, cursor, move_to_first, &[])? {
        return Some(None);
    }

    let mut rows = Vec::new();
    loop {
        let package_name = call_object_method(env, cursor, get_string, &[jvalue { i: 0 }])
            .map(|value| {
                let text = jni::get_jstring_utf8(env, value as jstring);
                jni::delete_local_ref(env, value);
                text
            })
            .filter(|value| !value.is_empty());
        let uid = call_int_method(env, cursor, get_int, &[jvalue { i: 1 }])?;
        rows.push(package_name.map(|package_name| (package_name, uid)));
        if !call_boolean_method(env, cursor, move_to_next, &[])? {
            break;
        }
    }
    unique_download_owner(rows)
}

unsafe fn close_cursor(env: *mut JNIEnv, cursor_class: jclass, cursor: jobject) {
    let Some(close) = get_method_id(env, cursor_class, "close", "()V") else {
        return;
    };
    let _ = call_void_method(env, cursor, close, &[]);
}

unsafe fn find_class(env: *mut JNIEnv, name: &str) -> Option<jclass> {
    let table = *env;
    let class_name = CString::new(name).ok()?;
    let class = ((*table).v1_1.FindClass)(env, class_name.as_ptr());
    // 必须先清理再判空：调用失败（返回 null）时异常必定 pending，
    // 短路写法 `is_null() || clear_exception` 会在失败时跳过清理，
    // 残留的 pending 异常会让后续 JNI 调用 abort 整个宿主进程
    // （2026-10-29 真机：media.module 因此反复 SIGABRT）。
    let threw = clear_exception(env);
    if class.is_null() || threw {
        return None;
    }
    Some(class)
}

unsafe fn get_method_id(
    env: *mut JNIEnv,
    class: jclass,
    name: &str,
    sig: &str,
) -> Option<jmethodID> {
    let table = *env;
    let method_name = CString::new(name).ok()?;
    let method_sig = CString::new(sig).ok()?;
    let method = ((*table).v1_1.GetMethodID)(env, class, method_name.as_ptr(), method_sig.as_ptr());
    let threw = clear_exception(env);
    if method.is_null() || threw {
        return None;
    }
    Some(method)
}

unsafe fn get_static_method_id(
    env: *mut JNIEnv,
    class: jclass,
    name: &str,
    sig: &str,
) -> Option<jmethodID> {
    let table = *env;
    let method_name = CString::new(name).ok()?;
    let method_sig = CString::new(sig).ok()?;
    let method =
        ((*table).v1_1.GetStaticMethodID)(env, class, method_name.as_ptr(), method_sig.as_ptr());
    let threw = clear_exception(env);
    if method.is_null() || threw {
        return None;
    }
    Some(method)
}

unsafe fn call_static_object_method(
    env: *mut JNIEnv,
    class: jclass,
    method: jmethodID,
    args: &[jvalue],
) -> Option<jobject> {
    let table = *env;
    let value = ((*table).v1_1.CallStaticObjectMethodA)(env, class, method, args.as_ptr());
    let threw = clear_exception(env);
    if value.is_null() || threw {
        return None;
    }
    Some(value)
}

unsafe fn call_object_method(
    env: *mut JNIEnv,
    obj: jobject,
    method: jmethodID,
    args: &[jvalue],
) -> Option<jobject> {
    let table = *env;
    let value = ((*table).v1_1.CallObjectMethodA)(env, obj, method, args.as_ptr());
    let threw = clear_exception(env);
    if value.is_null() || threw {
        return None;
    }
    Some(value)
}

unsafe fn call_boolean_method(
    env: *mut JNIEnv,
    obj: jobject,
    method: jmethodID,
    args: &[jvalue],
) -> Option<bool> {
    let table = *env;
    let value = ((*table).v1_1.CallBooleanMethodA)(env, obj, method, args.as_ptr());
    if clear_exception(env) {
        return None;
    }
    Some(value == jni_sys::JNI_TRUE)
}

unsafe fn call_int_method(
    env: *mut JNIEnv,
    obj: jobject,
    method: jmethodID,
    args: &[jvalue],
) -> Option<jint> {
    let table = *env;
    let value = ((*table).v1_1.CallIntMethodA)(env, obj, method, args.as_ptr());
    if clear_exception(env) {
        return None;
    }
    Some(value)
}

unsafe fn call_void_method(
    env: *mut JNIEnv,
    obj: jobject,
    method: jmethodID,
    args: &[jvalue],
) -> bool {
    let table = *env;
    ((*table).v1_1.CallVoidMethodA)(env, obj, method, args.as_ptr());
    !clear_exception(env)
}

unsafe fn new_string_array(
    env: *mut JNIEnv,
    string_class: jclass,
    values: &[&str],
) -> Option<jobjectArray> {
    let table = *env;
    let array = ((*table).v1_1.NewObjectArray)(
        env,
        values.len() as jint,
        string_class,
        std::ptr::null_mut(),
    );
    let threw = clear_exception(env);
    if array.is_null() || threw {
        return None;
    }

    for (index, value) in values.iter().enumerate() {
        let string_value = jni::new_jstring_utf8(env, value);
        if string_value.is_null() {
            continue;
        }
        ((*table).v1_1.SetObjectArrayElement)(env, array, index as jint, string_value as jobject);
        jni::delete_local_ref(env, string_value as jobject);
        if clear_exception(env) {
            jni::delete_local_ref(env, array as jobject);
            return None;
        }
    }

    Some(array)
}

unsafe fn clear_exception(env: *mut JNIEnv) -> bool {
    let table = *env;
    if !((*table).v1_2.ExceptionCheck)(env) {
        return false;
    }
    ((*table).v1_1.ExceptionClear)(env);
    true
}

unsafe fn clear_local_refs(env: *mut JNIEnv, values: &[jobject]) {
    for value in values {
        jni::delete_local_ref(env, *value);
    }
}

#[cfg(test)]
#[path = "../../tests/native-unit/download_owner.rs"]
mod tests;
