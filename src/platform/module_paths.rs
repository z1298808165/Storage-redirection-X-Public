pub const MODULE_DIR: &str = "/data/adb/modules/storage.redirect.x";
pub const MOUNT_STATE_DIR: &str = "/data/adb/modules/storage.redirect.x/tmp/mount_state";
pub const MOUNT_INTENT_DIR: &str = "/data/adb/modules/storage.redirect.x/tmp/mount_intent";
pub const REAL_STORAGE_TMP_DIR: &str = "/data/adb/modules/storage.redirect.x/tmp/real_storage";
pub const REAL_STORAGE_TMP_PREFIX: &str = "/data/adb/modules/storage.redirect.x/tmp/real_storage/";
pub const CONFIG_DIR: &str = "/data/adb/modules/storage.redirect.x/config";
pub const RUNTIME_DISABLE_FILE: &str = "/data/adb/modules/storage.redirect.x/.runtime_disabled";
pub const FUSE_PROBE_FILE: &str = "/data/adb/modules/storage.redirect.x/.fuse_probe";
/// daemon 写入、普通应用只读的 FUSE 能力快照。
pub const FUSE_CAPABILITY_FILE: &str = "/data/adb/modules/storage.redirect.x/.fuse_capability";
pub const MEDIA_HOOK_DEFERRED_FILE: &str =
    "/data/adb/modules/storage.redirect.x/logs/.media_hook_deferred";
/// MediaProvider Java hook 安装结果的落盘记录。
///
/// 系统代写进程只输出 logcat、不写模块日志文件，而 MediaProvider 的
/// specialize 发生在测试流清空 logcat 之前，因此安装结果在事后排查时
/// 两边都取不到。这里把结果写入文件，使「hook 从未安装」与
/// 「已安装但未触发」在失败后仍可区分。
pub const MEDIA_HOOK_INSTALL_STATE_FILE: &str =
    "/data/adb/modules/storage.redirect.x/logs/.media_hook_install_state";
/// MediaProvider 进程内热重载请求与确认文件。
pub const MEDIA_PROVIDER_HOT_RELOAD_REQUEST_FILE: &str =
    "/data/adb/modules/storage.redirect.x/logs/.media_provider_hot_reload.request";
pub const MEDIA_PROVIDER_HOT_RELOAD_ACK_FILE: &str =
    "/data/adb/modules/storage.redirect.x/logs/.media_provider_hot_reload.ack";
pub const RECENT_SOURCE_HINT_FILE: &str =
    "/data/adb/modules/storage.redirect.x/logs/.recent_source_hint";
pub const RECENT_PATH_CALLER_HINT_FILE: &str =
    "/data/adb/modules/storage.redirect.x/logs/.recent_path_caller_hint";
pub const SYSTEM_WRITER_UIDS_FILE: &str =
    "/data/adb/modules/storage.redirect.x/config/system_writer_uids.list";
pub const LOG_DIR: &str = "/data/adb/modules/storage.redirect.x/logs";

/// 过滤并归一化挂载点清单。
///
/// daemon 挂载与 companion 挂载都需要把挂载目标写入同一份状态文件格式，
/// 因此排序规则必须保持一致：先按路径长度降序，再按字典序降序，
/// 保证子目录排在父目录之前，卸载时可以从最深层开始，随后去重。
pub fn normalize_mount_targets(targets: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = targets
        .iter()
        .filter(|target| is_safe_mount_target(target))
        .cloned()
        .collect();
    normalized.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| b.cmp(a)));
    normalized.dedup();
    normalized
}

/// 判断挂载目标是否位于本模块允许操作的目录范围内。
///
/// 只允许 `/storage/`、`/mnt/` 与模块自身的真实存储临时目录；
/// 空路径、含 NUL 或含 `/../` 的路径一律拒绝，避免状态文件被污染后卸载到无关目录。
pub fn is_safe_mount_target(target: &str) -> bool {
    if target.is_empty() || target.contains('\0') || target.contains("/../") {
        return false;
    }
    target.starts_with("/storage/")
        || target.starts_with("/mnt/")
        || target.starts_with(REAL_STORAGE_TMP_PREFIX)
}

/// 把任意字符串转换为可安全用于文件名的形式。
///
/// 仅保留 ASCII 字母数字与 `.`、`_`、`-`，其余字符替换为 `_`，
/// 用于按包名生成挂载状态文件名。
pub fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
