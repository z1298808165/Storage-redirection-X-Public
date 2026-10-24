use crate::platform::paths;
use crate::redirect::policy;
use crate::zygisk::jni;
use jni_sys::{JNIEnv, jclass, jint, jmethodID, jobject, jobjectArray, jstring, jvalue};
use once_cell::sync::Lazy;
use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU32};

const DOWNLOADS_URI: &str = "content://downloads/all_downloads";
const STORAGE_PREFIX: &str = "/storage/emulated/";
const DOWNLOAD_PROVIDER_NAME: &str = "com.android.providers.downloads";
const FILE_SCHEME_PREFIX: &str = "file://";
const CACHE_TTL_MS: i64 = 15_000;
const CACHE_CAPACITY: usize = 128;
const CACHE_QUEUE_COMPACTION_LIMIT: usize = CACHE_CAPACITY * 2;
// 路径限定放宽后，归因链可能在批量枚举（MTP 扫描全盘等）中对大量互异路径
// 触发查询；真实 JNI 查询按秒限流，超限直接按未命中处理，缓存仍照常登记。
const QUERY_RATE_LIMIT_PER_SECOND: u32 = 8;
static QUERY_WINDOW_SEC: AtomicI64 = AtomicI64::new(-1);
static QUERY_COUNT_IN_WINDOW: AtomicU32 = AtomicU32::new(0);

struct CacheEntry {
    package_name: String,
    expires_at_ms: i64,
    generation: u64,
}

struct DownloadOwnerCache {
    values: HashMap<String, CacheEntry>,
    order: VecDeque<(String, u64)>,
    next_generation: u64,
}

impl DownloadOwnerCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
            next_generation: 0,
        }
    }

    fn get(&mut self, key: &str, now_ms: i64) -> Option<Option<String>> {
        let entry = self.values.get(key)?;

        if entry.expires_at_ms <= now_ms {
            self.values.remove(key);
            return None;
        }

        if entry.package_name.is_empty() {
            return Some(None);
        }
        Some(Some(entry.package_name.clone()))
    }

    fn put(&mut self, key: String, package_name: Option<String>, now_ms: i64) {
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        self.order.push_back((key.clone(), generation));

        self.values.insert(
            key,
            CacheEntry {
                package_name: package_name.unwrap_or_default(),
                expires_at_ms: now_ms + CACHE_TTL_MS,
                generation,
            },
        );

        while self.values.len() > CACHE_CAPACITY {
            let Some((expired_key, generation)) = self.order.pop_front() else {
                break;
            };
            if self
                .values
                .get(&expired_key)
                .is_some_and(|entry| entry.generation == generation)
            {
                self.values.remove(&expired_key);
            }
        }

        if self.order.len() > CACHE_QUEUE_COMPACTION_LIMIT {
            self.order.retain(|(queued_key, generation)| {
                self.values
                    .get(queued_key)
                    .is_some_and(|entry| entry.generation == *generation)
            });
        }
    }
}

static DOWNLOAD_OWNER_CACHE: Lazy<Mutex<DownloadOwnerCache>> =
    Lazy::new(|| Mutex::new(DownloadOwnerCache::new()));

// 按下载目标路径反查发起下载的应用包名
pub fn infer_download_owner_package_by_path(normalized_path: &str) -> Option<String> {
    if !should_lookup_download_owner(normalized_path) {
        return None;
    }

    let now_ms = paths::monotonic_ms();
    let candidates = build_lookup_candidates(normalized_path);
    for candidate in candidates {
        if let Some(cached) = DOWNLOAD_OWNER_CACHE
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(&candidate, now_ms)
        {
            if cached.is_some() {
                return cached;
            }
            continue;
        }

        if !try_acquire_query_slot(now_ms) {
            return None;
        }

        let package_name = query_download_owner_package(&candidate);
        DOWNLOAD_OWNER_CACHE
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .put(candidate, package_name.clone(), now_ms);

        if package_name.is_some() {
            return package_name;
        }
    }

    None
}

// 每秒最多 QUERY_RATE_LIMIT_PER_SECOND 次真实 JNI 查询；抢到窗口切换权的线程
// 重置计数。限流是防御性的，切换瞬间与并发累加交错的计数误差只影响本秒余量。
fn try_acquire_query_slot(now_ms: i64) -> bool {
    let window_sec = now_ms / 1000;
    let current_window = QUERY_WINDOW_SEC.load(std::sync::atomic::Ordering::Relaxed);
    if window_sec != current_window {
        let switched = QUERY_WINDOW_SEC
            .compare_exchange(
                current_window,
                window_sec,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Relaxed,
            )
            .is_ok();
        if switched {
            QUERY_COUNT_IN_WINDOW.store(1, std::sync::atomic::Ordering::Relaxed);
            return true;
        }
    }
    let count = QUERY_COUNT_IN_WINDOW.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    count <= QUERY_RATE_LIMIT_PER_SECOND
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

// 下载行的 destination 可指向任意用户存储路径（公共目录或应用私有目录），
// 归因链只在调用方包名未解析时兜底查询，且结果有 TTL 缓存，因此不按
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

fn query_download_owner_package(path: &str) -> Option<String> {
    jni::with_env(|env| unsafe { query_download_owner_package_impl(env, path) }).flatten()
}

unsafe fn query_download_owner_package_impl(env: *mut JNIEnv, path: &str) -> Option<String> {
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

    let move_to_first = get_method_id(env, cursor_class, "moveToFirst", "()Z")?;
    let moved = call_boolean_method(env, cursor, move_to_first, &[])?;
    if !moved {
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
        return None;
    }

    let get_string = get_method_id(env, cursor_class, "getString", "(I)Ljava/lang/String;")?;
    let get_int = get_method_id(env, cursor_class, "getInt", "(I)I")?;

    let package_name = call_object_method(env, cursor, get_string, &[jvalue { i: 0 }])
        .map(|value| {
            let text = jni::get_jstring_utf8(env, value as jstring);
            jni::delete_local_ref(env, value);
            text
        })
        .filter(|value| !value.is_empty())
        .filter(|value| !policy::is_system_writer_package(value));

    let package_name = if package_name.is_some() {
        package_name
    } else {
        let uid = call_int_method(env, cursor, get_int, &[jvalue { i: 1 }])?;
        resolve_package_by_uid(uid)
    };

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

    package_name
}

fn resolve_package_by_uid(uid: jint) -> Option<String> {
    if uid < 0 {
        return None;
    }

    let mut packages = policy::get_packages_for_uid(uid);
    packages.sort();
    packages.dedup();
    packages.retain(|package_name| {
        !package_name.is_empty() && !policy::is_system_writer_package(package_name)
    });
    // 共享 uid 组内有多个候选时无法确定行属于哪个成员，返回 None 交给
    // 调用方走后续归因路径；硬取排序首个成员会把引擎代写误归给无辜包。
    if packages.len() == 1 {
        return packages.pop();
    }
    None
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
    if class.is_null() || clear_exception(env) {
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
    if method.is_null() || clear_exception(env) {
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
    if method.is_null() || clear_exception(env) {
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
    if value.is_null() || clear_exception(env) {
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
    if value.is_null() || clear_exception(env) {
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
    if array.is_null() || clear_exception(env) {
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
