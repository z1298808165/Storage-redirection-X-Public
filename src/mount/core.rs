use super::MountPlanner;
use crate::platform::{fs, module_paths, paths};
use libc::{
    CLONE_NEWNS, MNT_DETACH, MS_BIND, MS_PRIVATE, MS_RDONLY, MS_REC, MS_REMOUNT, chmod, chown,
    mount, readlink, stat as c_stat, umount2, unshare,
};
use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};

const MEDIA_RW_UID: u32 = 1023;
const MEDIA_RW_GID: u32 = 1023;
const MAPPED_DIR_MODE: libc::mode_t = 0o2773;
const READ_ONLY_DIR_MODE: libc::mode_t = 0o555;
const REAL_PUBLIC_DIR_MODE: libc::mode_t = 0o2771;
const ALLOWED_REAL_DIR_MODE: libc::mode_t = MAPPED_DIR_MODE;
const ALLOWED_REAL_TREE_METADATA_LIMIT: usize = 256;
const READ_ONLY_TREE_METADATA_LIMIT: usize = 512;
const BIND_SUCCESS_LOG_STEP: u64 = 128;
const BIND_VERIFY_PASS_LOG_STEP: u64 = 256;
static BIND_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);
static BIND_VERIFY_PASS_COUNT: AtomicU64 = AtomicU64::new(0);

#[inline]
fn should_log_step(count: u64, step: u64) -> bool {
    count == 1 || count.is_multiple_of(step)
}

impl MountPlanner {
    // 按需 unshare 并设置 /storage 为 MS_PRIVATE
    pub(super) fn ensure_mount_namespace_prepared(&mut self) -> bool {
        if self.is_namespace_ready {
            return true;
        }

        if self.should_unshare {
            let ret = unsafe { unshare(CLONE_NEWNS) };
            if ret != 0 {
                log::error!("mount ns: unshare failed errno={}", last_errno());
                return false;
            }
        }

        let Ok(storage) = CString::new("/storage") else {
            return false;
        };
        let ret = unsafe {
            mount(
                std::ptr::null(),
                storage.as_ptr(),
                std::ptr::null(),
                (MS_REC | MS_PRIVATE) as libc::c_ulong,
                std::ptr::null(),
            )
        };
        if ret != 0 {
            log::warn!(
                "mount ns: /storage MS_PRIVATE failed errno={}",
                last_errno()
            );
        }

        let mut buf = [0u8; 256];
        let Ok(ns_path) = CString::new("/proc/self/ns/mnt") else {
            return false;
        };
        let len = unsafe { readlink(ns_path.as_ptr(), buf.as_mut_ptr() as *mut _, buf.len() - 1) };
        if len > 0 {
            let text = String::from_utf8_lossy(&buf[..len as usize]);
            log::info!("mount ns ready ns={}", text);
        } else {
            log::warn!("mount ns: readlink failed errno={}", last_errno());
        }

        self.is_namespace_ready = true;
        true
    }

    pub(super) fn ensure_directory_exists(&self, path: &str, should_chown: bool) -> bool {
        if path.is_empty() {
            log::warn!("mount dir: mkdir skipped empty path");
            return false;
        }
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            log::debug!("mount dir: skip public storage root mkdir path={}", path);
            return true;
        };
        if !fs::create_directory(&metadata_path, -1) {
            log::warn!(
                "mount dir: mkdir failed path={} metadata_path={}",
                path,
                metadata_path
            );
            return false;
        }

        if should_chown {
            self.ensure_writable_directory_chain(&metadata_path, self.app_uid);
        }

