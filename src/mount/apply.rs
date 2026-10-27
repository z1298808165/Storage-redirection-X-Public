use super::apply_targets::{
    build_allowed_real_source_candidates, build_mapping_source_roots, build_read_only_source_roots,
    collect_restored_read_only_excluded_children, detach_mount_if_present,
    is_covered_by_scoped_fuse_mount, mount_source_for_target_from_mountinfo,
    mountinfo_has_own_private_restore_layer, mountinfo_has_target, mountinfo_root_is_app_sandbox,
    mountinfo_root_matches_data_backend, namespace_mappings_outside_scoped_fuse,
    path_overlaps_mapping_request, path_shadows_mapping_request, read_mountinfo,
};
use super::map::PathMappingApplyOptions;
use super::planner::MEDIA_RW_GID;
use super::{MountPlanner, concrete_mount_fallback_parent};
use crate::domain::PathMapping;
use crate::platform::{fs, module_paths, paths};
use libc::{chmod, chown};
use std::ffi::CString;

/// Android 应用 uid 起点。低于该值的是系统 uid，不具备 app-specific 外部存储语义。
const SYSTEM_UID_LIMIT: i32 = 10000;

impl MountPlanner {
    fn prepare_real_storage_anchor(&mut self, storage_path: &str) -> Option<String> {
        self.real_storage_anchor = None;
        let real_storage_anchor_root = module_paths::REAL_STORAGE_TMP_DIR;
        let real_storage_anchor = paths::join(real_storage_anchor_root, &self.user_id.to_string());

        // storage_root_is_already_redirected 与 real_storage_anchor_is_usable 都是只读探测，
        // 且两者之间没有挂载变更，共享一次 mountinfo 读取，避免重复整份读取与解析。
        let mountinfo = if self.redirect_target.is_empty() {
            String::new()
        } else {
            read_mountinfo().unwrap_or_default()
        };

        if self.storage_root_is_already_redirected(&mountinfo, storage_path) {
            if self.real_storage_anchor_is_usable(&mountinfo, &real_storage_anchor) {
                // 复用前必须确认这个锚点指向的是**真实存储根**，而不是被上一轮重定向污染成沙箱的树。
                //
                // 配置热更新时顶层 `/storage/emulated/<user>` 已经被上一轮的沙箱挂载覆盖，这里因此
                // 判定「已重定向」并复用旧锚点；但旧锚点在上一轮里是从当时的 `/storage/emulated/<user>`
                // 绑过来的，那时它已经是沙箱，于是锚点也被污染。复用污染锚点会让后继的
                // `path_mappings` 找不到真实源目录：`apply_resolved_path_mappings` 静默返回 false，
                // 只留下 `redirect done`，映射目标落到沙箱内而不是真实公共路径
                // （真机上表现为场景 29 的 `file_missing .../Download/Test/srt_hot_after.txt`，
                // 文件实际在 .../Android/data/<pkg>/sdcard/Download/Test/）。
                //
                // 锚点健康时它应当把真实存储根挂在自身路径上，`root` 字段形如 `/media/<user>`；
                // 被污染时 `root` 会带 `Android/data/<包名>`。判定只排除「指向本应用沙箱」这一种
                // 明确污染形态，其它形态仍按可用处理，避免把既有环境差异误判成污染而反复重绑。
                if !mountinfo_root_is_app_sandbox(
                    &mountinfo,
                    &real_storage_anchor,
                    &self.package_name,
                ) {
                    log::info!(
                        "real storage anchor reused after redirect pkg={} storage={} anchor={}",
                        self.package_name,
                        storage_path,
                        real_storage_anchor
                    );
                    self.real_storage_anchor = Some(real_storage_anchor.clone());
                    return Some(real_storage_anchor.to_string());
                }
                log::warn!(
                    "real storage anchor polluted by previous redirect, rebinding pkg={} anchor={}",
                    self.package_name,
                    real_storage_anchor
                );
            } else {
                log::warn!(
                    "real storage anchor unavailable after redirect, fallback backend pkg={} storage={} anchor={} target={}",
                    self.package_name,
                    storage_path,
                    real_storage_anchor,
                    self.redirect_target
                );
            }
            // 锚点不可用或被上一轮重定向污染时，摘除视图根上的重定向绑定，用**真实可见
            // 视图**重建锚点，然后立刻把重定向绑回去。
            //
            // 顺序即安全性：摘除之后视图根会短暂回落为真实公共存储，若应用在这个窗口里
            // 写入，本该进沙箱的文件会落到公共路径。因此下面三个动作必须紧邻执行，中间
            // 不得插入任何 IO、目录创建或路径遍历——窗口被压缩到三次系统调用。重建用的
            // 源是重定向目标（沙箱）路径，不经过视图根，所以可以在窗口内安全使用。
            //
            // 之所以不能用 /data/media 后端锚点代替：该设备上 /data/media 是 ext4 而不是
            // FUSE，用它作为映射源会绕过 FUSE 权限层，应用对映射目标没有写权限（表现为
            // 热重载后首次写入成功、随后连续 Permission denied）。
            detach_mount_if_present(storage_path);
            let rebound = self.bind_mount(storage_path, &real_storage_anchor, true);
            let redirect_backend = self.to_data_media_backend_path(&self.redirect_target);
            let restored = if redirect_backend.is_empty() {
                false
            } else {
                self.bind_mount(&redirect_backend, storage_path, true)
            };
            if rebound {
                log::info!(
                    "real storage anchor rebound after detaching redirect pkg={} storage={} anchor={} redirect_restored={}",
                    self.package_name,
                    storage_path,
                    real_storage_anchor,
                    restored
                );
                self.real_storage_anchor = Some(real_storage_anchor.clone());
                return Some(real_storage_anchor.to_string());
            }
            log::warn!(
                "real storage anchor rebound failed, fallback backend pkg={} storage={} anchor={}",
                self.package_name,
                storage_path,
                real_storage_anchor
            );
            let anchor = self.bind_data_media_real_storage_anchor(
                real_storage_anchor_root,
                &real_storage_anchor,
            );
            self.real_storage_anchor = anchor.clone();
            return anchor;
        }

        if let Some(anchor) = self.bind_visible_real_storage_anchor(
            storage_path,
            real_storage_anchor_root,
            &real_storage_anchor,
        ) {
            self.real_storage_anchor = Some(anchor.clone());
            return Some(anchor);
        }

        // 走到这里说明所有可见别名源建出来的锚点都指向本应用沙箱：判为「未重定向」并不
        // 代表视图根真的干净，热重载时 storage_root_is_already_redirected 可能漏检（run
        // 35452820455 实测：该分支下 6 个可见源全部被判污染）。此时同样摘除视图根上的
        // 重定向、用真实视图重建锚点、再立即绑回，而不是退回后端锚点——后端锚点在部分
        // 设备上指向 ext4 的 /data/media，会绕过 FUSE 权限层导致 Permission denied。
        // 三个动作紧邻执行，窗口压缩到三次系统调用，中间不得插入 IO。
        detach_mount_if_present(storage_path);
        let rebound = self.bind_visible_real_storage_anchor(
            storage_path,
            real_storage_anchor_root,
            &real_storage_anchor,
        );
        let redirect_backend = self.to_data_media_backend_path(&self.redirect_target);
        let restored = if redirect_backend.is_empty() {
            false
        } else {
            self.bind_mount(&redirect_backend, storage_path, true)
        };
        if let Some(anchor) = rebound {
            log::info!(
                "real storage anchor rebound after detaching redirect (unredirected branch) pkg={} storage={} anchor={} redirect_restored={}",
                self.package_name,
                storage_path,
                anchor,
                restored
            );
            self.real_storage_anchor = Some(anchor.clone());
            return Some(anchor);
        }

        let anchor = self
            .bind_data_media_real_storage_anchor(real_storage_anchor_root, &real_storage_anchor);
        self.real_storage_anchor = anchor.clone();
        anchor
    }

