// 挂载目标的来源构建与 /proc/<pid>/mountinfo 判读。
//
// 这里只回答两个问题：「这次要建哪几个来源根」和「mountinfo 里这条记录是不是我们要的」。
// 挂载动作与顺序留在 apply，改判定口径不会碰挂载流程。

use crate::domain::PathMapping;
use crate::platform::{mountinfo, paths};
use libc::{MNT_DETACH, umount2};
use std::ffi::CString;

pub(super) fn path_overlaps_mapping_request(path: &str, path_mappings: &[PathMapping]) -> bool {
    path_mappings
        .iter()
        .any(|mapping| paths::matches(&mapping.request_path, path, true))
}

pub(super) fn path_shadows_mapping_request(path: &str, path_mappings: &[PathMapping]) -> bool {
    path_mappings
        .iter()
        .any(|mapping| paths::is_same_or_child(&mapping.request_path, path))
}

pub(super) fn collect_restored_read_only_excluded_children(
    read_only_paths: &[String],
    excluded_rules: &[String],
    path_mappings: &[PathMapping],
    scoped_fuse_roots: &[String],
) -> Vec<String> {
    let mut restored_children: Vec<String> = Vec::new();
    for read_only_path in read_only_paths {
        restored_children.extend(
            excluded_rules
                .iter()
                .filter(|excluded| {
                    !paths::contains_wildcards(excluded)
                        && paths::is_child(excluded, read_only_path)
                        && !path_overlaps_mapping_request(excluded, path_mappings)
                        && !is_covered_by_scoped_fuse_mount(excluded, scoped_fuse_roots)
                })
                .cloned(),
        );
    }
    paths::sort_dedup_paths_longest_first_case_insensitive(&mut restored_children);
    restored_children
}

pub(super) fn build_mapping_source_roots(
    real_storage_anchor: &Option<String>,
    data_media_root: &str,
) -> Vec<String> {
    let mut roots = Vec::with_capacity(2);
    if let Some(anchor) = real_storage_anchor {
        roots.push(anchor.clone());
    }
    if !roots.iter().any(|root| root == data_media_root) {
        roots.push(data_media_root.to_string());
    }
    roots
}

/// 只读挂载的内容来源顺序：真实后端优先，可见别名锚点只作回退。
///
/// 可见锚点是 `/storage/emulated/<user>` 的 bind，在 Android 11 及以上它背后是系统
/// FUSE，属于派生视图；模块自身与测试都在 `/data/media/<user>` 上直接建目录和写种子，
/// 这些改动不会自动让系统 FUSE 失效，锚点因此可能把刚写入的真实文件读成空或旧内容。
/// 只读规则只限制写入、不改变内容归属，因此固定以真实后端为权威来源。
pub(super) fn build_read_only_source_roots(
    source_roots: &[String],
    data_media_root: &str,
) -> Vec<String> {
    let mut roots = Vec::with_capacity(source_roots.len().saturating_add(1));
    if !data_media_root.is_empty() {
        roots.push(data_media_root.to_string());
    }
    for root in source_roots {
        if root != data_media_root && !roots.iter().any(|existing| existing == root) {
            roots.push(root.clone());
        }
    }
    roots
}

pub(super) fn build_allowed_real_source_candidates(
    real_storage_anchor: &Option<String>,
    data_media_root: &str,
    relative: &str,
) -> Vec<String> {
    let backend_source = paths::join(data_media_root, relative);
    let mut candidates = Vec::with_capacity(2);
    if let Some(anchor) = real_storage_anchor {
        let anchor_source = paths::join(anchor, relative);
        if !paths::eq_ignore_case(&anchor_source, &backend_source) {
            candidates.push(anchor_source);
        }
    }
    candidates.push(backend_source.clone());
    candidates
}

pub(super) fn namespace_mappings_outside_scoped_fuse(
    resolved_mappings: &[PathMapping],
    scoped_fuse_roots: &[String],
) -> Vec<PathMapping> {
    if scoped_fuse_roots.is_empty() {
        return resolved_mappings.to_vec();
    }

    resolved_mappings
        .iter()
        .filter(|mapping| {
            !scoped_fuse_roots
                .iter()
                .any(|root| paths::is_same_or_child(&mapping.request_path, root))
        })
        .cloned()
        .collect()
}

pub(super) fn is_covered_by_scoped_fuse_mount(path: &str, scoped_fuse_roots: &[String]) -> bool {
    scoped_fuse_roots
        .iter()
        .any(|root| paths::eq_ignore_case(path, root) || paths::is_child(path, root))
}

/// 读取当前进程的挂载表。
///
/// `mount::planner` 在重挂载之前也要用挂载表判断目标是否真的是挂载点，因此这里共享
/// 同一份读取实现，避免两个模块各写一遍解析逻辑。
pub(super) fn read_mountinfo() -> Option<String> {
    std::fs::read_to_string("/proc/self/mountinfo").ok()
}