        true
    }

    // 创建或修正映射目录的所有者，使应用可写入
    pub(super) fn ensure_writable_mapped_directory(&self, path: &str, owner_uid: i32) -> bool {
        if path.is_empty() {
            log::warn!("mount dir: metadata skipped empty path");
            return false;
        }
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            log::debug!("mount dir: skip public storage root metadata path={}", path);
            return true;
        };
        log::debug!(
            "mount dir prep path={} metadata_path={} owner={}",
            path,
            metadata_path,
            owner_uid
        );
        let is_existing = fs::is_directory(&metadata_path);
        if !is_existing && !fs::create_directory(&metadata_path, owner_uid) {
            log::warn!(
                "mount dir: missing and mkdir failed path={} metadata_path={}",
                path,
                metadata_path
            );
            return false;
        }

        let Ok(c_path) = CString::new(metadata_path.as_str()) else {
            return false;
        };

        // 无论目录是否已存在，都确保所有者正确
        if owner_uid >= 0 {
            let ret = unsafe { chown(c_path.as_ptr(), owner_uid as u32, MEDIA_RW_GID) };
            if ret != 0 {
                log::warn!(
                    "mount dir: chown failed errno={} {} path={} metadata_path={}",
                    last_errno(),
                    errno_text(),
                    path,
                    metadata_path
                );
            }
        }

        // 无论目录是否已存在，都确保权限正确
        let ret = unsafe { chmod(c_path.as_ptr(), MAPPED_DIR_MODE) };
        if ret != 0 {
            log::warn!(
                "mount dir: chmod failed errno={} {} path={} metadata_path={}",
                last_errno(),
                errno_text(),
                path,
                metadata_path
            );
        }

        true
    }

    pub(super) fn ensure_read_only_directory_metadata(&self, path: &str) -> bool {
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            log::debug!("mount dir: skip public storage root readonly path={}", path);
            return true;
        };
        let Ok(c_path) = CString::new(metadata_path.as_str()) else {
            return false;
        };
        let ret = unsafe { chmod(c_path.as_ptr(), READ_ONLY_DIR_MODE) };
        if ret != 0 {
            log::warn!(
                "mount dir: readonly chmod failed errno={} {} path={} metadata_path={}",
                last_errno(),
                errno_text(),
                path,
                metadata_path
            );
            return false;
        }
        log::debug!(
            "mount dir readonly path={} metadata_path={}",
            path,
            metadata_path
        );
        true
    }

    pub(super) fn ensure_read_only_tree_accessible(&self, path: &str) {
        let Some(root) = self.metadata_operations_path(path) else {
            return;
        };
        if !is_data_media_shared_public_directory(&root, self.user_id) || !fs::is_directory(&root) {
            return;
        }

        let mut visited = 0usize;
        let mut pending = VecDeque::from([root.clone()]);
        while let Some(current) = pending.pop_front() {
            if visited >= READ_ONLY_TREE_METADATA_LIMIT {
                log::warn!(
                    "mount dir: readonly metadata scan limit reached root={} limit={}",
                    path,
                    READ_ONLY_TREE_METADATA_LIMIT
                );
                break;
            }
            visited += 1;

            fix_read_only_public_metadata(&current);

            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let child = entry.path().to_string_lossy().replace('\\', "/");
                if child.is_empty() {
                    continue;
                }
                if entry.file_type().map(|ty| ty.is_dir()).unwrap_or(false) {
                    pending.push_back(child);
                } else {
                    fix_read_only_public_metadata(&child);
                }
            }
        }
    }

    pub(super) fn ensure_app_writable_directory_chain(&self, path: &str, owner_uid: i32) {
        self.ensure_writable_directory_chain(path, owner_uid);
    }

    pub(super) fn ensure_real_public_directory_exists(&self, path: &str, owner_uid: i32) -> bool {
        if path.is_empty() {
            log::warn!("mount dir: real public mkdir skipped empty path");
            return false;
        }
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            log::debug!("mount dir: skip public storage root real path={}", path);
            return true;
        };
        let should_fix_public_metadata =
            is_data_media_shared_public_directory(&metadata_path, self.user_id);
        let uid = if should_fix_public_metadata {
            MEDIA_RW_UID as i32
        } else {
            -1
        };
        if !fs::create_directory(&metadata_path, uid) {
            log::warn!(
                "mount dir: real public mkdir failed path={} metadata_path={}",
                path,
                metadata_path
            );
            return false;
        }

        if should_fix_public_metadata {
            self.fix_real_public_directory_metadata_chain(&metadata_path);
            self.fix_allowed_real_directory_metadata_chain(&metadata_path, owner_uid);
        }

        true
    }

    pub(super) fn ensure_allowed_real_existing_directory_tree_writable(
        &self,
        path: &str,
        owner_uid: i32,
        excluded_real_paths: &[String],
    ) {
        if owner_uid < 0 {
            return;
        }
        let Some(root) = self.metadata_operations_path(path) else {
            return;
        };
        if !is_data_media_shared_public_directory(&root, self.user_id) || !fs::is_directory(&root) {
            return;
        }

        let mut visited = 0usize;
        let mut pending = VecDeque::from([root]);
        while let Some(current) = pending.pop_front() {
            if visited >= ALLOWED_REAL_TREE_METADATA_LIMIT {
                log::warn!(
                    "mount dir: allowed real metadata scan limit reached root={} limit={}",
                    path,
                    ALLOWED_REAL_TREE_METADATA_LIMIT
                );
                break;
            }
            visited += 1;

            if allowed_real_metadata_path_is_excluded(&current, excluded_real_paths) {
                continue;
            }
            if fs::is_directory(&current) {
                fix_allowed_real_directory_metadata(&current, owner_uid);
            }

            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
                let child = entry.path().to_string_lossy().replace('\\', "/");
                if !child.is_empty() {
                    pending.push_back(child);
                }
            }
        }
    }

    fn fix_allowed_real_directory_metadata_chain(&self, path: &str, owner_uid: i32) {
        if owner_uid < 0 {
            return;
        }
        let root = paths::data_media_user_root_for_user(self.user_id);
        let original = path.to_string();
        let mut current = original.clone();
        while !current.is_empty() {
            if current == root {
                break;
            }

            if should_apply_allowed_real_writable_metadata(&current, &original, self.user_id)
                && fs::is_directory(&current)
            {
                fix_allowed_real_directory_metadata(&current, owner_uid);
            }

            let parent = parent_preserving_backend_alias(&current);
            if parent.is_empty() || parent == "/" || parent == current {
                break;
            }
            current = parent;
        }
    }

    fn fix_real_public_directory_metadata_chain(&self, path: &str) {
        let mut current = path.to_string();
        while !current.is_empty() {
            if is_data_media_shared_public_directory(&current, self.user_id)
                && fs::is_directory(&current)
            {
                fix_real_public_directory_metadata(&current);
            }

            let parent = parent_preserving_backend_alias(&current);
            if parent.is_empty() || parent == "/" || parent == current {
                break;
            }
            current = parent;
        }
    }

    pub(super) fn ensure_shared_mapping_parent_chain(&self, path: &str) {
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            return;
        };
        let Some(stop_dir) = shared_storage_root(&metadata_path) else {
            return;
        };

        let mut current = parent_preserving_backend_alias(&metadata_path);
        while !current.is_empty() {
            if current == stop_dir {
                break;
            }
            if is_android_private_storage_path(&current, &stop_dir) {
                break;
            }

            if fs::is_directory(&current) {
                self.ensure_shared_mapping_directory(&current);
            }

            let parent = parent_preserving_backend_alias(&current);
            if parent.is_empty() || parent == "/" || parent == current {
                break;
            }
            current = parent;
        }
    }

    fn ensure_shared_mapping_directory(&self, path: &str) {
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            return;
        };
        let Ok(c_path) = CString::new(metadata_path.as_str()) else {
            return;
        };

        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        let ret = unsafe { c_stat(c_path.as_ptr(), st.as_mut_ptr()) };
        let existing_stat = if ret == 0 {
            Some(unsafe { st.assume_init() })
        } else {
            log::warn!(
                "mount dir: shared parent stat failed errno={} {} path={} metadata_path={}",
                last_errno(),
                errno_text(),
                path,
                metadata_path
            );
            None
        };

        if let Some(st) = existing_stat {
            let ret = unsafe { chown(c_path.as_ptr(), st.st_uid, MEDIA_RW_GID) };
            if ret != 0 {
                log::warn!(
                    "mount dir: shared parent chgrp failed errno={} {} path={} metadata_path={}",
                    last_errno(),
                    errno_text(),
                    path,
                    metadata_path
                );
            }

            let mode = (st.st_mode as libc::mode_t) & 0o7777;
            let fixed_mode = mode | 0o2010;
            if fixed_mode != mode {
                let ret = unsafe { chmod(c_path.as_ptr(), fixed_mode) };
                if ret != 0 {
                    log::warn!(
                        "mount dir: shared parent chmod failed errno={} {} path={} metadata_path={}",
                        last_errno(),
                        errno_text(),
                        path,
                        metadata_path
                    );
                }
            }
        }
    }

    fn ensure_writable_directory_chain(&self, path: &str, owner_uid: i32) {
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            return;
        };
        let stop_dir = paths::storage_user_root(&metadata_path)
            .or_else(|| paths::data_media_user_root(&metadata_path));
        let mut current = metadata_path;
        while !current.is_empty() {
            if let Some(stop) = stop_dir.as_deref()
                && current == stop
            {
                break;
            }

            if fs::is_directory(&current)
                && should_apply_app_writable_metadata(&current, self.user_id)
            {
                let _ = self.ensure_writable_mapped_directory(&current, owner_uid);
            }

            let parent = parent_preserving_backend_alias(&current);
            if parent.is_empty() || parent == "/" || parent == current {
                break;
            }
            current = parent;
        }
    }

    pub(super) fn to_data_media_backend_path(&self, storage_path: &str) -> String {
        paths::storage_to_data_media_for_user(storage_path, self.user_id).unwrap_or_default()
    }

    pub(super) fn normalize_path(&self, path: &str) -> String {
        paths::normalize(path)
    }

    pub(super) fn resolve_user_path(&self, path: &str) -> String {
        paths::resolve_user_path(path, self.user_id)
    }

    pub(super) fn resolve_placeholders(&self, path: &str) -> String {
        let has_redirect_placeholder =
            path.contains("${REDIRECT_TARGET}") || path.contains("$REDIRECT_TARGET");
        if has_redirect_placeholder && self.redirect_target.is_empty() {
            log::warn!(
                "mount path: redirect target missing, cannot expand placeholder path={}",
                path
            );
        }

        paths::resolve_placeholders(path, &self.app_data_dir, &self.redirect_target)
    }

    pub(super) fn bind_mount(&self, source: &str, target: &str, is_recursive: bool) -> bool {
        self.bind_mount_inner(source, target, is_recursive, true)
    }

    pub(super) fn bind_mount_overlay(
        &self,
        source: &str,
        target: &str,
        is_recursive: bool,
    ) -> bool {
        self.bind_mount_inner(source, target, is_recursive, false)
    }

    pub(super) fn bind_mount_read_write_overlay(
        &self,
        source: &str,
        target: &str,
        is_recursive: bool,
    ) -> bool {
        let use_recursive = self.should_use_recursive_bind(source, target, is_recursive);
        if !self.bind_mount_overlay(source, target, use_recursive) {
            return false;
        }

        if self.remount_bind_read_write(target, use_recursive) {
            log::info!("readwrite bind ok src={} dst={}", source, target);
            return true;
        }

        let Ok(c_target) = CString::new(target) else {
            return false;
        };
        let ret = unsafe { umount2(c_target.as_ptr(), MNT_DETACH) };
        if ret != 0 {
            log::warn!(
                "readwrite bind cleanup failed dst={} errno={} {}",
                target,
                last_errno(),
                errno_text()
            );
        }
        false
    }

    fn bind_mount_inner(
        &self,
        source: &str,
        target: &str,
        is_recursive: bool,
        allow_same_inode_shortcut: bool,
    ) -> bool {
        let use_recursive = self.should_use_recursive_bind(source, target, is_recursive);
        if allow_same_inode_shortcut && paths_have_same_inode(source, target) {
            if self.remount_bind_read_write(target, use_recursive) {
                self.record_mounted_target(target);
                log::debug!("bind skip existing src={} dst={}", source, target);
                return true;
            }
            log::warn!(
                "bind existing remount rw failed, retry bind src={} dst={}",
                source,
                target
            );
        }

        let Ok(c_source) = CString::new(source) else {
            return false;
        };
        let Ok(c_target) = CString::new(target) else {
            return false;
        };

        let mut flags = MS_BIND;
        if use_recursive {
            flags |= MS_REC;
        }

        let mut ret = unsafe {
            mount(
                c_source.as_ptr(),
                c_target.as_ptr(),
                std::ptr::null(),
                flags as libc::c_ulong,
                std::ptr::null(),
            )
        };
        if ret != 0 {
            let errno = last_errno();
            if errno == libc::ENOTCONN && self.repair_disconnected_storage_parent(target) {
                ret = unsafe {
                    mount(
                        c_source.as_ptr(),
                        c_target.as_ptr(),
                        std::ptr::null(),
                        flags as libc::c_ulong,
                        std::ptr::null(),
                    )
                };
                if ret == 0 {
                    log::warn!(
                        "bind retry ok after storage parent repair src={} dst={}",
                        source,
                        target
                    );
                }
            }
        }
        if ret != 0 {
            log::error!(
                "bind mount failed src={} dst={} errno={} {}",
                source,
                target,
                last_errno(),
                errno_text()
            );
            return false;
        }

        self.record_mounted_target(target);

        let bind_count = BIND_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if should_log_step(bind_count, BIND_SUCCESS_LOG_STEP) {
            log::debug!("bind ok src={} dst={} n={}", source, target, bind_count);
        }
        if self.is_public_storage_alias_path(source)
            || self.is_public_storage_alias_path(target)
            || self.is_real_storage_anchor_path(source)
            || self.is_real_storage_anchor_path(target)
        {
            log::debug!(
                "bind verify skipped fuse-backed path src={} dst={}",
                source,
                target
            );
            return true;
        }
        let Ok(c_source_stat) = CString::new(source) else {
            return true;
        };
        let Ok(c_target_stat) = CString::new(target) else {
            return true;
        };
        let mut st_source = std::mem::MaybeUninit::<c_stat>::uninit();
        let mut st_target = std::mem::MaybeUninit::<c_stat>::uninit();
        let is_source_ok =
            unsafe { libc::stat(c_source_stat.as_ptr(), st_source.as_mut_ptr()) } == 0;
        let is_target_ok =
            unsafe { libc::stat(c_target_stat.as_ptr(), st_target.as_mut_ptr()) } == 0;
        if is_source_ok && is_target_ok {
            let st_source = unsafe { st_source.assume_init() };
            let st_target = unsafe { st_target.assume_init() };
            let is_same =
                st_source.st_dev == st_target.st_dev && st_source.st_ino == st_target.st_ino;
            if is_same {
                let verify_ok_count = BIND_VERIFY_PASS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(verify_ok_count, BIND_VERIFY_PASS_LOG_STEP) {
                    log::debug!(
                        "bind verify ok src={} dst={} n={}",
                        source,
                        target,
                        verify_ok_count
                    );
                }
            } else {
                log::warn!(
                    "bind verify mismatch src={} dst={} sdev={} tdev={} sino={} tino={}",
                    source,
                    target,
                    st_source.st_dev,
                    st_target.st_dev,
                    st_source.st_ino,
                    st_target.st_ino
                );
            }
        } else {
            log::warn!(
                "bind verify failed src={} dst={} reason=stat",
                source,
                target
            );
        }
        true
    }

    fn should_use_recursive_bind(&self, source: &str, target: &str, requested: bool) -> bool {
        if !requested {
            return false;
        }
        if self.is_public_storage_alias_path(source)
            || self.is_public_storage_alias_path(target)
            || self.is_real_storage_anchor_path(source)
            || self.is_real_storage_anchor_path(target)
            || paths::data_media_user_root(source).is_some()
            || paths::data_media_user_root(target).is_some()
        {
            log::debug!(
                "bind recursive disabled for storage tree src={} dst={}",
                source,
                target
            );
            return false;
        }
        true
    }

    fn repair_disconnected_storage_parent(&self, target: &str) -> bool {
        let Some((source, mount_point)) = storage_emulated_repair_mount(target, self.user_id)
        else {
            return false;
        };
        if !fs::is_directory(&source) {
            log::warn!(
                "storage parent repair skipped source missing src={} dst={} target={}",
                source,
                mount_point,
                target
            );
            return false;
        }

        let Ok(c_source) = CString::new(source.as_str()) else {
            return false;
        };
        let Ok(c_mount_point) = CString::new(mount_point.as_str()) else {
            return false;
        };

        let _ = unsafe { umount2(c_mount_point.as_ptr(), MNT_DETACH) };
        let ret = unsafe {
            mount(
                c_source.as_ptr(),
                c_mount_point.as_ptr(),
                std::ptr::null(),
                MS_BIND as libc::c_ulong,
                std::ptr::null(),
            )
        };
        if ret != 0 {
            log::warn!(
                "storage parent repair failed src={} dst={} target={} errno={} {}",
                source,
                mount_point,
                target,
                last_errno(),
                errno_text()
            );
            return false;
        }

        self.record_mounted_target(&mount_point);
        log::warn!(
            "storage parent repaired src={} dst={} target={}",
            source,
            mount_point,
            target
        );
        true
    }

    pub(super) fn bind_mount_read_only(
        &self,
        source: &str,
        target: &str,
        is_recursive: bool,
    ) -> bool {
        let use_recursive = self.should_use_recursive_bind(source, target, is_recursive);
        if !self.bind_mount(source, target, use_recursive) {
            return false;
        }

        if self.remount_bind_read_only(target, use_recursive) {
            log::info!("readonly bind ok src={} dst={}", source, target);
            return true;
        }

        let Ok(c_target) = CString::new(target) else {
            return false;
        };
        let ret = unsafe { umount2(c_target.as_ptr(), MNT_DETACH) };
        if ret != 0 {
            log::warn!(
                "readonly bind cleanup failed dst={} errno={} {}",
                target,
                last_errno(),
                errno_text()
            );
        }
        false
    }

    fn remount_bind_read_only(&self, target: &str, is_recursive: bool) -> bool {
        if remount_bind_read_only_inner(target, is_recursive) {
            return true;
        }
        if is_recursive && remount_bind_read_only_inner(target, false) {
            log::warn!("readonly remount recursive fallback ok dst={}", target);
            return true;
        }
        false
    }

    fn remount_bind_read_write(&self, target: &str, is_recursive: bool) -> bool {
        if remount_bind_read_write_inner(target, is_recursive) {
            return true;
        }
        if is_recursive && remount_bind_read_write_inner(target, false) {
            log::warn!("readwrite remount recursive fallback ok dst={}", target);
            return true;
        }
        false
    }

    fn record_mounted_target(&self, target: &str) {
        if target.is_empty() {
            return;
        }
        let mut targets = self.mounted_targets.borrow_mut();
        if !targets.iter().any(|item| item == target) {
            targets.push(target.to_string());
        }
    }

    fn metadata_operations_path(&self, path: &str) -> Option<String> {
        if path.is_empty() {
            return None;
        }
        if let Some(backend_path) = self.real_storage_anchor_backend_path(path) {
            return Some(backend_path);
        }
        if self.is_storage_root_redirected && self.is_public_storage_alias_path(path) {
            return Some(path.to_string());
        }
        if let Some(backend_path) = paths::storage_to_data_media_for_user(path, self.user_id) {
            return Some(backend_path);
        }
        if self.is_user_storage_alias_path(path) {
            return None;
        }
        Some(path.to_string())
    }

    fn is_user_storage_alias_path(&self, path: &str) -> bool {
        if path.is_empty() {
            return false;
        }
        let normalized = paths::normalize(path);
        paths::is_same_or_child(
            &normalized,
            &paths::storage_user_root_for_user(self.user_id),
        )
    }

    fn is_public_storage_alias_path(&self, path: &str) -> bool {
        if path.starts_with("/data/media/") || path == "/data/media" {
            return false;
        }
        self.is_user_storage_alias_path(path)
    }

    fn real_storage_anchor_backend_path(&self, path: &str) -> Option<String> {
        let anchor_root = self.real_storage_anchor_root();
        let relative = paths::relative_child_path(path, &anchor_root)?;
        if relative.is_empty() {
            return None;
        }
        Some(paths::join(
            &paths::data_media_user_root_for_user(self.user_id),
            relative,
        ))
    }

    fn is_real_storage_anchor_path(&self, path: &str) -> bool {
        paths::is_same_or_child(path, &self.real_storage_anchor_root())
    }

    pub(crate) fn real_storage_anchor(&self) -> Option<String> {
        self.real_storage_anchor.clone()
    }

    fn real_storage_anchor_root(&self) -> String {
        paths::join(
            module_paths::REAL_STORAGE_TMP_DIR,
            &self.user_id.to_string(),
        )
    }
}