    fn bind_visible_real_storage_anchor(
        &self,
        storage_path: &str,
        real_storage_anchor_root: &str,
        real_storage_anchor: &str,
    ) -> Option<String> {
        if !self.ensure_directory_exists(real_storage_anchor_root, false)
            || !self.ensure_directory_exists(real_storage_anchor, false)
        {
            return None;
        }

        let data_media_root = paths::data_media_user_root_for_user(self.user_id);
        for source_candidate in self.expand_storage_alias_paths(storage_path) {
            if paths::eq_ignore_case(&source_candidate, &data_media_root) {
                continue;
            }
            if !fs::is_directory(&source_candidate) {
                continue;
            }
            detach_mount_if_present(real_storage_anchor);
            if self.bind_mount(&source_candidate, real_storage_anchor, true) {
                // 绑定后必须复核锚点没有指向本应用沙箱：热重载时
                // `storage_root_is_already_redirected` 可能判为「未重定向」并走到这里，
                // 但 `/storage/emulated/<user>` 的可见内容已被上一轮重定向覆盖，绑定出来的
                // 锚点 `root` 会带 `Android/{data,media,obb}/<包名>`，指向沙箱而不是真实存储。
                // 用被污染的锚点解析 path_mappings 源目录，映射目标会落到沙箱内而不是真实
                // 公共路径（场景 29 的 `Download/Test` 落点错）。复核失败就摘掉重来，由上层
                // 回退到以 `/data/media` 为源的后端锚点，那里不受重定向影响。
                let fresh_mountinfo = read_mountinfo().unwrap_or_default();
                if mountinfo_root_is_app_sandbox(
                    &fresh_mountinfo,
                    real_storage_anchor,
                    &self.package_name,
                ) {
                    log::warn!(
                        "real storage anchor visible bind polluted, retry backend pkg={} source={} anchor={}",
                        self.package_name,
                        source_candidate,
                        real_storage_anchor
                    );
                    detach_mount_if_present(real_storage_anchor);
                    continue;
                }
                log::info!(
                    "real storage anchored visible {} -> {}",
                    source_candidate,
                    real_storage_anchor
                );
                return Some(real_storage_anchor.to_string());
            }
        }
        None
    }

    fn bind_data_media_real_storage_anchor(
        &self,
        real_storage_anchor_root: &str,
        real_storage_anchor: &str,
    ) -> Option<String> {
        if !self.ensure_directory_exists(real_storage_anchor_root, false)
            || !self.ensure_directory_exists(real_storage_anchor, false)
        {
            return None;
        }

        let data_media_root = paths::data_media_user_root_for_user(self.user_id);
        if !fs::is_directory(&data_media_root) {
            log::warn!(
                "real storage anchor backend missing pkg={} backend={}",
                self.package_name,
                data_media_root
            );
            return None;
        }

        detach_mount_if_present(real_storage_anchor);
        if self.bind_mount(&data_media_root, real_storage_anchor, false) {
            log::info!(
                "real storage anchored backend {} -> {}",
                data_media_root,
                real_storage_anchor
            );
            return Some(real_storage_anchor.to_string());
        }

        None
    }

    fn real_storage_anchor_is_usable(&self, mountinfo: &str, real_storage_anchor: &str) -> bool {
        if !fs::is_directory(real_storage_anchor) {
            return false;
        }
        if !mountinfo_has_target(mountinfo, real_storage_anchor) {
            return false;
        }
        ["Android", "Download", "DCIM"]
            .iter()
            .any(|child| fs::is_directory(&paths::join(real_storage_anchor, child)))
    }

    fn storage_root_is_already_redirected(&self, mountinfo: &str, storage_path: &str) -> bool {
        if self.redirect_target.is_empty() {
            return false;
        }
        let redirect_backend = self.to_data_media_backend_path(&self.redirect_target);
        if redirect_backend.is_empty() {
            return false;
        }
        let Some(root) = mount_source_for_target_from_mountinfo(mountinfo, storage_path) else {
            return false;
        };
        if paths::is_same_or_child(&redirect_backend, &root) {
            return true;
        }
        let matched = mountinfo_root_matches_data_backend(&root, &redirect_backend);
        if matched {
            // 走到归一化命中说明视图根由系统 FUSE 视图承载（`root` 写作 `/<user>/...`）。
            // 这条记录必须与「真正未重定向」在日志里分得开：前者是复用既有锚点、映射沿用
            // FUSE 后端权限层的前置条件；后者才是重建锚点流程的起点，而重建在当前视图根
            // 仍指向沙箱时必然失败并退回 `/data/media` 后端锚点。
            log::info!(
                "storage root redirected via fuse view pkg={} target={} root={} backend={}",
                self.package_name,
                storage_path,
                root,
                redirect_backend
            );
        }
        matched
    }

    pub fn apply_sdcard_redirect(
        &mut self,
        allowed_real_paths: &[String],
        excluded_real_paths: &[String],
        read_only_paths: &[String],
        path_mappings: &[PathMapping],
        scoped_fuse_roots: &[String],
    ) -> bool {
        let resolved_target_storage =
            self.resolve_user_path(&self.normalize_path(&self.redirect_target));
        if resolved_target_storage.is_empty() {
            log::error!("redirect target empty");
            return false;
        }

        let resolved_target = self.to_data_media_backend_path(&resolved_target_storage);
        if resolved_target.is_empty() {
            log::error!(
                "redirect target not under storage/emulated: {}",
                resolved_target_storage
            );
            return false;
        }

        log::info!("redirect target={}", resolved_target_storage);
        log::debug!("redirect backend={}", resolved_target);
        log::info!(
            "mount args pkg={} uid={} user={} allow={} excl={} ro={} map={}",
            self.package_name,
            self.app_uid,
            self.user_id,
            allowed_real_paths.len(),
            excluded_real_paths.len(),
            read_only_paths.len(),
            path_mappings.len()
        );

        if !self.ensure_mount_namespace_prepared() {
            log::error!("mount ns init failed");
            return false;
        }

        let storage_path = paths::storage_user_root_for_user(self.user_id);
        let android_path = paths::join(&storage_path, "Android");
        log::info!(
            "key mounts storage={} android={}",
            storage_path,
            android_path
        );

        let real_storage_anchor = self.prepare_real_storage_anchor(&storage_path);
        if real_storage_anchor.is_none() {
            log::warn!(
                "real storage anchor failed, fallback to data/media: {}/{}",
                module_paths::REAL_STORAGE_TMP_DIR,
                self.user_id
            );
        }

        let data_media_root = paths::data_media_user_root_for_user(self.user_id);
        if !fs::is_directory(&data_media_root) {
            log::error!("data/media missing: {}", data_media_root);
            return false;
        }

        if !self.ensure_directory_exists(&resolved_target, false) {
            log::error!("mkdir redirect failed: {}", resolved_target);
            return false;
        }

        if !self.ensure_writable_mapped_directory(&resolved_target, self.app_uid) {
            log::warn!("fix redirect root perm failed: {}", resolved_target);
        }
        self.ensure_app_writable_directory_chain(&resolved_target, self.app_uid);

        let redirect_android_path = paths::join(&resolved_target, "Android");
        if !self.ensure_directory_exists(&redirect_android_path, true) {
            log::error!("mkdir android placeholder failed");
            return false;
        }
        self.ensure_app_writable_directory_chain(&redirect_android_path, self.app_uid);
        if !self.prepare_redirect_android_app_directories(&redirect_android_path) {
            log::error!("mkdir redirected Android app directories failed");
            return false;
        }

        let mut is_storage_redirect_applied = false;
        if !self.bind_mount_with_storage_aliases(
            &resolved_target,
            &storage_path,
            true,
            super::PrimaryMountFailure::AbortAll,
            Some("storage main mount failed"),
            Some("storage alias mount failed"),
            Some("storage alias mount ok"),
            Some(&mut is_storage_redirect_applied),
        ) {
            log::error!("storage redirect failed");
            return false;
        }

        if !is_storage_redirect_applied {
            log::error!("storage redirect failed (no mount point)");
            return false;
        }
        self.is_storage_root_redirected = true;

        log::info!("android root stays redirected unless allowed explicitly");
        self.ensure_scoped_fuse_mount_points(scoped_fuse_roots, &storage_path);
        // 自有私有目录恢复必须在放行/映射 bind **之前**执行：映射目标若落在自有
        // 目录子树下（场景 35 的 Android/data/<pkg>/... → Download/...），映射 bind
        // 要叠在恢复 bind 之上才会生效；恢复放在末尾则会反过来遮蔽先放的映射
        // bind（CI A17 场景 35 实证：写入成功但映射目标缺失）。namespace 形态下
        // apply 之后没有会遮蔽存储根的挂载，这里恢复即终态；宿主接管/scoped 形态
        // 中 apply 阶段的恢复可能被随后的视图根 FUSE 遮蔽（真机 2026-10-25 实证：
        // 同一构建时而生效时而丢失），由调用方在全部挂载动作结束后调用
        // [`Self::restore_own_private_system_view`] 幂等补挂（same_mount_identity
        // 判定已恢复时不再叠层）。
        self.restore_own_private_directories(&storage_path, &data_media_root, scoped_fuse_roots);

        let restored_allowed_paths = self.restore_allowed_real_paths(
            allowed_real_paths,
            excluded_real_paths,
            scoped_fuse_roots,
            &real_storage_anchor,
            &data_media_root,
            &storage_path,
        );
        self.restore_excluded_sandbox_paths(excluded_real_paths, &resolved_target, &storage_path);

        let mapping_source_roots =
            build_mapping_source_roots(&real_storage_anchor, &data_media_root);
        let resolved_mappings = self.resolve_path_mappings(path_mappings, &storage_path);
        let namespace_mappings =
            namespace_mappings_outside_scoped_fuse(&resolved_mappings, scoped_fuse_roots);
        if !resolved_mappings.is_empty() {
            log::info!(
                "map resolve in={} effective={} namespace={} scoped_fuse={}",
                path_mappings.len(),
                resolved_mappings.len(),
                namespace_mappings.len(),
                scoped_fuse_roots.len()
            );
            if !self.apply_resolved_path_mappings(
                &namespace_mappings,
                &storage_path,
                &mapping_source_roots,
                &restored_allowed_paths,
                read_only_paths,
                excluded_real_paths,
                PathMappingApplyOptions {
                    should_chown_current_dirs: true,
                    should_create_missing_request_path: true,
                    should_use_existing_target_source_only: false,
                },
            ) {
                log::warn!(
                    "mapping: nothing applied count={} storage={}",
                    namespace_mappings.len(),
                    storage_path
                );
            }
        }

        let read_only_shadowed_mappings = self.read_only_shadowed_path_mappings(
            &namespace_mappings,
            read_only_paths,
            &storage_path,
            "readonly map restore",
        );
        if !read_only_paths.is_empty() {
            // 只读规则整体未生效意味着本该被拒绝的写入会被放行，属用户可感知的功能
            // 失效而非日志噪音，必须留下告警而不是丢弃返回值。
            if !self.apply_read_only_paths(
                read_only_paths,
                excluded_real_paths,
                &namespace_mappings,
                &storage_path,
                &mapping_source_roots,
                scoped_fuse_roots,
            ) {
                log::warn!(
                    "readonly: nothing applied count={} storage={}",
                    read_only_paths.len(),
                    storage_path
                );
            }
            if !read_only_shadowed_mappings.is_empty() {
                let is_any_restored = self.apply_resolved_path_mappings(
                    &read_only_shadowed_mappings,
                    &storage_path,
                    &mapping_source_roots,
                    &restored_allowed_paths,
                    read_only_paths,
                    excluded_real_paths,
                    PathMappingApplyOptions {
                        should_chown_current_dirs: true,
                        should_create_missing_request_path: false,
                        should_use_existing_target_source_only: false,
                    },
                );
                if is_any_restored {
                    log::info!(
                        "readonly shadowed mappings restored count={}",
                        read_only_shadowed_mappings.len()
                    );
                }
            }
        }

        log::info!("redirect done");
        true
    }

