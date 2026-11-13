// 存储根、用户目录与 Android 私有目录的路径换算。
//
// 这里只做「路径形态」的换算，不涉及挂载与策略决策；返回值取决于传入路径是否
// 符合对应形态，调用方仍需自行判断结果是否可用于挂载。

use crate::platform::paths::{
    DATA_MEDIA_PREFIX, STORAGE_EMULATED_PREFIX, join, normalize, normalize_syntax, starts_with,
};

/// `/data/data` 是否为指向 `/data/user/0` 的符号链接。
///
/// 是符号链接时两种写法解析到同一个 vfsmount，归一化是等价变换；是真实目录时
/// （AOSP 上 `/data/user/0` 是 `/data/data` 的派生 bind）两者是**不同的 vfsmount**，
/// 而挂载按 (父 vfsmount, dentry) 索引——归一化会让挂载落在应用不会走的那条路径上，
/// 表现为映射静默失效（应用写入成功、后端没有文件）。
///
/// 该形态由 init 在启动时建立、之后不再变化，因此缓存一次。取不到元信息时按符号链接
/// 处理，保持本函数改动前的行为。
fn data_data_is_symlink() -> bool {
    static IS_SYMLINK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IS_SYMLINK.get_or_init(|| {
        std::fs::symlink_metadata("/data/data")
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(true)
    })
}

/// 把主用户路径改写为指定用户的路径。
///
/// 只替换路径**前缀**：此前用的是整串 `replace`，会改写路径中间出现的同名字面量。
/// 例如 user 10 访问 `/storage/emulated/0/Download/storage/emulated/0/note.txt`
/// （备份类目录里确实会出现该字面量）时两处都被改写，得到的路径与真实文件不符，
/// 后续只读与映射判定就作用在错误的对象上。
pub fn resolve_user_path(path: &str, user_id: i32) -> String {
    // `/data/data` 是 `/data/user/0` 的历史别名。只有在两者确实是同一个 vfsmount
    // （`/data/data` 为符号链接）时才归一化；是真实目录时保持字面形态，否则挂载会落到
    // 应用访问不到的路径上，映射静默失效。
    if data_data_is_symlink()
        && let Some(rest) = path.strip_prefix("/data/data")
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("/data/user/{user_id}{rest}");
    }

    if user_id == 0 {
        return path.to_string();
    }

    // 命中前缀时一次分配即可完成改写。
    if let Some(rest) = path.strip_prefix("/storage/emulated/0")
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("/storage/emulated/{}{}", user_id, rest);
    }

    if let Some(rest) = path.strip_prefix("/data/user/0")
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("/data/user/{}{}", user_id, rest);
    }

    path.to_string()
}

pub fn storage_user_root_for_user(user_id: i32) -> String {
    format!("{}{}", STORAGE_EMULATED_PREFIX, user_id)
}

pub fn data_media_user_root_for_user(user_id: i32) -> String {
    format!("{}{}", DATA_MEDIA_PREFIX, user_id)
}

/// 宿主进程内数据面 f2fs 直连根的锚点（仅宿主子进程建立后为 `Some`）。
///
/// 背景：应用接入把宿主挂载 move_mount 到应用命名空间的 /storage/emulated/<user>
/// 后，AOSP 存储 bind 链的 shared 传播组会把同一挂载**传播回宿主命名空间的
/// /data/media/<user>**（用户设备 mountinfo 实证 master:104 的 fuse 副本条目）。
/// 该路径是宿主数据面 backend 的直连 f2fs 真身，被覆盖后任何 backend stat 的
/// 路径解析都会命中自家连接的 dentry 重验证（请求发回自己的连接、无人应答），
/// 全部 worker 死锁。锚点在宿主子进程启动早期（任何应用接入之前）把
/// /data/media/<user> bind 到模块私有目录：bind 记录的是当时的 f2fs 挂载实例，
/// 之后 /data/media/<user> 被传播副本覆盖也不影响经锚点的路径解析，数据面与
/// 被污染路径彻底解耦。
static MEDIA_REAL_ANCHOR_ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// 宿主子进程建立锚点后调用，登记锚点根（/data/media/<user> 的映像目录）。
pub fn set_media_real_anchor_root(anchor_root: String) {
    let _ = MEDIA_REAL_ANCHOR_ROOT.set(anchor_root);
}

/// 停摆取证用：当前进程锚点是否已建立。`None` 表示无锚点（数据面走原路径，
/// 停摆现场的 mountinfo 应能据此判断自再入来源）。
pub fn media_real_anchor_root() -> Option<&'static str> {
    MEDIA_REAL_ANCHOR_ROOT.get().map(String::as_str)
}