fn shared_storage_root(path: &str) -> Option<String> {
    paths::storage_user_root(path).or_else(|| paths::data_media_user_root(path))
}

fn is_android_private_storage_path(path: &str, storage_root: &str) -> bool {
    let Some(relative) = paths::relative_child_path(path, storage_root) else {
        return false;
    };
    relative == "Android/data"
        || relative == "Android/media"
        || relative == "Android/obb"
        || relative.starts_with("Android/data/")
        || relative.starts_with("Android/media/")
        || relative.starts_with("Android/obb/")
}

fn should_apply_app_writable_metadata(path: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        if path.starts_with("/data/media/") || path == "/data/media" {
            return false;
        }
        return true;
    };

    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return true;
    }

    match parts.next() {
        Some("data" | "media" | "obb") => parts.next().is_some(),
        _ => true,
    }
}

fn is_data_media_shared_public_directory(path: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        return false;
    };
    !is_android_app_private_relative_path(relative)
}

fn is_android_app_private_relative_path(relative: &str) -> bool {
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return false;
    }
    match parts.next() {
        Some("data" | "media" | "obb") => parts.next().is_some(),
        _ => false,
    }
}

fn fix_real_public_directory_metadata(path: &str) {
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
    let ret = unsafe { c_stat(c_path.as_ptr(), st.as_mut_ptr()) };
    if ret != 0 {
        log::warn!(
            "mount dir: real public stat failed errno={} {} path={}",
            last_errno(),
            errno_text(),
            path
        );
        return;
    }
    let st = unsafe { st.assume_init() };

    if st.st_uid != MEDIA_RW_UID || st.st_gid != MEDIA_RW_GID {
        let ret = unsafe { chown(c_path.as_ptr(), MEDIA_RW_UID, MEDIA_RW_GID) };
        if ret != 0 {
            log::warn!(
                "mount dir: real public chown failed errno={} {} path={}",
                last_errno(),
                errno_text(),
                path
            );
        }
    }

    let mode = (st.st_mode as libc::mode_t) & 0o7777;
    if mode != REAL_PUBLIC_DIR_MODE {
        let ret = unsafe { chmod(c_path.as_ptr(), REAL_PUBLIC_DIR_MODE) };
        if ret != 0 {
            log::warn!(
                "mount dir: real public chmod failed errno={} {} path={}",
                last_errno(),
                errno_text(),
                path
            );
        }
    }
}