    /// 把允许真实目录按解析结果绑回应用视图，返回实际恢复成功的路径。
    ///
    /// 返回值按长度降序去重排序：映射解析据此判断「允许目录优先」，防止映射源
    /// 覆盖在已放行的真实目录上。scoped FUSE 覆盖的路径跳过 bind（由会话接管）。
    fn restore_allowed_real_paths(
        &self,
        allowed_real_paths: &[String],
        excluded_real_paths: &[String],
        scoped_fuse_roots: &[String],
        real_storage_anchor: &Option<String>,
        data_media_root: &str,
        storage_path: &str,
    ) -> Vec<String> {
        let mut restored_allowed_paths: Vec<String> = Vec::new();
        if allowed_real_paths.is_empty() {
            return restored_allowed_paths;
        }
        let resolved_paths = self.resolve_concrete_storage_paths(
            allowed_real_paths,
            storage_path,
            "allow",
            "allow mount",
        );

        let mut effective_paths: Vec<String> = Vec::with_capacity(resolved_paths.len());
        for path in resolved_paths {
            let mut is_redundant = false;
            for kept in &effective_paths {
                if paths::matches(kept, &path, true) {
                    is_redundant = true;
                    break;
                }
            }
            if !is_redundant {
                effective_paths.push(path);
            }
        }

        for allowed_path in effective_paths {
            if is_covered_by_scoped_fuse_mount(&allowed_path, scoped_fuse_roots) {
                log::info!(
                    "skip allow mount path (handled by scoped fuse): {}",
                    allowed_path
                );
                continue;
            }

            let Some(relative) = paths::relative_child_path(&allowed_path, storage_path) else {
                continue;
            };

            if !self.ensure_directory_exists(&allowed_path, true) {
                log::warn!("mkdir allow failed: {}", allowed_path);
                continue;
            }

            let mut is_restored_allowed_path = false;
            let source_candidates = build_allowed_real_source_candidates(
                real_storage_anchor,
                data_media_root,
                relative,
            );

            for real_source in source_candidates {
                if !self.ensure_real_public_directory_exists(&real_source) {
                    log::warn!("real path missing and mkdir failed: {}", real_source);
                    continue;
                }
                self.ensure_allowed_real_existing_directory_tree_writable(
                    &real_source,
                    excluded_real_paths,
                );

                let _ = self.bind_mount_with_storage_aliases(
                    &real_source,
                    &allowed_path,
                    true,
                    super::PrimaryMountFailure::StopCurrentTarget,
                    None,
                    Some("allow alias restore failed"),
                    Some("allow alias restore ok"),
                    Some(&mut is_restored_allowed_path),
                );
                if is_restored_allowed_path {
                    log::info!("allow restored {}", allowed_path);
                    restored_allowed_paths.push(allowed_path);
                    break;
                }
            }
        }
        paths::sort_dedup_paths_longest_first_case_insensitive(&mut restored_allowed_paths);
        restored_allowed_paths
    }

    /// 把排除目录按「沙盒可写镜像」恢复：每个排除目标在沙盒根下有对应目录，
    /// 绑定后应用对其的写入落进沙盒而不影响真实存储。
    fn restore_excluded_sandbox_paths(
        &self,
        excluded_real_paths: &[String],
        resolved_target: &str,
        storage_path: &str,
    ) {
        if excluded_real_paths.is_empty() {
            return;
        }
        let resolved_paths =
            self.resolve_excluded_storage_mount_paths(excluded_real_paths, storage_path);

        for excluded_path in resolved_paths {
            let Some(relative) = paths::relative_child_path(&excluded_path, storage_path) else {
                continue;
            };

            if !fs::is_directory(&excluded_path) {
                log::info!("exclude mount target missing, skip bind: {}", excluded_path);
                continue;
            }

            let sandbox_source = paths::join(resolved_target, relative);
            if !self.ensure_writable_mapped_directory(&sandbox_source, self.app_uid) {
                log::warn!("exclude sandbox mkdir failed: {}", sandbox_source);
                continue;
            }

            let mut is_restored_excluded_path = false;
            let _ = self.bind_mount_with_storage_aliases(
                &sandbox_source,
                &excluded_path,
                true,
                super::PrimaryMountFailure::StopCurrentTarget,
                None,
                Some("exclude alias restore failed"),
                Some("exclude alias restore ok"),
                Some(&mut is_restored_excluded_path),
            );
            if is_restored_excluded_path {
                log::info!("exclude restored {}", excluded_path);
            }
        }
    }

    fn prepare_redirect_android_app_directories(&self, redirect_android_path: &str) -> bool {
        let app_data_root = paths::join(
            &paths::join(redirect_android_path, "data"),
            &self.package_name,
        );
        let app_media_root = paths::join(
            &paths::join(redirect_android_path, "media"),
            &self.package_name,
        );
        let required_paths = [
            paths::join(&app_data_root, "files"),
            paths::join(&app_data_root, "cache"),
            app_media_root,
        ];

        for path in required_paths {
            if !self.ensure_writable_mapped_directory(&path, self.app_uid) {
                log::warn!("redirected Android app directory mkdir failed: {}", path);
                return false;
            }
            self.ensure_app_writable_directory_chain(&path, self.app_uid);
        }
        true
    }

