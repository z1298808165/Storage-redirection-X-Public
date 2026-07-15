use super::map::PathMappingApplyOptions;
use super::{MountPlanner, concrete_mount_fallback_parent};
use crate::domain::PathMapping;
use crate::platform::{fs, module_paths, paths};
use libc::{MNT_DETACH, umount2};
use std::ffi::CString;

impl MountPlanner {
    fn prepare_real_storage_anchor(&mut self, storage_path: &str) -> Option<String> {
        self.real_storage_anchor = None;
        let real_storage_anchor_root = module_paths::REAL_STORAGE_TMP_DIR;
        let real_storage_anchor = paths::join(real_storage_anchor_root, &self.user_id.to_string());

        if self.storage_root_is_already_redirected(storage_path) {
            if self.real_storage_anchor_is_usable(&real_storage_anchor) {
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
                "real storage anchor unavailable after redirect, fallback backend pkg={} storage={} anchor={} target={}",
                self.package_name,
                storage_path,
                real_storage_anchor,
                self.redirect_target
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
            if self.bind_mount(&source_candidate, &real_storage_anchor, true) {
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

    fn real_storage_anchor_is_usable(&self, real_storage_anchor: &str) -> bool {
        if !fs::is_directory(real_storage_anchor) {
            return false;
        }
        if mount_source_for_target(real_storage_anchor).is_none() {
            return false;
        }
        ["Android", "Download", "DCIM"]
            .iter()
            .any(|child| fs::is_directory(&paths::join(real_storage_anchor, child)))
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

    fn storage_root_is_already_redirected(&self, storage_path: &str) -> bool {
        if self.redirect_target.is_empty() {
            return false;
        }
        let redirect_backend = self.to_data_media_backend_path(&self.redirect_target);
        if redirect_backend.is_empty() {
            return false;
        }
        mount_source_for_target(storage_path)
            .map(|source| {
                paths::is_same_or_child(&redirect_backend, &source)
                    || mountinfo_root_matches_data_backend(&source, &redirect_backend)
            })
            .unwrap_or(false)
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

        if !allowed_real_paths.is_empty() {
            let resolved_paths = self.resolve_concrete_storage_paths(
                allowed_real_paths,
                &storage_path,
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

                let Some(relative) = paths::relative_child_path(&allowed_path, &storage_path)
                else {
                    continue;
                };

                if !self.ensure_directory_exists(&allowed_path, true) {
                    log::warn!("mkdir allow failed: {}", allowed_path);
                    continue;
                }

                let mut is_restored_allowed_path = false;
                let source_candidates = build_allowed_real_source_candidates(
                    &real_storage_anchor,
                    &data_media_root,
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
                        break;
                    }
                }
            }
        }

        if !excluded_real_paths.is_empty() {
            let resolved_paths =
                self.resolve_excluded_storage_mount_paths(excluded_real_paths, &storage_path);

            for excluded_path in resolved_paths {
                let Some(relative) = paths::relative_child_path(&excluded_path, &storage_path)
                else {
                    continue;
                };

                if !fs::is_directory(&excluded_path) {
                    log::info!("exclude mount target missing, skip bind: {}", excluded_path);
                    continue;
                }

                let sandbox_source = paths::join(&resolved_target, relative);
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
            let is_any_applied = self.apply_resolved_path_mappings(
                &namespace_mappings,
                &storage_path,
                &mapping_source_roots,
                read_only_paths,
                excluded_real_paths,
                PathMappingApplyOptions {
                    should_chown_current_dirs: true,
                    should_create_missing_request_path: true,
                    should_use_existing_target_source_only: false,
                },
            );
            let _ = is_any_applied;
        }

        let read_only_shadowed_mappings = self.read_only_shadowed_path_mappings(
            &namespace_mappings,
            read_only_paths,
            &storage_path,
            "readonly map restore",
        );
        if !read_only_paths.is_empty() {
            let _ = self.apply_read_only_paths(
                read_only_paths,
                excluded_real_paths,
                &namespace_mappings,
                &storage_path,
                &mapping_source_roots,
                scoped_fuse_roots,
            );
            if !read_only_shadowed_mappings.is_empty() {
                let is_any_restored = self.apply_resolved_path_mappings(
                    &read_only_shadowed_mappings,
                    &storage_path,
                    &mapping_source_roots,
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

            let mut resolved_sandboxed_paths = Vec::with_capacity(sandboxed_paths.len());
            for path in sandboxed_paths {
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
                if !fs::is_directory(&sandboxed_path) {
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

    fn apply_read_only_paths(
        &self,
        read_only_paths: &[String],
        excluded_real_paths: &[String],
        path_mappings: &[PathMapping],
        storage_path: &str,
        source_roots: &[String],
        scoped_fuse_roots: &[String],
    ) -> bool {
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
                log::info!("readonly mount kept with scoped fuse fallback: {}", path);
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
            let Some(source_path) = self.resolve_read_only_source(relative, source_roots) else {
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

            let Some(source_path) = self.resolve_read_only_source(relative, source_roots) else {
                log::warn!("readonly source missing: {}", read_only_path);
                continue;
            };
            self.ensure_read_only_tree_accessible(&source_path);

            let mut is_read_only_mounted = false;
            let preserve_data_media_backend = restored_excluded_children
                .iter()
                .any(|excluded_child| paths::is_child(excluded_child, read_only_path));
            if preserve_data_media_backend {
                let _ = self.bind_read_write_mount_with_storage_aliases(
                    &source_path,
                    read_only_path,
                    true,
                    super::PrimaryMountFailure::StopCurrentTarget,
                    Some("readonly primary mount failed"),
                    Some("readonly alias mount failed"),
                    Some("readonly alias ok"),
                    Some(&mut is_read_only_mounted),
                );
                if is_read_only_mounted {
                    let _ = self.ensure_read_only_directory_metadata(read_only_path);
                }
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
            let Some(source_path) = self.resolve_read_only_source(relative, source_roots) else {
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
        is_any_mounted
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

fn path_overlaps_mapping_request(path: &str, path_mappings: &[PathMapping]) -> bool {
    path_mappings
        .iter()
        .any(|mapping| paths::matches(&mapping.request_path, path, true))
}

fn path_shadows_mapping_request(path: &str, path_mappings: &[PathMapping]) -> bool {
    path_mappings
        .iter()
        .any(|mapping| paths::is_same_or_child(&mapping.request_path, path))
}

fn collect_restored_read_only_excluded_children(
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

fn build_mapping_source_roots(
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

fn build_allowed_real_source_candidates(
    real_storage_anchor: &Option<String>,
    data_media_root: &str,
    relative: &str,
) -> Vec<String> {
    let backend_source = paths::join(data_media_root, relative);
    let mut candidates = Vec::with_capacity(2);
    candidates.push(backend_source.clone());
    if let Some(anchor) = real_storage_anchor {
        let anchor_source = paths::join(anchor, relative);
        if !paths::eq_ignore_case(&anchor_source, &backend_source) {
            candidates.push(anchor_source);
        }
    }
    candidates
}

fn namespace_mappings_outside_scoped_fuse(
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

fn is_covered_by_scoped_fuse_mount(path: &str, scoped_fuse_roots: &[String]) -> bool {
    scoped_fuse_roots
        .iter()
        .any(|root| paths::eq_ignore_case(path, root) || paths::is_child(path, root))
}

fn mount_source_for_target(target: &str) -> Option<String> {
    let content = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    mount_source_for_target_from_mountinfo(&content, target)
}

fn mount_source_for_target_from_mountinfo(content: &str, target: &str) -> Option<String> {
    let normalized_target = paths::normalize(target);
    content
        .lines()
        .filter_map(parse_mountinfo_source_and_target)
        .filter(|(_, mount_target)| paths::eq_ignore_case(mount_target, &normalized_target))
        .max_by_key(|(_, mount_target)| mount_target.len())
        .map(|(source, _)| source)
}

fn parse_mountinfo_source_and_target(line: &str) -> Option<(String, String)> {
    let separator = line.find(" - ")?;
    let before_separator = &line[..separator];
    let after_separator = &line[separator + 3..];
    let mut before_fields = before_separator.split_whitespace();
    let _id = before_fields.next()?;
    let _parent = before_fields.next()?;
    let _major_minor = before_fields.next()?;
    let root = unescape_mountinfo_field(before_fields.next()?);
    let target = paths::normalize(&unescape_mountinfo_field(before_fields.next()?));
    let mut after_fields = after_separator.split_whitespace();
    let _fs_type = after_fields.next()?;
    let _source = after_fields.next()?;
    Some((root, target))
}

fn mountinfo_root_matches_data_backend(root: &str, backend: &str) -> bool {
    paths::eq_ignore_case(root, backend)
        || root
            .strip_prefix("/media/")
            .map(|suffix| format!("/data/media/{suffix}"))
            .map(|source| paths::is_same_or_child(backend, &source))
            .unwrap_or(false)
}

fn detach_mount_if_present(target: &str) {
    if mount_source_for_target(target).is_none() {
        return;
    }
    let Ok(c_target) = CString::new(target) else {
        return;
    };
    let ret = unsafe { umount2(c_target.as_ptr(), MNT_DETACH) };
    if ret != 0 {
        log::warn!(
            "real storage anchor detach failed target={} errno={}",
            target,
            unsafe { *libc::__errno() }
        );
    }
}

fn unescape_mountinfo_field(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1..index + 4]
                .iter()
                .all(|byte| (b'0'..=b'7').contains(byte))
        {
            let code = (bytes[index + 1] - b'0') * 64
                + (bytes[index + 2] - b'0') * 8
                + (bytes[index + 3] - b'0');
            output.push(code as char);
            index += 4;
        } else {
            output.push(bytes[index] as char);
            index += 1;
        }
    }
    output
}