fn fix_allowed_real_directory_metadata(path: &str, owner_uid: i32) {
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    let ret = unsafe { chown(c_path.as_ptr(), owner_uid as u32, MEDIA_RW_GID) };
    if ret != 0 {
        log::warn!(
            "mount dir: allowed real chown failed errno={} {} path={}",
            last_errno(),
            errno_text(),
            path
        );
    }

    let ret = unsafe { chmod(c_path.as_ptr(), ALLOWED_REAL_DIR_MODE) };
    if ret != 0 {
        log::warn!(
            "mount dir: allowed real chmod failed errno={} {} path={}",
            last_errno(),
            errno_text(),
            path
        );
    }
}

fn fix_read_only_public_metadata(path: &str) {
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
    let ret = unsafe { c_stat(c_path.as_ptr(), st.as_mut_ptr()) };
    if ret != 0 {
        log::warn!(
            "mount dir: readonly public stat failed errno={} {} path={}",
            last_errno(),
            errno_text(),
            path
        );
        return;
    }
    let st = unsafe { st.assume_init() };
    let mode = (st.st_mode as libc::mode_t) & 0o7777;
    let required = if (st.st_mode & libc::S_IFMT) == libc::S_IFDIR {
        0o0555
    } else {
        0o0444
    };
    let fixed_mode = mode | required;
    if fixed_mode == mode {
        return;
    }

    let ret = unsafe { chmod(c_path.as_ptr(), fixed_mode) };
    if ret != 0 {
        log::warn!(
            "mount dir: readonly public chmod failed errno={} {} path={} mode={:o}",
            last_errno(),
            errno_text(),
            path,
            fixed_mode
        );
    }
}