    // 把应用自己包名的 `Android/media/<pkg>` 恢复成系统存储视图（`data`/`obb` 保持
    // f2fs 直连，原因见下方清单处的注释）。这些目录属于 app-specific 外部存储，系统
    // MediaProvider 已按包名隔离，本就不该被重定向；而 storage root 整体 bind 到沙箱时
    // 会把它们一并带走，应用保存或下载到自己私有目录的文件会多套一层 sdcard/。这里在
    // 整体重定向之后优先从挂载前保存的系统 FUSE 锚点恢复；不能直接绑定 /data/media 的
    // f2fs 后端，否则目标会带着 media_rw_data_file 上下文绕过 MediaProvider FUSE，
    // 普通应用即使目录属于自身 UID 仍会被 SELinux 拒绝。后端路径只用于创建缺失的包名根。
    //
    // 覆盖形态不再构成跳过理由（2026-10-25）：整根由共享宿主 FUSE 接管时，自有私有
    // 目录若留在宿主层，后端是 f2fs 直连；未接管的消费方（MediaProvider 自身、跨进程
    // 管理器、Shizuku/shell）经系统 MediaProvider FUSE 访问同一物理文件时属于**另一层
    // 内核缓存**。SQLite 的 WAL 依赖跨进程共享的 `-shm` mmap 与页内锁，两层页缓存之间
    // 完全不可见：一方判定 WAL 无效后回滚到上一次 checkpoint 的主库（真机 XRadiant 的
    // 主库曾被回落到 4096B 空 schema 页），表现为掉登录、数据丢失、打库卡等锁。
    // 只有「该目录自身被单独挂成 scoped FUSE 根」时仍跳过——那种形态下再叠 bind 会让
    // 已启动的 FUSE 服务空转，清理账本也对不上。
    fn restore_own_private_directories(
        &self,
        storage_path: &str,
        data_media_root: &str,
        scoped_fuse_roots: &[String],
    ) -> usize {
        // 恢复三类自有私有目录（`Android/{data,media,obb}/<pkg>`）。
        //
        // namespace 形态下 storage root 整体 bind 到沙箱（`Android/data/<pkg>/sdcard`），
        // 自有私有目录若不 bind 回真实位置，应用保存/下载到自己私有目录的文件会多套
        // 一层 sdcard/ 且真实路径缺失（CI A17 场景 34 实证：写入成功但可见层与后端
        // 双双查无此文件）。2026-10-25 曾把恢复收窄到仅 media，依据是「data 恢复让
        // 沙箱根在 MP 重启后 ENOTCONN（真机场景 2/3/4/12 全灭）」——该观察与同会话
        // 后来定位的 `prepare_real_storage_anchor` 摘根缺陷同场发生，属误归因；
        // 恢复三类目录的全量 CI（run 40167024322，场景 1-38+40 五版本）是绿的。
        //
        // `Android/media/<pkg>` 无条件恢复（XRadiant/Sen 这类跨进程共享数据库场景，
        // 两层页缓存分裂见下）。`data`/`obb` 在被 scoped FUSE 覆盖时跳过：那种形态
        // 下自有私有路径由 srx FUSE 数据面按真实路径处理，再叠 bind 会绕开引擎、
        // 让监视记录与清理账本对不上。
        let private_roots = [
            format!("Android/data/{}", self.package_name),
            format!("Android/media/{}", self.package_name),
            format!("Android/obb/{}", self.package_name),
        ];
        let mut restored_count = 0usize;
        // 系统 uid 没有 app-specific 外部存储语义：它们在 `/data/media/<user>/Android`
        // 下没有按包名隔离的目录，逐个 mkdir 只会在开机风暴期刷出成片告警。
        if self.app_uid < SYSTEM_UID_LIMIT {
            return 0;
        }
        // 源必须是**系统视图锚点**：`real_storage_anchor` 在部分形态下会被污染成沙箱、
        // 或退回 `/data/media` 的 f2fs 后端锚点，用它 bind 会把自有私有目录重新拉出系统
        // FUSE 层，回到「宿主 FUSE 与系统 FUSE 各持一层页缓存」的分裂形态。
        let Some(system_view) = self.ensure_system_view_anchor() else {
            log::warn!(
                "own private system view anchor unavailable pkg={}",
                self.package_name
            );
            return 0;
        };
        for relative in private_roots {
            let storage_target = paths::join(storage_path, &relative);
            let is_media = relative.starts_with("Android/media/");
            if scoped_fuse_roots
                .iter()
                .any(|root| paths::eq_ignore_case(&storage_target, root))
                || (!is_media
                    && is_covered_by_scoped_fuse_mount(&storage_target, scoped_fuse_roots))
            {
                log::info!(
                    "skip own private restore (handled by scoped fuse): {}",
                    storage_target
                );
                continue;
            }

            let backend_source = paths::join(data_media_root, &relative);
            // MediaProvider 可能尚未为包名下的任意子目录建立真实目录；先由守护进程
            // 在当前 mount namespace 中显式创建完整后端路径，再执行元数据修正。
            // 创建失败不致命：源探测会再判一次，缺目录时静默等待后续 reconcile。
            if !fs::create_directory(&backend_source, self.app_uid)
                || !self.ensure_writable_mapped_directory(&backend_source, self.app_uid)
            {
                log::debug!("own private backend mkdir skipped: {}", backend_source);
            }
            // 三类自有私有目录统一保留系统 FUSE 的访问控制与上下文：源取系统视图锚点，
            // 缺失时不把 /data/media 直接暴露给应用，保留诊断等待后续恢复。
            let source = paths::join(&system_view, &relative);
            if !fs::is_directory(&source) {
                log::warn!("own private system view missing: {}", source);
                continue;
            }
            // metadata_operations_path 将 storage alias 转到 /data/media；但在包名
            // 根尚未挂载子目录时，需要先在当前 namespace 的可见路径创建挂载点，
            // 否则 bind 只能覆盖包名根，应用后续创建 QQfile_recv 会得到 ENOENT。
            //
            // 只在换算后的后端路径上创建，不再先对视图路径直接 mkdir：apply 阶段
            // `/storage/emulated/<user>` 可能还是沙箱 bind、或指向尚未启动的 FUSE，
            // 对该路径 mkdir 会失败并短路掉本可成功的后端创建（真机 2026-10-25：
            // `own private target mkdir failed` → 恢复计数恒为 0）。
            if !self.ensure_directory_exists(&storage_target, true) {
                log::warn!(
                    "own private target mkdir failed: {} errno={}",
                    storage_target,
                    fs::last_create_directory_errno()
                );
                continue;
            }

            let mut is_restored = false;
            // 幂等：热重载与 reconcile 会重复进入这里，已经指向系统视图（与源同一
            // dev+ino）时不再叠一层 bind，否则同一 target 上的层数会随重挂次数增长。
            // 已在系统视图上时属主通常也已对齐（见 restore 后的归一化），跳过扫描。
            //
            // dev+ino 之外再比 mountinfo root 形态（真机 2026-10-27 issue #2 实证）：
            // 应用视图的 `/storage/emulated/<user>` 上叠着 MediaProvider 的二级用户
            // FUSE 视图（fs 0:340），锚点 system_view 是 0:294 的 bind，stat 的 dev
            // 恒不相等，仅凭 dev+ino 会把「已恢复」判成「未恢复」，每轮 reconcile
            // 全量重 bind（单包媒体目录一小时内 18 次、含同秒并发 3 次），层数叠加
            // 且 MP FUSE 请求风暴最终把 FuseDaemon 拖挂（`__fuse_simple_request`
            // 永久停等 → 全局存储停摆）。root 精确等于 `/<user>/<relative>` 的记录
            // 存在即视为已恢复；MP 若在其上重建用户视图，内容仍经其视图取自同一棵
            // 底层树，重复 bind 只会触发下一轮重建循环。
            let expected_restore_root = format!("/{}/{}", self.user_id, relative);
            let restore_mountinfo = read_mountinfo().unwrap_or_default();
            if same_mount_identity(&storage_target, &source)
                || mountinfo_has_own_private_restore_layer(
                    &restore_mountinfo,
                    &storage_target,
                    &expected_restore_root,
                )
            {
                log::debug!("own private already on system view {}", storage_target);
                restored_count += 1;
                continue;
            }
            let _ = self.bind_own_private_restore(
                &source,
                &storage_target,
                true,
                Some("own private restore failed"),
                Some("own private alias restore failed"),
                Some("own private alias restore ok"),
                Some(&mut is_restored),
            );
            if is_restored {
                log::info!("own private restored {}", storage_target);
                // bind 回系统 FUSE 后，权限层按文件属主判定：这棵树历史上被
                // MediaProvider 代写过（真机实测全树属主是 MP 自身 uid），不归一化
                // 的话应用访问自己的媒体目录反而 EACCES（`ky2: 无法打开数据库文件`）。
                // 原生语义下属主就是应用本身，这里一次性对齐，之后的新文件由
                // companion fchown 链路维持。
                //
                // **只归一化 media 子树**：`data`/`obb` 的属组漂移由一次性迁移
                // （owner-migration）与场景 6 的「漂移不被运行期修复」契约管辖，
                // 运行期 chown 会把注入的漂移夹具修回、破坏防回潮守卫。
                if is_media {
                    self.normalize_own_media_ownership(data_media_root, &relative);
                }
                restored_count += 1;
            } else {
                // 失败必须留痕：这条恢复是跨进程 SQLite 共享缓存层的唯一保证，
                // 静默失败会直接回到两层页缓存分裂（真机表现为每隔若干次热重载
                // 掉一次登录）。
                log::warn!(
                    "own private restore failed target={} source={} errno={}",
                    storage_target,
                    source,
                    fs::last_create_directory_errno()
                );
            }
        }
        restored_count
    }

