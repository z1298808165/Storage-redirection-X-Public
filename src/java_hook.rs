// Java hook 模块入口：加载内嵌 DEX、拿 Hooker class 引用、绑定 native callback
// 由 specialize_pre 在 MediaProvider 目标进程内触发；失败不影响其他路径

#![allow(dead_code)]

mod dex_loader;
mod hooker_class;
mod lsplant;

use crate::zygisk::jni::{
    call_static_boolean_method_a, delete_local_ref, get_static_method_id, new_jstring_utf8,
};
use jni_sys::{JNIEnv, jobject, jvalue};
use std::sync::atomic::{AtomicBool, Ordering};

// build.rs 调 d8 产出；工具链缺失时为空，运行时探测 dex_loader 自动返回 null 失败
const HOOKER_DEX: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/Hooker.dex"));
const PACKAGE_EVENT_RECEIVER_DEX: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/PackageEventReceiver.dex"));
static MEDIA_PROVIDER_HOOK_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub fn is_available() -> bool {
    !HOOKER_DEX.is_empty()
}

pub fn dex_len() -> usize {
    HOOKER_DEX.len()
}

pub fn install_package_event_receiver(env: *mut JNIEnv, module_dir: &str) -> bool {
    if PACKAGE_EVENT_RECEIVER_DEX.is_empty() || env.is_null() || module_dir.is_empty() {
        return false;
    }

    let clazz = dex_loader::load_dex_class(
        env,
        PACKAGE_EVENT_RECEIVER_DEX,
        "org.srx.hook.PackageEventReceiver",
    );
    if clazz.is_null() {
        log::warn!("package event receiver load dex class failed");
        return false;
    }

    let method = get_static_method_id(env, clazz, "install", "(Ljava/lang/String;)Z");
    if method.is_null() {
        log::warn!("package event receiver install method missing");
        delete_local_ref(env, clazz);
        return false;
    }

    let module_dir_jstr = new_jstring_utf8(env, module_dir);
    if module_dir_jstr.is_null() {
        log::warn!("package event receiver module dir jstring failed");
        delete_local_ref(env, clazz);
        return false;
    }

    let ok = call_static_boolean_method_a(
        env,
        clazz,
        method,
        &[jvalue {
            l: module_dir_jstr as jobject,
        }],
    );
    delete_local_ref(env, module_dir_jstr as jobject);
    delete_local_ref(env, clazz);
    ok
}

// 在目标进程内初始化一次；成功后 hooker_class() 返回全局引用
pub fn init_once(env: *mut JNIEnv) -> bool {
    if MEDIA_PROVIDER_HOOK_INITIALIZED.load(Ordering::Acquire) {
        return true;
    }
    if !init(env, "installMediaProviderHook", "media provider") {
        return false;
    }
    MEDIA_PROVIDER_HOOK_INITIALIZED.store(true, Ordering::Release);
    true
}

fn init(env: *mut JNIEnv, install_method: &str, hook_name: &str) -> bool {
    if HOOKER_DEX.is_empty() || env.is_null() {
        return false;
    }
    log::info!("java hook {} dex bytes={}", hook_name, HOOKER_DEX.len());
    let clazz = dex_loader::load_dex_class(env, HOOKER_DEX, "org.srx.hook.Hooker");
    if clazz.is_null() {
        log::warn!("java hook {} load dex class failed", hook_name);
        return false;
    }
    let class_init_ok = hooker_class::init(env, clazz);
    if !class_init_ok {
        delete_local_ref(env, clazz);
        return false;
    }
    let install_ok = install_hook(env, clazz, install_method);
    if !install_ok {
        log::warn!("java hook install {} hook failed", hook_name);
    }
    delete_local_ref(env, clazz);
    install_ok
}

fn install_hook(env: *mut JNIEnv, clazz: jni_sys::jclass, method_name: &str) -> bool {
    let method = get_static_method_id(env, clazz, method_name, "()Z");
    if method.is_null() {
        return false;
    }
    call_static_boolean_method_a(env, clazz, method, &[])
}

#[allow(unused_imports)]
pub use hooker_class::hooker_class;