fn should_apply_allowed_real_writable_metadata(path: &str, original: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        return false;
    };
    if relative.is_empty() || is_android_app_private_relative_path(relative) {
        return false;
    }
    path == original || relative.split('/').filter(|part| !part.is_empty()).count() >= 2
}

fn allowed_real_metadata_path_is_excluded(path: &str, excluded_real_paths: &[String]) -> bool {
    if excluded_real_paths.is_empty() {
        return false;
    }
    let storage_path = paths::data_media_to_storage_path(path);
    excluded_real_paths
        .iter()
        .any(|excluded| paths::matches(excluded, &storage_path, true))
}

fn parent_preserving_backend_alias(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return trimmed.to_string();
    }
    if let Some(pos) = trimmed.rfind('/') {
        if pos == 0 {
            "/".to_string()
        } else {
            trimmed[..pos].to_string()
        }
    } else {
        String::new()
    }
}

fn storage_emulated_repair_mount(target: &str, user_id: i32) -> Option<(String, String)> {
    let normalized = paths::normalize(target);
    let storage_root = paths::storage_user_root_for_user(user_id);
    if paths::eq_ignore_case(&normalized, &storage_root)
        || paths::is_child(&normalized, &storage_root)
    {
        Some(("/data/media".to_string(), "/storage/emulated".to_string()))
    } else {
        None
    }
}