    /// 递归把自有媒体目录的属主对齐到应用 uid（组 media_rw）。
    ///
    /// 这棵树历史上被 MediaProvider 代写过（真机实测 `XRadiant/` 全树属主是 MP 自身
    /// uid），bind 回系统 MediaProvider FUSE 后权限层按属主判定，应用访问自己的媒体
    /// 目录反而 EACCES（真机 2026-10-25：`ky2: 无法打开数据库文件`）。原生语义下这棵
    /// 树的属主就是应用本身，这里一次性对齐，之后的新文件由 companion fchown 链路维持。
    ///
    /// 直接对 f2fs 后端路径做 chown/chmod：root 侧操作不受 MediaProvider 权限约束，
    /// 且不用等 MediaProvider FUSE 的 attr 缓存过期。深度与条目数双重限幅，防止极端
    /// 目录拖垮挂载子进程预算。
    fn normalize_own_media_ownership(&self, backend_root: &str, relative: &str) {
        let root = paths::join(backend_root, relative);
        let (fixed, skipped) = self.fix_media_tree_ownership(&root, 0);
        if fixed > 0 || skipped > 0 {
            log::info!(
                "own media ownership normalized fixed={} skipped={} root={}",
                fixed,
                skipped,
                root
            );
        }
    }

    fn fix_media_tree_ownership(&self, path: &str, depth: usize) -> (usize, usize) {
        use std::os::unix::fs::MetadataExt as _;

        const MAX_DEPTH: usize = 8;
        const MAX_ENTRIES: usize = 4096;
        if depth > MAX_DEPTH {
            return (0, 1);
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return (0, 1);
        };
        let mut fixed = 0usize;
        let mut skipped = 0usize;
        for entry in entries.flatten() {
            if fixed + skipped >= MAX_ENTRIES {
                log::warn!("own media ownership scan truncated at {}", path);
                return (fixed, skipped + 1);
            }
            let entry_path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&entry_path) else {
                skipped += 1;
                continue;
            };
            let needs_owner = meta.uid() != self.app_uid as u32;
            if meta.is_dir() {
                if needs_owner {
                    self.fix_media_entry_owner(&entry_path, true);
                    fixed += 1;
                }
                let path_str = entry_path.to_string_lossy().to_string();
                let (child_fixed, child_skipped) =
                    self.fix_media_tree_ownership(&path_str, depth + 1);
                fixed += child_fixed;
                skipped += child_skipped;
            } else if meta.is_file() && needs_owner {
                self.fix_media_entry_owner(&entry_path, false);
                fixed += 1;
            }
        }
        (fixed, skipped)
    }

    /// 单个条目的属主/权限对齐：目录 02770、文件 0660，组固定 media_rw。
    fn fix_media_entry_owner(&self, path: &std::path::Path, is_dir: bool) {
        use std::os::unix::ffi::OsStrExt as _;

        let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
            return;
        };
        // SAFETY: c_path 指向合法 NUL 结尾字符串，chown/chmod 是纯元数据操作，
        // 不触碰文件内容，失败仅影响该条目的可见性（调用方已记录总数）。
        unsafe {
            chown(c_path.as_ptr(), self.app_uid as u32, MEDIA_RW_GID);
            chmod(c_path.as_ptr(), if is_dir { 0o2770 } else { 0o660 });
        }
    }

    /// 共享宿主预接入（整根由宿主 FUSE 接管）之后恢复自有私有目录的系统视图。
    ///
    /// 预接入分支直接短路 planner，不经过 [`Self::apply_sdcard_redirect`]，因此此前
    /// 完全不会执行自有私有目录恢复：`Android/{data,media,obb}/<pkg>` 留在宿主 FUSE
    /// 内、后端是 f2fs 直连，与未接管消费方经系统 MediaProvider FUSE 的访问分成两层
    /// 内核缓存（SQLite `-shm` 的 mmap 与页内锁跨不过去 → WAL 被判无效 → 回滚空主库）。
    ///
    /// **不能复用 [`Self::prepare_real_storage_anchor`]**：该函数在「存储根已重定向」
    /// 分支会先 detach 存储根、用真实视图重建锚点、再把沙箱绑回，那是给
    /// `apply_sdcard_redirect` 准备的序列。预接入路径的存储根是共享宿主 FUSE，
    /// 照搬会把宿主会话摘掉并换成普通 f2fs bind（真机 2026-10-25 实测：支付宝
    /// `/storage/emulated/0` 从 `srx_fuse_host` 变成 `254:58 /media/0/.../sdcard`）。
    /// 这里只做「从当前命名空间里未被重定向覆盖的系统视图根建一个私有锚点」，
    /// 全程不触碰存储根。
    /// 参数 `scoped_fuse_roots` 是本轮规划的 scoped FUSE 根：`data`/`obb` 被其覆盖时
    /// 跳过恢复（自有私有路径由 srx FUSE 数据面按真实路径处理），media 无条件恢复。
    pub fn restore_own_private_system_view(&mut self, scoped_fuse_roots: &[String]) -> usize {
        // 系统 uid（<10000）没有 app-specific 外部存储语义，也不该去创建
        // `/data/media/<user>/Android/{data,media,obb}/<pkg>`；跳过可避免开机时整片
        // 系统服务刷出 `own private source mkdir failed`。
        if self.app_uid < SYSTEM_UID_LIMIT {
            return 0;
        }
        let storage_path = paths::storage_user_root_for_user(self.user_id);
        let data_media_root = paths::data_media_user_root_for_user(self.user_id);
        self.restore_own_private_directories(&storage_path, &data_media_root, scoped_fuse_roots)
    }

    /// 建立（或复用）指向系统存储视图的私有锚点，返回**存储根结构**的路径。
    ///
    /// 源取 `/mnt/user/<user>/emulated`：它在应用命名空间里恒为一层未被模块覆盖的
    /// MediaProvider FUSE 视图（模块只覆盖 `<user>` 子树与存储根，不覆盖 emulated
    /// 本身）。整根 bind 之后，锚点下多一层 `<user>` 才是存储根结构，因此返回
    /// `<anchor>/<user>`；该层次不经过被覆盖的路径解析，是干净的系统视图。
    ///
    /// 锚点根与 `real_storage` 分开存放（见 `module_paths::SYSTEM_VIEW_TMP_DIR`），
    /// 避免与「存储根结构」语义的既有锚点互相覆盖。
    fn ensure_system_view_anchor(&self) -> Option<String> {
        let anchor_root = paths::join(module_paths::SYSTEM_VIEW_TMP_DIR, &self.user_id.to_string());
        let user_view = paths::join(&anchor_root, &self.user_id.to_string());
        let source = format!("/mnt/user/{}/emulated", self.user_id);
        if !fs::is_directory(&source) {
            log::warn!("system view source unavailable: {}", source);
            return None;
        }
        if same_mount_identity(&anchor_root, &source)
            && fs::is_directory(&paths::join(&user_view, "Android"))
        {
            return Some(user_view);
        }
        // 锚点是模块私有路径，不承载任何应用可见路径：上一轮遗留（可能已被污染成沙箱
        // 或 f2fs 后端）可以安全摘除后重建，摘除窗口内没有应用 IO 经过它。
        detach_mount_if_present(&anchor_root);
        if !fs::create_directory(&anchor_root, -1) && !fs::is_directory(&anchor_root) {
            log::warn!("system view anchor mkdir failed: {}", anchor_root);
            return None;
        }
        // **必须非递归 bind**：模块会把 `/mnt/user/<user>/emulated/<user>` 换成自己的
        // FUSE（别名路径也要走重定向），递归 bind 会把这个子挂载一并复制到锚点里，
        // 锚点下的 `<user>` 仍然是模块层，取的还是 f2fs 直连后端——等于没恢复（真机
        // 2026-10-25 实测：`tmp/system_view/0/0` 变成 `srx_fuse_host[...]`）。
        // 非递归 bind 只复制该挂载本身，锚点内 `<user>` 回落到 MediaProvider FUSE 的
        // 普通目录节点，正是需要的干净系统视图。
        if !self.bind_mount(&source, &anchor_root, false) {
            log::warn!(
                "system view anchor bind failed anchor={} source={}",
                anchor_root,
                source
            );
            return None;
        }
        Some(user_view)
    }

    fn ensure_scoped_fuse_mount_points(&self, scoped_fuse_roots: &[String], storage_path: &str) {
        if scoped_fuse_roots.is_empty() {
            return;
        }
        for root in scoped_fuse_roots {
            if paths::eq_ignore_case(root, storage_path) {
                continue;
            }
            if !paths::is_child(root, storage_path) {
                log::warn!("skip scoped fuse mount point outside storage: {}", root);
                continue;
            }
            if self.ensure_directory_exists(root, true) {
                log::info!("scoped fuse mount point ready {}", root);
            } else {
                log::warn!("scoped fuse mount point mkdir failed {}", root);
            }
        }
    }

    pub fn apply_path_mappings_only(
        &mut self,
        path_mappings: &[PathMapping],
        sandboxed_paths: &[String],
        read_only_paths: &[String],
        scoped_fuse_roots: &[String],
    ) -> bool {
        if !self.ensure_mount_namespace_prepared() {
            log::error!("mount ns init failed");
            return false;
        }

        if path_mappings.is_empty() && sandboxed_paths.is_empty() && read_only_paths.is_empty() {
            log::info!("map-only: no mappings/sandbox/read-only paths, skip");
            return true;
        }

        let storage_path = paths::storage_user_root_for_user(self.user_id);
        let real_storage_anchor = self.prepare_real_storage_anchor(&storage_path);
        let data_media_root = paths::data_media_user_root_for_user(self.user_id);
        if !fs::is_directory(&data_media_root) {
            log::error!("data/media missing: {}", data_media_root);
            return false;
        }

        if !sandboxed_paths.is_empty() {
            let resolved_target_storage =
                self.resolve_user_path(&self.normalize_path(&self.redirect_target));
            let resolved_target = self.to_data_media_backend_path(&resolved_target_storage);
            if resolved_target.is_empty() {
                log::error!(
                    "map-only sandbox target not under storage: {}",
                    resolved_target_storage
                );
                return false;
            }

            let (sandboxed_includes, sandboxed_excludes) =
                paths::split_exclusion_rules(sandboxed_paths);
            let mut resolved_sandboxed_paths = Vec::with_capacity(sandboxed_includes.len());
            for path in &sandboxed_includes {
                let Some(resolved) =
                    self.resolve_storage_path_for_mount(path, &storage_path, "sandbox")
                else {
                    continue;
                };
                if paths::contains_wildcards(&resolved) {
                    let matched_paths =
                        self.concrete_wildcard_mount_matches(&resolved, &storage_path);
                    if !matched_paths.is_empty() {
                        log::warn!(
                            "fallback sandbox wildcard to concrete matches: {} -> {} dirs",
                            resolved,
                            matched_paths.len()
                        );
                        resolved_sandboxed_paths.extend(matched_paths);
                        continue;
                    }
                    if let Some(fallback) = concrete_mount_fallback_parent(&resolved, &storage_path)
                    {
                        log::warn!(
                            "fallback sandbox wildcard to concrete parent: {} -> {}",
                            resolved,
                            fallback
                        );
                        resolved_sandboxed_paths.push(fallback);
                    } else {
                        log::warn!(
                            "skip sandbox mount (wildcard has no safe concrete parent): {}",
                            resolved
                        );
                    }
                    continue;
                }
                resolved_sandboxed_paths.push(resolved);
            }
            paths::sort_dedup_paths_longest_first_case_insensitive(&mut resolved_sandboxed_paths);

            for sandboxed_path in resolved_sandboxed_paths {
                let Some(relative) = paths::relative_child_path(&sandboxed_path, &storage_path)
                else {
                    continue;
                };

                let sandbox_source = paths::join(&resolved_target, relative);
                if !self.ensure_writable_mapped_directory(&sandbox_source, self.app_uid) {
                    log::warn!("sandbox source mkdir failed: {}", sandbox_source);
                    continue;
                }
                if is_covered_by_scoped_fuse_mount(&sandboxed_path, scoped_fuse_roots) {
                    log::info!(
                        "skip sandbox mount (handled by scoped fuse): {}",
                        sandboxed_path
                    );
                    continue;
                }
                // 目标是否存在必须用后端路径判定：视图路径的 stat 在挂载子进程里会
                // 遍历系统 FUSE，开机风暴期 MediaProvider FUSE 响应慢，进程会卡在
                // fuse_get_req（D 态，仅 SIGKILL 可打断，真机 2026-10-16 boot 期实测
                // com.miui.voiceassist map_only 子进程即卡在此）。/data/media 与
                // FUSE 视图内容同源，后端判定语义等价且不经过 FUSE。
                let sandbox_target_backend = self.to_data_media_backend_path(&sandboxed_path);
                if sandbox_target_backend.is_empty() || !fs::is_directory(&sandbox_target_backend) {
                    log::warn!(
                        "sandbox mount target missing, skip namespace bind to avoid public placeholder: {}",
                        sandboxed_path
                    );
                    continue;
                }

                let mut is_sandbox_mount_applied = false;
                let _ = self.bind_mount_with_storage_aliases(
                    &sandbox_source,
                    &sandboxed_path,
                    true,
                    super::PrimaryMountFailure::StopCurrentTarget,
                    None,
                    Some("sandbox alias mount failed"),
                    Some("sandbox alias ok"),
                    Some(&mut is_sandbox_mount_applied),
                );
                if is_sandbox_mount_applied {
                    log::info!("map-only sandbox {} -> {}", sandboxed_path, sandbox_source);
                }
            }

            // 嵌套排除规则覆盖沙盒父目录：将排除子目录恢复为真实共享存储，
            // 使 `Pictures/*` 与 `!Pictures/QQ` 组合保持递归语义。
            for excluded in sandboxed_excludes {
                let Some(excluded_path) = self.resolve_storage_path_for_mount(
                    &excluded,
                    &storage_path,
                    "sandbox exclude",
                ) else {
                    continue;
                };
                if paths::contains_wildcards(&excluded_path) {
                    continue;
                }
                // 与 sandbox include 同理：存在性判定走 /data/media 后端路径，
                // 避免挂载子进程在系统 FUSE 上做 stat（开机风暴期 D 态卡死）。
                let excluded_backend = self.to_data_media_backend_path(&excluded_path);
                if excluded_backend.is_empty() || !fs::is_directory(&excluded_backend) {
                    continue;
                }
                let Some(relative) = paths::relative_child_path(&excluded_path, &storage_path)
                else {
                    continue;
                };
                let source_candidates = build_allowed_real_source_candidates(
                    &real_storage_anchor,
                    &data_media_root,
                    relative,
                );
                for source in source_candidates {
                    if !fs::is_directory(&source) {
                        continue;
                    }
                    let mut restored = false;
                    let _ = self.bind_mount_with_storage_aliases(
                        &source,
                        &excluded_path,
                        true,
                        super::PrimaryMountFailure::StopCurrentTarget,
                        None,
                        Some("sandbox exclude restore failed"),
                        Some("sandbox exclude restored"),
                        Some(&mut restored),
                    );
                    if restored {
                        break;
                    }
                }
            }
        }

        let mapping_source_roots =
            build_mapping_source_roots(&real_storage_anchor, &data_media_root);
        let resolved_mappings = self.resolve_path_mappings(path_mappings, &storage_path);
        let namespace_mappings =
            namespace_mappings_outside_scoped_fuse(&resolved_mappings, scoped_fuse_roots);
        log::info!(
            "map-only resolve in={} effective={} namespace={} scoped_fuse={}",
            path_mappings.len(),
            resolved_mappings.len(),
            namespace_mappings.len(),
            scoped_fuse_roots.len()
        );

        let mut is_any_applied = self.apply_resolved_path_mappings(
            &namespace_mappings,
            &storage_path,
            &mapping_source_roots,
            &[],
            read_only_paths,
            &[],
            PathMappingApplyOptions {
                should_chown_current_dirs: false,
                should_create_missing_request_path: false,
                should_use_existing_target_source_only: false,
            },
        );

        let read_only_shadowed_mappings = self.read_only_shadowed_path_mappings(
            &namespace_mappings,
            read_only_paths,
            &storage_path,
            "map-only readonly map restore",
        );
        let mut is_read_only_applied = false;
        if !read_only_paths.is_empty() {
            is_read_only_applied = self.apply_read_only_paths(
                read_only_paths,
                &[],
                &namespace_mappings,
                &storage_path,
                &mapping_source_roots,
                scoped_fuse_roots,
            );
            let is_any_restored = self.apply_resolved_path_mappings(
                &read_only_shadowed_mappings,
                &storage_path,
                &mapping_source_roots,
                &[],
                read_only_paths,
                &[],
                PathMappingApplyOptions {
                    should_chown_current_dirs: false,
                    should_create_missing_request_path: false,
                    should_use_existing_target_source_only: false,
                },
            );
            is_any_applied = is_any_applied || is_any_restored;
        }

        if !is_any_applied && !is_read_only_applied {
            log::warn!("map-only: nothing applied");
        } else {
            log::info!("map-only done");
        }

        true
    }

    /// 摘掉系统 FUSE 视图层后，只重建 path_mappings 的 bind。
    ///
    /// 场景 29 热重载在 x86_64 Android 13/14 上：`apply_sdcard_redirect` 重新 bind 视图根会
    /// 触发 MediaProvider 重建 FUSE 隔离视图，导致视图根被拒；随后摘 FUSE 层又会让挂在
    /// FUSE 层之下的映射被 `MNT_DETACH` 级联 detach。因此必须在摘 FUSE 层之后**只重建映射
    /// 子路径 bind**——它不碰视图根（`/storage/emulated/<user>`），不会触发 MediaProvider
    /// 重建 FUSE（触发重建的是视图根的 detach/re-bind）。
    ///
    /// 复用第一次 apply 已建立的 `self.real_storage_anchor`，绝不重新走
    /// [`prepare_real_storage_anchor`]：那个入口在视图根已被重定向时会 detach/re-bind 视图根
    /// 来重建锚点，同样会触发 MediaProvider 重建 FUSE，与摘除的目的相悖。
    ///
    /// 两条挂载路径都会调用：daemon（`daemon_mount`）与 companion
    /// （`lifecycle::companion_mount`），顺序约束见 `crate::system_fuse_view` 模块文档。
    pub fn reapply_path_mappings_only(&self, path_mappings: &[PathMapping]) -> bool {
        let storage_path = paths::storage_user_root_for_user(self.user_id);
        let data_media_root = paths::data_media_user_root_for_user(self.user_id);
        let mapping_source_roots =
            build_mapping_source_roots(&self.real_storage_anchor, &data_media_root);
        let resolved_mappings = self.resolve_path_mappings(path_mappings, &storage_path);
        let namespace_mappings = namespace_mappings_outside_scoped_fuse(&resolved_mappings, &[]);
        if namespace_mappings.is_empty() {
            log::info!("reapply map after fuse clear: no effective mappings");
            return false;
        }
        log::info!(
            "reapply map after fuse clear in={} effective={}",
            path_mappings.len(),
            namespace_mappings.len()
        );
        self.apply_resolved_path_mappings(
            &namespace_mappings,
            &storage_path,
            &mapping_source_roots,
            &[],
            &[],
            &[],
            PathMappingApplyOptions {
                should_chown_current_dirs: true,
                should_create_missing_request_path: true,
                should_use_existing_target_source_only: false,
            },
        )
    }

    fn apply_read_only_paths(
        &self,
        read_only_paths: &[String],
        excluded_real_paths: &[String],
        path_mappings: &[PathMapping],
        storage_path: &str,
        source_roots: &[String],
        scoped_fuse_roots: &[String],
    ) -> bool {
        let data_media_root = paths::data_media_user_root_for_user(self.user_id);
        let read_only_source_roots = build_read_only_source_roots(source_roots, &data_media_root);
        let (included_read_only_paths, excluded_read_only_paths) =
            paths::split_exclusion_rules(read_only_paths);
        let resolved_paths = self.resolve_concrete_storage_paths(
            &included_read_only_paths,
            storage_path,
            "readonly",
            "readonly mount",
        );
        if resolved_paths.is_empty() {
            log::warn!("readonly: no concrete paths");
            return false;
        }

        let mut excluded_rules =
            self.resolve_read_only_exclusion_rules(excluded_real_paths, storage_path);
        let resolved_read_only_excluded_paths =
            self.resolve_read_only_exclusion_rules(&excluded_read_only_paths, storage_path);
        excluded_rules.extend(paths::overlapping_exclusion_rules(
            &resolved_paths,
            &resolved_read_only_excluded_paths,
        ));
        paths::sort_dedup_paths_case_insensitive(&mut excluded_rules);
        // scoped FUSE 已接管对应路径的只读判定时，namespace 侧不再重复挂载。
        let mut delegated_to_scoped_fuse = false;
        let mut effective_paths: Vec<String> = Vec::with_capacity(resolved_paths.len());
        for path in resolved_paths {
            if path_shadows_mapping_request(&path, path_mappings) {
                log::warn!(
                    "skip readonly mount (mapping request child would be shadowed): {}",
                    path
                );
                continue;
            }
            if path_overlaps_mapping_request(&path, path_mappings) {
                log::warn!(
                    "skip readonly mount (mapping request handled separately): {}",
                    path
                );
                continue;
            }
            if excluded_rules
                .iter()
                .any(|excluded| paths::matches(excluded, &path, true))
            {
                log::warn!("skip readonly mount (excluded conflict): {}", path);
                continue;
            }
            if is_covered_by_scoped_fuse_mount(&path, scoped_fuse_roots) {
                // scoped FUSE 覆盖范围内的只读规则由 FUSE 规则层执行：FUSE 对 open-for-write、
                // truncate、chmod、link、mkdir、rename、delete 与 W_OK 访问逐条判定 is_read_only。
                // 此处再叠一层 namespace 只读 bind 会用 real_storage 锚点遮蔽 FUSE 视图，
                // 命中通配父目录的真实文件会读成空或过期内容（只读规则本意只限制写入）。
                // FUSE 启动失败时 apply_mount_namespace_fallback 会先回滚再按展开后的规则重挂，
                // 只读强制仍然保留，因此这里不保留 namespace 备份挂载。
                log::info!("readonly mount delegated to scoped fuse: {}", path);
                delegated_to_scoped_fuse = true;
                continue;
            }
            let is_redundant = effective_paths
                .iter()
                .any(|kept| paths::matches(kept, &path, true));
            if !is_redundant {
                effective_paths.push(path);
            }
        }

        let restored_excluded_children = collect_restored_read_only_excluded_children(
            &effective_paths,
            &excluded_rules,
            path_mappings,
            scoped_fuse_roots,
        );
        for excluded_child in &restored_excluded_children {
            let Some(relative) = paths::relative_child_path(excluded_child, storage_path) else {
                continue;
            };
            let Some(source_path) =
                self.resolve_read_only_source(relative, &read_only_source_roots)
            else {
                continue;
            };
            if !self.ensure_writable_mapped_directory(&source_path, self.app_uid) {
                log::warn!(
                    "readonly exclude source metadata pre-fix failed: {}",
                    source_path
                );
            }
        }

        let mut is_any_mounted = false;
        let mut mounted_read_only_paths: Vec<String> = Vec::with_capacity(effective_paths.len());
        for read_only_path in &effective_paths {
            let Some(relative) = paths::relative_child_path(read_only_path, storage_path) else {
                continue;
            };

            if !self.ensure_directory_exists(read_only_path, false) {
                log::warn!("readonly target mkdir failed: {}", read_only_path);
                continue;
            }

            let Some(source_path) =
                self.resolve_read_only_source(relative, &read_only_source_roots)
            else {
                log::warn!("readonly source missing: {}", read_only_path);
                continue;
            };
            self.ensure_read_only_tree_accessible(&source_path);

            let mut is_read_only_mounted = false;
            let preserve_data_media_backend = restored_excluded_children
                .iter()
                .any(|excluded_child| paths::is_child(excluded_child, read_only_path));
            if preserve_data_media_backend {
                let _ = self.bind_read_only_mount_with_storage_aliases_preserving_backend(
                    &source_path,
                    read_only_path,
                    true,
                    super::PrimaryMountFailure::StopCurrentTarget,
                    Some("readonly primary mount failed"),
                    Some("readonly alias mount failed"),
                    Some("readonly alias ok"),
                    Some(&mut is_read_only_mounted),
                );
            } else {
                let _ = self.bind_read_only_mount_with_storage_aliases(
                    &source_path,
                    read_only_path,
                    true,
                    super::PrimaryMountFailure::StopCurrentTarget,
                    Some("readonly primary mount failed"),
                    Some("readonly alias mount failed"),
                    Some("readonly alias ok"),
                    Some(&mut is_read_only_mounted),
                );
            }
            if is_read_only_mounted {
                is_any_mounted = true;
                log::info!("readonly {} -> {}", read_only_path, source_path);
                mounted_read_only_paths.push(read_only_path.clone());
            }
        }
        let restored_excluded_children = collect_restored_read_only_excluded_children(
            &mounted_read_only_paths,
            &excluded_rules,
            path_mappings,
            scoped_fuse_roots,
        );
        for excluded_child in restored_excluded_children {
            let Some(relative) = paths::relative_child_path(&excluded_child, storage_path) else {
                continue;
            };
            if !self.ensure_directory_exists(&excluded_child, false) {
                log::warn!(
                    "readonly exclude restore target mkdir failed: {}",
                    excluded_child
                );
                continue;
            }
            let Some(source_path) =
                self.resolve_read_only_source(relative, &read_only_source_roots)
            else {
                log::warn!(
                    "readonly exclude restore source missing: {}",
                    excluded_child
                );
                continue;
            };
            let mut is_exclude_restored = false;
            let _ = self.bind_read_write_mount_with_storage_aliases(
                &source_path,
                &excluded_child,
                true,
                super::PrimaryMountFailure::StopCurrentTarget,
                None,
                Some("readonly exclude alias restore failed"),
                Some("readonly exclude alias restore ok"),
                Some(&mut is_exclude_restored),
            );
            if is_exclude_restored {
                if !self.ensure_writable_mapped_directory(&source_path, self.app_uid) {
                    log::warn!(
                        "readonly exclude source metadata post-fix failed: {}",
                        source_path
                    );
                }
                if !self.ensure_writable_mapped_directory(&excluded_child, self.app_uid) {
                    log::warn!(
                        "readonly exclude target metadata post-fix failed: {}",
                        excluded_child
                    );
                }
                log::info!("readonly exclude restored {}", excluded_child);
            }
        }
        // 交由 scoped FUSE 执行的只读规则同样属于"已强制"，不能按未生效处理，
        // 否则会误报 readonly 规则放行写入。
        is_any_mounted || delegated_to_scoped_fuse
    }

    fn resolve_read_only_exclusion_rules(
        &self,
        excluded_real_paths: &[String],
        storage_path: &str,
    ) -> Vec<String> {
        let mut resolved_rules = Vec::with_capacity(excluded_real_paths.len());
        for path in excluded_real_paths {
            let Some(resolved) =
                self.resolve_storage_path_for_mount(path, storage_path, "readonly exclude")
            else {
                continue;
            };
            resolved_rules.push(resolved);
        }
        resolved_rules
    }

    fn resolve_read_only_source(&self, relative: &str, source_roots: &[String]) -> Option<String> {
        for root in source_roots {
            let candidate = paths::join(root, relative);
            if fs::is_directory(&candidate) {
                return Some(candidate);
            }
        }

        let data_media_source = paths::join(
            &paths::data_media_user_root_for_user(self.user_id),
            relative,
        );
        if !self.ensure_writable_mapped_directory(&data_media_source, self.app_uid) {
            return None;
        }
        Some(data_media_source)
    }

    fn resolve_concrete_storage_paths(
        &self,
        paths_in: &[String],
        storage_path: &str,
        source_name: &str,
        wildcard_source_name: &str,
    ) -> Vec<String> {
        let mut resolved_paths: Vec<String> = Vec::with_capacity(paths_in.len());
        for path in paths_in {
            let Some(resolved) =
                self.resolve_storage_path_for_mount(path, storage_path, source_name)
            else {
                continue;
            };
            if paths::contains_wildcards(&resolved) {
                let matched_paths = self.concrete_wildcard_mount_matches(&resolved, storage_path);
                if !matched_paths.is_empty() {
                    log::warn!(
                        "fallback {} wildcard to concrete matches: {} -> {} dirs",
                        wildcard_source_name,
                        resolved,
                        matched_paths.len()
                    );
                    resolved_paths.extend(matched_paths);
                    continue;
                }
                if let Some(fallback) = concrete_mount_fallback_parent(&resolved, storage_path) {
                    log::warn!(
                        "fallback {} wildcard to concrete parent: {} -> {}",
                        wildcard_source_name,
                        resolved,
                        fallback
                    );
                    resolved_paths.push(fallback);
                } else {
                    log::warn!(
                        "skip {} (wildcard has no safe concrete parent): {}",
                        wildcard_source_name,
                        resolved
                    );
                }
                continue;
            }
            resolved_paths.push(resolved);
        }

        paths::sort_dedup_paths_case_insensitive(&mut resolved_paths);
        resolved_paths
    }

    fn resolve_excluded_storage_mount_paths(
        &self,
        excluded_real_paths: &[String],
        storage_path: &str,
    ) -> Vec<String> {
        let mut resolved_paths: Vec<String> = Vec::with_capacity(excluded_real_paths.len());
        for path in excluded_real_paths {
            let Some(resolved) = self.resolve_storage_path_for_mount(path, storage_path, "exclude")
            else {
                continue;
            };
            if paths::contains_wildcards(&resolved) {
                let matched_paths = self.concrete_wildcard_mount_matches(&resolved, storage_path);
                if !matched_paths.is_empty() {
                    log::warn!(
                        "fallback exclude wildcard to concrete matches: {} -> {} dirs",
                        resolved,
                        matched_paths.len()
                    );
                    resolved_paths.extend(matched_paths);
                } else {
                    log::warn!(
                        "skip exclude mount (wildcard has no concrete directory match): {}",
                        resolved
                    );
                }
                continue;
            }
            resolved_paths.push(resolved);
        }

        paths::sort_dedup_paths_longest_first_case_insensitive(&mut resolved_paths);
        resolved_paths
    }

    fn resolve_storage_path_for_mount(
        &self,
        path: &str,
        storage_path: &str,
        source_name: &str,
    ) -> Option<String> {
        let resolved =
            self.resolve_user_path(&self.resolve_placeholders(&self.normalize_path(path)));
        if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
            return None;
        }
        if paths::eq_ignore_case(&resolved, storage_path) {
            log::warn!(
                "skip {} (whole storage not supported): {}",
                source_name,
                resolved
            );
            return None;
        }
        if !paths::is_child(&resolved, storage_path) {
            log::warn!("skip {} (not under storage): {}", source_name, resolved);
            return None;
        }
        Some(resolved)
    }

    fn read_only_shadowed_path_mappings(
        &self,
        resolved_mappings: &[PathMapping],
        read_only_paths: &[String],
        storage_path: &str,
        source_name: &str,
    ) -> Vec<PathMapping> {
        if resolved_mappings.is_empty() || read_only_paths.is_empty() {
            return Vec::new();
        }
        let (included_read_only_paths, _) = paths::split_exclusion_rules(read_only_paths);
        let read_only_roots = self.resolve_concrete_storage_paths(
            &included_read_only_paths,
            storage_path,
            source_name,
            source_name,
        );
        let mut shadowed = Vec::new();
        for mapping in resolved_mappings {
            if read_only_roots
                .iter()
                .any(|root| paths::is_same_or_child(&mapping.request_path, root))
            {
                shadowed.push(mapping.clone());
            }
        }
        shadowed
    }
}

/// 两个路径是否指向同一个挂载对象。
///
/// 用作自有私有目录恢复的幂等判据：bind 生效后 `stat` 返回的是源侧的 dev+ino，
/// 与源路径完全一致；未 bind 时两侧分别是存储视图与系统 FUSE 锚点，dev 必然不同。
fn same_mount_identity(left: &str, right: &str) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let (Ok(left_meta), Ok(right_meta)) = (std::fs::metadata(left), std::fs::metadata(right))
    else {
        return false;
    };
    left_meta.dev() == right_meta.dev() && left_meta.ino() == right_meta.ino()
}
