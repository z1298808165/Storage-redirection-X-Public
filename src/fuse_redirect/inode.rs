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
        state.inodes.remove(&rel);
        state.inode_path_versions.remove(&ino);
    }
}

pub(super) fn remove_inode_path(state: &mut FuseState, rel: &str) {
    if let Some(ino) = state.inodes.remove(rel) {
        state.paths_by_inode.remove(&ino);
        state.inode_path_versions.remove(&ino);
        state.lookup_counts.remove(&ino);
        state.dir_entry_refs.remove(&ino);
    }
}

pub(super) fn remap_inode_path(state: &mut FuseState, old_rel: &str, new_rel: &str) {
    if old_rel == new_rel {
        return;
    }
    remove_inode_path(state, new_rel);
    if let Some(ino) = state.inodes.remove(old_rel) {
        state.inodes.insert(new_rel.to_string(), ino);
        state.paths_by_inode.insert(ino, new_rel.to_string());
        let version = state.inode_path_versions.entry(ino).or_default();
        // quality-allow(chinese-language): wrapping_add 是 Rust 整数回绕 API 名称。
        *version = version.wrapping_add(1);
    }
}
