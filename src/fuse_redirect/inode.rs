//! FUSE 重定向后端的 inode 记账辅助函数。
//!
//! 这里集中维护目录项引用计数、inode 与相对路径的双向映射，以及路径改名时的
//! 版本号回绕，避免 `mod.rs` 同时承载协议实现和状态记账细节。

use super::{DirEntry, FuseState, ROOT_INO};

pub(super) fn add_dir_entry_refs(state: &mut FuseState, entries: &[DirEntry]) {
    for entry in entries {
        if entry.ino.0 == ROOT_INO {
            continue;
        }
        let count = state.dir_entry_refs.entry(entry.ino.0).or_default();
        // quality-allow(chinese-language): 本行是 Rust 解引用赋值，saturating_add 为标准库 API 名称。
        *count = count.saturating_add(1);
    }
}

pub(super) fn remove_dir_entry_refs(state: &mut FuseState, entries: &[DirEntry]) {
    for entry in entries {
        if entry.ino.0 == ROOT_INO {
            continue;
        }
        if let Some(count) = state.dir_entry_refs.get_mut(&entry.ino.0) {
            // quality-allow(chinese-language): 本行是 Rust 解引用赋值，saturating_sub 为标准库 API 名称。
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.dir_entry_refs.remove(&entry.ino.0);
            }
        }
        remove_unreferenced_inode(state, entry.ino.0);
    }
}

pub(super) fn remove_unreferenced_inode(state: &mut FuseState, ino: u64) {
    if ino == ROOT_INO
        || state.lookup_counts.contains_key(&ino)
        || state.dir_entry_refs.contains_key(&ino)
    {
        return;
    }
    if let Some(rel) = state.paths_by_inode.remove(&ino) {
        let key = state.inode_key(&rel);
        state.inodes.remove(&key);
        state.inode_path_versions.remove(&ino);
        state.inode_policy_uids.remove(&ino);
    }
}

pub(super) fn remove_inode_path(state: &mut FuseState, rel: &str) {
    // 路径已被删除或被 rename 覆盖：按路径记的属主同步失效，否则同名新文件会继承旧属主。
    state.path_policy_uids.remove(rel);
    let key = state.inode_key(rel);
    if let Some(ino) = state.inodes.remove(&key) {
        state.paths_by_inode.remove(&ino);
        state.inode_path_versions.remove(&ino);
        state.inode_policy_uids.remove(&ino);
        state.lookup_counts.remove(&ino);
        state.dir_entry_refs.remove(&ino);
    }
}

pub(super) fn remap_inode_path(state: &mut FuseState, old_rel: &str, new_rel: &str) {
    if old_rel == new_rel {
        return;
    }
    // casefold 后端上仅改大小写的 rename（`a` → `A`）是同一个文件：键不变，只更新
    // 展示路径；若按「目标已存在」先移除，会把正在改名的 inode 自己删掉。
    if state.inode_key(old_rel) == state.inode_key(new_rel) {
        let key = state.inode_key(old_rel);
        if let Some(&ino) = state.inodes.get(&key) {
            state.paths_by_inode.insert(ino, new_rel.to_string());
            let version = state.inode_path_versions.entry(ino).or_default();
            // quality-allow(chinese-language): wrapping_add 是 Rust 整数回绕 API 名称。
            *version = version.wrapping_add(1);
        }
        if let Some(uid) = state.path_policy_uids.remove(old_rel) {
            state.path_policy_uids.insert(new_rel.to_string(), uid);
        }
        remap_descendant_paths(state, old_rel, new_rel);
        return;
    }
    remove_inode_path(state, new_rel);
    if let Some(uid) = state.path_policy_uids.remove(old_rel) {
        state.path_policy_uids.insert(new_rel.to_string(), uid);
    }
    let old_key = state.inode_key(old_rel);
    if let Some(ino) = state.inodes.remove(&old_key) {
        let new_key = state.inode_key(new_rel);
        state.inodes.insert(new_key, ino);
        state.paths_by_inode.insert(ino, new_rel.to_string());
        let version = state.inode_path_versions.entry(ino).or_default();
        // quality-allow(chinese-language): wrapping_add 是 Rust 整数回绕 API 名称。
        *version = version.wrapping_add(1);
    }
    remap_descendant_paths(state, old_rel, new_rel);
}

/// 目录 rename 后把旧前缀下的全部子孙 inode 迁到新前缀。
///
/// 只改目录自身会让子孙仍挂在旧路径上：内核对已缓存的子 dentry 继续用旧 nodeid
/// 发请求，daemon 按旧 rel 去后端找，得到 ENOENT（真机：`mv dir dir2` 后
/// `cat dir2/f` 报 No such file，后端文件完好）。子孙的路径版本一并递增，
/// 让按 inode 缓存的后端解析失效重建。
fn remap_descendant_paths(state: &mut FuseState, old_rel: &str, new_rel: &str) {
    let old_prefix = format!("{}/", state.inode_key(old_rel));
    let moved: Vec<(String, u64)> = state
        .inodes
        .iter()
        .filter(|(key, _)| key.starts_with(&old_prefix))
        .map(|(key, ino)| (key.clone(), *ino))
        .collect();
    for (old_key, ino) in moved {
        state.inodes.remove(&old_key);
        let Some(old_path) = state.paths_by_inode.get(&ino).cloned() else {
            continue;
        };
        // 展示路径保留子孙自身的大小写，只替换被改名的前缀段。
        let suffix = &old_path[old_rel.len().min(old_path.len())..];
        let new_path = format!("{new_rel}{suffix}");
        let new_key = state.inode_key(&new_path);
        state.inodes.insert(new_key, ino);
        if let Some(uid) = state.path_policy_uids.remove(&old_path) {
            state.path_policy_uids.insert(new_path.clone(), uid);
        }
        state.paths_by_inode.insert(ino, new_path);
        let version = state.inode_path_versions.entry(ino).or_default();
        // quality-allow(chinese-language): wrapping_add 是 Rust 整数回绕 API 名称。
        *version = version.wrapping_add(1);
    }
}