pub(super) fn mount_source_for_target_from_mountinfo(
    content: &str,
    target: &str,
) -> Option<String> {
    let normalized_target = paths::normalize(target);
    let mut matched_root: Option<String> = None;
    for line in content.lines() {
        let Some(entry) = mountinfo::parse_entry(line) else {
            continue;
        };
        if !paths::eq_ignore_case(
            &paths::normalize(&mountinfo::unescape_field(entry.target)),
            &normalized_target,
        ) {
            continue;
        }
        // 命中项的 target 长度一致，沿用原先 max_by_key 的“取最后一个命中”语义；
        // 只有命中时才展开 root 字段，未命中的行不再产生分配。
        matched_root = Some(mountinfo::unescape_field(entry.root));
    }
    matched_root
}

/// 只判断挂载点是否存在，命中首个匹配即返回，不解析也不分配 root 字段。
pub(super) fn mountinfo_has_target(content: &str, target: &str) -> bool {
    let normalized_target = paths::normalize(target);
    content.lines().any(|line| {
        mountinfo::parse_entry(line)
            .map(|entry| {
                paths::eq_ignore_case(
                    &paths::normalize(&mountinfo::unescape_field(entry.target)),
                    &normalized_target,
                )
            })
            .unwrap_or(false)
    })
}

/// 把 `mountinfo` 的 `root` 字段归一为 `/data/media/<user>/...` 形态。
///
/// `root` 有两种等价写法：经系统 FUSE 视图时是 `/<user>/...`（真实记录形如
/// `0:98 /0/Android/data/<包名>/sdcard /storage/emulated/0 ... - fuse /dev/fuse`），经
/// `/data` 分区视图时是 `/media/<user>/...`。两者指向同一棵存储树，判定前必须归一，
/// 否则只认得出其中一半。
///
/// 这不是理论差异：bind 会把源挂载的 `source` 与 `fs_type` 一并继承，视图根只要由系统
/// FUSE 视图承载，`root` 就写作 `/0/...`。旧实现只处理 `/media/...`，于是本已重定向的
/// 视图根被判成「未重定向」，继而按未重定向流程重建真实存储锚点：可见别名此刻全部指向
/// 沙箱（构造上必然如此）而全部判污染，最终退回 `/data/media` 后端锚点——该锚点在 ext4
/// 设备上绕过系统 FUSE 权限层，映射目标对普通应用不可写（Android 14 场景 29 实测
/// `Permission denied`，Android 13 则表现为视图根被 FUSE 层覆盖后 MediaProvider 拒绝）。
///
/// 只接受 `<user>/<非空尾部>` 形态：`/`、`/media`、`/0` 这类没有存储子树尾部的 `root`
/// 说明不了重定向，一律返回 `None`。
fn mountinfo_root_as_data_media(root: &str) -> Option<String> {
    let rest = match root.strip_prefix("/media/") {
        Some(rest) => rest,
        None => root.strip_prefix('/')?,
    };
    let (user, tail) = rest.split_once('/')?;
    if user.is_empty() || !user.bytes().all(|byte| byte.is_ascii_digit()) || tail.is_empty() {
        return None;
    }
    Some(format!("/data/media/{rest}"))
}

/// `root`（mountinfo 的 `root` 字段）是否表示 `backend`（`/data/media/<user>/...` 形态）
/// 那棵沙箱子树。两种 `root` 写法都接受，见 [`mountinfo_root_as_data_media`]。
pub(super) fn mountinfo_root_matches_data_backend(root: &str, backend: &str) -> bool {
    paths::eq_ignore_case(root, backend)
        || mountinfo_root_as_data_media(root)
            .map(|source| paths::is_same_or_child(backend, &source))
            .unwrap_or(false)
}

/// 判断挂载点 `target` 上那条记录的 `root` 是否落在该应用的私有沙箱里。
///
/// 用于识别「锚点被上一轮重定向污染」这一种形态：健康锚点的 `root` 指向真实存储根
/// （形如 `/media/<user>`），被污染时则带 `Android/data/<包名>`。
pub(super) fn mountinfo_root_is_app_sandbox(
    content: &str,
    target: &str,
    package_name: &str,
) -> bool {
    if package_name.is_empty() {
        return false;
    }
    mount_source_for_target_from_mountinfo(content, target)
        .map(|root| {
            ["Android/data/", "Android/media/", "Android/obb/"]
                .iter()
                .any(|prefix| root.contains(&format!("{prefix}{package_name}")))
        })
        .unwrap_or(false)
}
pub(super) fn detach_mount_if_present(target: &str) {
    // 该函数会 umount2 改变挂载表，且被 bind_mount 之间反复调用，必须每次重新读取；
    // 这里只需要判断挂载点是否存在，用存在性探测替代取 source 的整表扫描。
    let present = read_mountinfo()
        .map(|content| mountinfo_has_target(&content, target))
        .unwrap_or(false);
    if !present {
        return;
    }
    let Ok(c_target) = CString::new(target) else {
        return;
    };
    // SAFETY: c_target 指向本作用域内以 NUL 结尾的合法路径，MNT_DETACH 只作用于该挂载点。
    let ret = unsafe { umount2(c_target.as_ptr(), MNT_DETACH) };
    if ret != 0 {
        log::warn!(
            "real storage anchor detach failed target={} errno={}",
            target,
            // SAFETY: __errno 返回当前线程的 errno 槽位指针，读取不产生副作用。
            unsafe { *libc::__errno() }
        );
    }
}