fn remount_bind_read_only_inner(target: &str, is_recursive: bool) -> bool {
    let Ok(c_target) = CString::new(target) else {
        return false;
    };
    let mut flags = MS_BIND | MS_REMOUNT | MS_RDONLY;
    if is_recursive {
        flags |= MS_REC;
    }
    let ret = unsafe {
        mount(
            std::ptr::null(),
            c_target.as_ptr(),
            std::ptr::null(),
            flags as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret == 0 {
        return true;
    }
    log::warn!(
        "readonly remount failed dst={} recursive={} errno={} {}",
        target,
        is_recursive,
        last_errno(),
        errno_text()
    );
    false
}

fn remount_bind_read_write_inner(target: &str, is_recursive: bool) -> bool {
    let Ok(c_target) = CString::new(target) else {
        return false;
    };
    let mut flags = MS_BIND | MS_REMOUNT;
    if is_recursive {
        flags |= MS_REC;
    }
    let ret = unsafe {
        mount(
            std::ptr::null(),
            c_target.as_ptr(),
            std::ptr::null(),
            flags as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret == 0 {
        return true;
    }
    log::warn!(
        "readwrite remount failed dst={} recursive={} errno={} {}",
        target,
        is_recursive,
        last_errno(),
        errno_text()
    );
    false
}

fn paths_have_same_inode(left: &str, right: &str) -> bool {
    let Ok(c_left) = CString::new(left) else {
        return false;
    };
    let Ok(c_right) = CString::new(right) else {
        return false;
    };
    let mut st_left = std::mem::MaybeUninit::<c_stat>::uninit();
    let mut st_right = std::mem::MaybeUninit::<c_stat>::uninit();
    let left_ok = unsafe { libc::stat(c_left.as_ptr(), st_left.as_mut_ptr()) } == 0;
    let right_ok = unsafe { libc::stat(c_right.as_ptr(), st_right.as_mut_ptr()) } == 0;
    if !left_ok || !right_ok {
        return false;
    }
    let st_left = unsafe { st_left.assume_init() };
    let st_right = unsafe { st_right.assume_init() };
    st_left.st_dev == st_right.st_dev && st_left.st_ino == st_right.st_ino
}

fn last_errno() -> i32 {
    unsafe { *libc::__errno() }
}

fn errno_text() -> String {
    let code = last_errno();
    unsafe {
        CStr::from_ptr(libc::strerror(code))
            .to_string_lossy()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_preserves_data_media_backend_alias() {
        assert_eq!(
            parent_preserving_backend_alias("/data/media/0/Download/第三方下载/Nnngram"),
            "/data/media/0/Download/第三方下载"
        );
        assert_eq!(
            parent_preserving_backend_alias("/storage/emulated/0/Download/Nnngram"),
            "/storage/emulated/0/Download"
        );
    }

    #[test]
    fn storage_repair_mount_targets_emulated_user_roots() {
        assert_eq!(
            storage_emulated_repair_mount("/storage/emulated/0", 0),
            Some(("/data/media".to_string(), "/storage/emulated".to_string()))
        );
        assert_eq!(
            storage_emulated_repair_mount("/storage/emulated/10/Download", 10),
            Some(("/data/media".to_string(), "/storage/emulated".to_string()))
        );
        assert_eq!(
            storage_emulated_repair_mount("/mnt/user/0/emulated/0", 0),
            None
        );
    }

    #[test]
    fn recursive_bind_is_disabled_for_storage_trees() {
        let planner = MountPlanner::new(
            "com.example.app",
            10123,
            "/data/user/0/com.example.app",
            "/storage/emulated/0/Android/data/com.example.app/sdcard",
            false,
        );

        assert!(!planner.should_use_recursive_bind(
            "/data/media/0",
            "/data/adb/modules/storage.redirect.x/tmp/real_storage/0",
            true
        ));
        assert!(!planner.should_use_recursive_bind(
            "/storage/emulated/0/Download",
            "/storage/emulated/0/Android/data/com.example.app/sdcard/Download",
            true
        ));
        assert!(!planner.should_use_recursive_bind(
            "/data/source",
            "/storage/emulated/0/Download",
            true
        ));
    }

    #[test]
    fn recursive_bind_is_kept_for_non_storage_paths() {
        let planner = MountPlanner::new(
            "com.example.app",
            10123,
            "/data/user/0/com.example.app",
            "/storage/emulated/0/Android/data/com.example.app/sdcard",
            false,
        );

        assert!(planner.should_use_recursive_bind(
            "/data/local/tmp/source",
            "/data/local/tmp/target",
            true
        ));
        assert!(!planner.should_use_recursive_bind(
            "/data/local/tmp/source",
            "/data/local/tmp/target",
            false
        ));
    }

    #[test]
    fn metadata_operations_use_data_media_for_public_storage_aliases() {
        let mut planner = MountPlanner::new(
            "com.example.app",
            10123,
            "/data/user/0/com.example.app",
            "/storage/emulated/0/Android/data/com.example.app/sdcard",
            false,
        );

        assert_eq!(
            planner
                .metadata_operations_path("/storage/emulated/0/DCIM/Camera")
                .as_deref(),
            Some("/data/media/0/DCIM/Camera")
        );
        assert_eq!(
            planner
                .metadata_operations_path("/sdcard/Download")
                .as_deref(),
            Some("/data/media/0/Download")
        );
        assert_eq!(
            planner
                .metadata_operations_path("/data/media/0/Pictures")
                .as_deref(),
            Some("/data/media/0/Pictures")
        );
        assert_eq!(
            planner.metadata_operations_path("/storage/emulated/0"),
            None
        );
        assert_eq!(
            planner
                .metadata_operations_path("/data/adb/modules/storage.redirect.x/tmp")
                .as_deref(),
            Some("/data/adb/modules/storage.redirect.x/tmp")
        );

        let anchor_path = paths::join(
            &paths::join(module_paths::REAL_STORAGE_TMP_DIR, "0"),
            "DCIM/Camera",
        );
        assert_eq!(
            planner.metadata_operations_path(&anchor_path).as_deref(),
            Some("/data/media/0/DCIM/Camera")
        );

        planner.is_storage_root_redirected = true;
        assert_eq!(
            planner
                .metadata_operations_path("/storage/emulated/0/DCIM/Camera")
                .as_deref(),
            Some("/storage/emulated/0/DCIM/Camera")
        );
    }

    #[test]
    fn real_public_directory_metadata_skips_android_app_private_dirs() {
        assert!(is_data_media_shared_public_directory(
            "/data/media/0/Pictures",
            0
        ));
        assert!(is_data_media_shared_public_directory(
            "/data/media/0/Pictures/CoolMarket",
            0
        ));
        assert!(is_data_media_shared_public_directory(
            "/data/media/0/Android",
            0
        ));
        assert!(is_data_media_shared_public_directory(
            "/data/media/0/Android/data",
            0
        ));
        assert!(!is_data_media_shared_public_directory(
            "/data/media/0/Android/data/com.example.app",
            0
        ));
        assert!(!is_data_media_shared_public_directory(
            "/data/media/0/Android/media/com.example.app/files",
            0
        ));
        assert!(!is_data_media_shared_public_directory(
            "/data/media/10/Pictures",
            0
        ));
    }

    #[test]
    fn app_writable_metadata_starts_at_android_private_package_root() {
        assert!(should_apply_app_writable_metadata(
            "/data/media/0/Download/SrtProbe",
            0
        ));
        assert!(!should_apply_app_writable_metadata(
            "/data/media/0/Android",
            0
        ));
        assert!(!should_apply_app_writable_metadata(
            "/data/media/0/Android/data",
            0
        ));
        assert!(should_apply_app_writable_metadata(
            "/data/media/0/Android/data/com.example.app",
            0
        ));
        assert!(should_apply_app_writable_metadata(
            "/data/media/0/Android/data/com.example.app/sdcard/Download",
            0
        ));
        assert!(!should_apply_app_writable_metadata(
            "/data/media/10/Android/data/com.example.app",
            0
        ));
    }

    #[test]
    fn allowed_real_metadata_skips_storage_root_and_top_level_dirs() {
        assert!(should_apply_allowed_real_writable_metadata(
            "/data/media/0/Download/SrtMountNsAllow/TeamAlpha/Deep",
            "/data/media/0/Download/SrtMountNsAllow/TeamAlpha/Deep",
            0
        ));
        assert!(should_apply_allowed_real_writable_metadata(
            "/data/media/0/Download/SrtMountNsAllow/TeamAlpha",
            "/data/media/0/Download/SrtMountNsAllow/TeamAlpha/Deep",
            0
        ));
        assert!(!should_apply_allowed_real_writable_metadata(
            "/data/media/0/Download",
            "/data/media/0/Download/SrtMountNsAllow/TeamAlpha/Deep",
            0
        ));
        assert!(!should_apply_allowed_real_writable_metadata(
            "/data/media/0",
            "/data/media/0/Download/SrtMountNsAllow/TeamAlpha/Deep",
            0
        ));
        assert!(!should_apply_allowed_real_writable_metadata(
            "/data/media/0/Android/data/com.example.app",
            "/data/media/0/Android/data/com.example.app",
            0
        ));
    }

    #[test]
    fn allowed_real_tree_metadata_respects_excluded_rules() {
        let excluded = vec![
            "/storage/emulated/0/Download/SrtAllow/tmp".to_string(),
            "/storage/emulated/0/Download/*.part".to_string(),
        ];

        assert!(allowed_real_metadata_path_is_excluded(
            "/data/media/0/Download/SrtAllow/tmp",
            &excluded,
        ));
        assert!(allowed_real_metadata_path_is_excluded(
            "/data/media/0/Download/SrtAllow/tmp/nested",
            &excluded,
        ));
        assert!(!allowed_real_metadata_path_is_excluded(
            "/data/media/0/Download/SrtProbe",
            &excluded,
        ));
    }
}