/// 数据面 f2fs 直连根：宿主进程返回锚点（未被传播副本污染），其余上下文
/// （daemon、scoped 会话 worker）返回原路径 /data/media/<user>——那些命名
/// 空间里不存在该副本，原路径始终安全。
pub fn data_media_user_root_for_current_ns(user_id: i32) -> String {
    match (MEDIA_REAL_ANCHOR_ROOT.get(), user_id) {
        (Some(anchor), 0) => anchor.clone(),
        _ => data_media_user_root_for_user(user_id),
    }
}

/// 把锚点形态的路径转回 /data/media 形式，供覆盖挂载感知解析
/// （backend_overlay 查询的是 init 命名空间里以 /data/media 形式登记的覆盖层）。
/// 非 /data/media 形式的路径（real_root 的 MP 视图等）原样返回。
pub fn media_real_form_of(path: &str) -> String {
    if let Some(anchor) = MEDIA_REAL_ANCHOR_ROOT.get()
        && let Some(rest) = path.strip_prefix(anchor.as_str())
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("/data/media{rest}");
    }
    path.to_string()
}

/// 返回同一用户共享存储的所有系统路径别名。
///
/// 挂载创建和挂载状态校验必须使用完全一致的别名集合，否则状态文件中的
/// `/data/media/<user>` 与应用 namespace 中的 `/storage/emulated/<user>` 会被
/// 误判成两个独立挂载组，重启后持续触发重复重挂载。
pub fn storage_alias_roots_for_user(user_id: i32) -> Vec<String> {
    let user_str = user_id.to_string();
    let mut alias_roots = Vec::with_capacity(16);
    alias_roots.push(storage_user_root_for_user(user_id));
    alias_roots.push(data_media_user_root_for_user(user_id));
    alias_roots.push("/storage/self/primary".to_string());
    if user_id == 0 {
        alias_roots.push("/storage/emulated/legacy".to_string());
    }
    alias_roots.push(format!("/mnt/user/{user_str}/emulated/{user_str}"));
    alias_roots.push(format!("/mnt/runtime/default/emulated/{user_str}"));
    alias_roots.push(format!("/mnt/runtime/read/emulated/{user_str}"));
    alias_roots.push(format!("/mnt/runtime/write/emulated/{user_str}"));
    alias_roots.push(format!("/mnt/runtime/full/emulated/{user_str}"));
    alias_roots.push(format!("/mnt/installer/{user_str}/emulated/{user_str}"));
    alias_roots.push(format!("/mnt/installer/emulated/{user_str}"));
    alias_roots.push(format!(
        "/mnt/androidwritable/{user_str}/emulated/{user_str}"
    ));
    alias_roots.push(format!("/mnt/androidwritable/emulated/{user_str}"));
    alias_roots.push(format!("/mnt/pass_through/{user_str}/emulated/{user_str}"));
    alias_roots.push(format!("/mnt/pass_through/emulated/{user_str}"));
    alias_roots
}

pub fn default_redirect_target(package_name: &str, user_id: i32) -> String {
    format!(
        "{}/Android/data/{}/sdcard",
        storage_user_root_for_user(user_id),
        package_name
    )
}

pub fn is_default_redirect_backend_path(path: &str) -> bool {
    let Some(rest) = path.trim_end_matches('/').strip_prefix(DATA_MEDIA_PREFIX) else {
        return false;
    };

    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let Some(user_id) = parts.next() else {
        return false;
    };
    if !user_id.chars().all(|ch| ch.is_ascii_digit())
        || parts.next() != Some("Android")
        || parts.next() != Some("data")
    {
        return false;
    }

    let Some(package_name) = parts.next() else {
        return false;
    };
    is_valid_package_name(package_name) && parts.next() == Some("sdcard") && parts.next().is_none()
}

pub fn data_media_to_storage_path(path: &str) -> String {
    if !starts_with(path, DATA_MEDIA_PREFIX) {
        return path.to_string();
    }
    format!(
        "{}{}",
        STORAGE_EMULATED_PREFIX,
        &path[DATA_MEDIA_PREFIX.len()..]
    )
}

pub fn storage_to_data_media_path(path: &str) -> String {
    if !starts_with(path, STORAGE_EMULATED_PREFIX) {
        return path.to_string();
    }
    format!(
        "{}{}",
        DATA_MEDIA_PREFIX,
        &path[STORAGE_EMULATED_PREFIX.len()..]
    )
}

// quality-allow(lint-suppression): 两个按用户换算的函数只被部分构建目标使用，
pub fn storage_to_data_media_for_user(storage_path: &str, user_id: i32) -> Option<String> {
    let prefix = format!("{}/", storage_user_root_for_user(user_id));
    let normalized = normalize(storage_path);
    let suffix = normalized.strip_prefix(&prefix)?;
    if suffix.is_empty() {
        return None;
    }
    Some(join(&data_media_user_root_for_user(user_id), suffix))
}

// quality-allow(lint-suppression): 两个按用户换算的函数只被部分构建目标使用，
// 保留 dead_code 豁免以免在缺少调用方的目标上构建失败。
#[allow(dead_code)]
pub fn storage_relative_path_for_user(storage_path: &str, user_id: i32) -> Option<String> {
    let prefix = format!("{}/", storage_user_root_for_user(user_id));
    let normalized = normalize(storage_path);
    let suffix = normalized.strip_prefix(&prefix)?;
    if suffix.is_empty() {
        return None;
    }
    Some(suffix.to_string())
}

pub fn storage_user_root(path: &str) -> Option<String> {
    let rest = path.strip_prefix(STORAGE_EMULATED_PREFIX)?;
    let mut parts = rest.split('/');
    let user_id = parts.next()?;
    if user_id.is_empty() || !user_id.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    Some(format!("{}{}", STORAGE_EMULATED_PREFIX, user_id))
}

pub fn data_media_user_root(path: &str) -> Option<String> {
    let rest = path.strip_prefix(DATA_MEDIA_PREFIX)?;
    let mut parts = rest.split('/');
    let user_id = parts.next()?;
    if user_id.is_empty() || !user_id.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    Some(format!("{}{}", DATA_MEDIA_PREFIX, user_id))
}

pub fn android_private_data_media_root(
    normalized_path: &str,
    owner_package: &str,
    user_id: i32,
) -> Option<String> {
    let storage_prefix = format!("{}/Android/", storage_user_root_for_user(user_id));
    let rest = normalized_path.strip_prefix(&storage_prefix)?;

    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let category = parts.next()?;
    if category != "data" && category != "media" && category != "obb" {
        return None;
    }
    if parts.next() != Some(owner_package) {
        return None;
    }

    Some(format!(
        "{}/Android/{}/{}",
        data_media_user_root_for_user(user_id),
        category,
        owner_package
    ))
}

// 解析 /storage/emulated/<user_id>/... 中的 user_id，失败返回 -1
pub fn extract_user_id_from_storage_path(path: &str) -> i32 {
    const PREFIX: &str = "/storage/emulated/";
    if !path.starts_with(PREFIX) {
        return -1;
    }

    let user_start = PREFIX.len();
    let user_end = path[user_start..]
        .find('/')
        .map(|idx| user_start + idx)
        .unwrap_or(path.len());
    if user_start >= user_end {
        return -1;
    }

    if !path[user_start..user_end]
        .chars()
        .all(|ch| ch.is_ascii_digit())
    {
        return -1;
    }

    path[user_start..user_end].parse().unwrap_or(-1)
}

/// 从 `/data/media/<user>/...` 后端路径提取 Android 用户 ID。
///
/// 只能做语法归一化：`normalize` 会把 `/data/media/<user>/...` 改写为
/// `/storage/emulated/<user>/...`，先归一化再按 `/data/media/` 前缀截取会永远拿不到
/// 用户段，函数退化成恒返回 `None`，调用方基于它的后端识别分支随之失效。
pub fn extract_user_id_from_data_media_path(path: &str) -> Option<i32> {
    let normalized = normalize_syntax(path);
    let rest = normalized.strip_prefix(DATA_MEDIA_PREFIX)?;
    let user = rest.split('/').next()?.parse::<i32>().ok()?;
    (user >= 0).then_some(user)
}

pub fn extract_android_private_path_owner(path: &str) -> String {
    const PREFIX: &str = "/storage/emulated/";
    let Some(rest) = path.strip_prefix(PREFIX) else {
        return String::new();
    };

    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let Some(user_id) = parts.next() else {
        return String::new();
    };
    if !user_id.chars().all(|ch| ch.is_ascii_digit()) {
        return String::new();
    }
    if parts.next() != Some("Android") {
        return String::new();
    }

    match parts.next() {
        Some("data" | "media" | "obb") => {}
        _ => return String::new(),
    }

    let Some(package_name) = parts.next() else {
        return String::new();
    };
    if is_valid_package_name(package_name) {
        package_name.to_string()
    } else {
        String::new()
    }
}

pub(crate) fn is_valid_package_name(package_name: &str) -> bool {
    !package_name.is_empty()
        && package_name.contains('.')
        && !package_name.starts_with('.')
        && !package_name.ends_with('.')
        && package_name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
        && package_name.split('.').all(|part| !part.is_empty())
}
