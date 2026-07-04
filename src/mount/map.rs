use super::MountPlanner;
use crate::domain::{PathMapping, sort_path_mappings_shortest_request_first};
use crate::platform::{fs, module_paths, paths};

impl MountPlanner {
    pub(super) fn resolve_path_mappings(
        &self,
        path_mappings: &[PathMapping],
        storage_path: &str,
    ) -> Vec<PathMapping> {
        let mut resolved = Vec::with_capacity(path_mappings.len());

        for mapping in path_mappings {
            let mut current_path = self.resolve_user_path(
                &self.resolve_placeholders(&self.normalize_path(&mapping.request_path)),
            );
            let mut target_path = self.resolve_user_path(
                &self.resolve_placeholders(&self.normalize_path(&mapping.final_path)),
            );

            if current_path.is_empty() || target_path.is_empty() {
                continue;
            }
            if paths::has_unsafe_segments(&current_path) || paths::has_unsafe_segments(&target_path)
            {
                continue;
            }

            if !paths::is_absolute(&current_path) {
                current_path = self.normalize_path(&paths::join(storage_path, &current_path));
            }
            if !paths::is_absolute(&target_path) {
                target_path = self.normalize_path(&paths::join(storage_path, &target_path));
            }

            if paths::eq_ignore_case(&current_path, storage_path)
                || paths::eq_ignore_case(&target_path, storage_path)
            {
                log::warn!(
                    "skip map (whole storage not supported): cur={} tgt={}",
                    current_path,
                    target_path
                );
                continue;
            }

            if !paths::is_child(&current_path, storage_path)
                || !paths::is_child(&target_path, storage_path)
            {
                log::warn!(
                    "skip map (not under storage): cur={} tgt={}",
                    current_path,
                    target_path
                );
                continue;
            }

            if paths::is_android_data_or_obb_path(&target_path) {
                log::warn!("skip map (private app target): tgt={}", target_path);
                continue;
            }

            if paths::eq_ignore_case(&current_path, &target_path) {
                continue;
            }

            resolved.push(PathMapping::new(current_path, target_path));
        }

        sort_path_mappings_shortest_request_first(&mut resolved);

        resolved
    }

    pub(super) fn apply_resolved_path_mappings(
        &self,
        resolved_mappings: &[PathMapping],
        storage_path: &str,
        target_source_roots: &[String],
        read_only_paths: &[String],
        excluded_real_paths: &[String],
        should_chown_current_dirs: bool,
        should_create_missing_request_path: bool,
        should_use_existing_target_source_only: bool,
        is_any_applied_out: Option<&mut bool>,
    ) -> bool {
        let mut is_any_applied = false;

        for mapping in resolved_mappings {
            let Some(target_relative) =
                paths::relative_child_path(&mapping.final_path, storage_path)
            else {
                continue;
            };

            let Some((target_source, should_fix_target_permissions)) = self
                .resolve_mapping_target_source(
                    target_relative,
                    target_source_roots,
                    should_use_existing_target_source_only,
                )
            else {
                continue;
            };
            if should_fix_target_permissions {
                self.ensure_shared_mapping_parent_chain(&target_source);
            }

            if !self.ensure_mapping_request_mount_point(
                &mapping.request_path,
                storage_path,
                should_chown_current_dirs,
                should_create_missing_request_path,
            ) {
                log::warn!("map mount point unavailable: {}", mapping.request_path);
                continue;
            }

            let mut is_current_path_mounted = false;
            let _ = self.bind_overlay_mount_with_storage_aliases(
                &target_source,
                &mapping.request_path,
                true,
                super::PrimaryMountFailure::StopCurrentTarget,
                None,
                Some("map alias mount failed"),
                Some("map alias ok"),
                Some(&mut is_current_path_mounted),
            );

            if is_current_path_mounted {
                is_any_applied = true;
                log::info!("map {} -> {}", mapping.request_path, mapping.final_path);

                if self.is_mapping_read_only(
                    mapping,
                    read_only_paths,
                    excluded_real_paths,
                    storage_path,
                ) {
                    let mut is_read_only_mounted = false;
                    let _ = self.bind_read_only_mount_with_storage_aliases(
                        &target_source,
                        &mapping.request_path,
                        true,
                        super::PrimaryMountFailure::StopCurrentTarget,
                        Some("map readonly primary mount failed"),
                        Some("map readonly alias mount failed"),
                        Some("map readonly alias ok"),
                        Some(&mut is_read_only_mounted),
                    );
                    if is_read_only_mounted {
                        log::info!(
                            "map readonly {} -> {}",
                            mapping.request_path,
                            mapping.final_path
                        );
                    }
                }
            }
        }

        if let Some(out) = is_any_applied_out {
            *out = is_any_applied;
        }
        true
    }

    fn resolve_mapping_target_source(
        &self,
        target_relative: &str,
        target_source_roots: &[String],
        should_use_existing_target_source_only: bool,
    ) -> Option<(String, bool)> {
        for root in target_source_roots {
            let candidate = paths::join(root, target_relative);
            if fs::is_directory(&candidate) {
                return Some((candidate, false));
            }
        }

        // Existing public mapping targets should keep the FUSE-backed source so
        // callers read through Android's storage permission model. Only create a
        // /data/media backend when the target directory does not exist yet.
        let target_data_media = paths::join(
            &paths::data_media_user_root_for_user(self.user_id),
            target_relative,
        );
        if should_use_existing_target_source_only && !fs::is_directory(&target_data_media) {
            log::warn!("map target missing: {}", target_data_media);
            return None;
        }
        if !self.ensure_writable_mapped_directory(&target_data_media, self.app_uid) {
            log::warn!("map target missing and mkdir failed: {}", target_data_media);
            return None;
        }
        Some((target_data_media, true))
    }

    fn ensure_mapping_request_mount_point(
        &self,
        request_path: &str,
        storage_path: &str,
        should_chown_current_dirs: bool,
        should_prefer_redirect_fallback: bool,
    ) -> bool {
        if !should_prefer_redirect_fallback {
            return fs::is_directory(request_path);
        }

        let uid = if should_chown_current_dirs {
            self.app_uid
        } else {
            -1
        };
        if fs::create_directory(request_path, uid) {
            return true;
        }

        let Some(current_relative) = paths::relative_child_path(request_path, storage_path) else {
            return false;
        };
        let current_fallback = if should_prefer_redirect_fallback {
            paths::join(&self.redirect_target, current_relative)
        } else {
            paths::join(
                &paths::data_media_user_root_for_user(self.user_id),
                current_relative,
            )
        };
        let current_fallback = self.normalize_path(&current_fallback);
        if self.ensure_directory_exists(&current_fallback, should_chown_current_dirs) {
            log::warn!(
                "map mount point prepared via backend fallback request={} backend={}",
                request_path,
                current_fallback
            );
            return true;
        }

        false
    }

    pub(super) fn is_mapping_read_only(
        &self,
        mapping: &PathMapping,
        read_only_paths: &[String],
        excluded_real_paths: &[String],
        storage_path: &str,
    ) -> bool {
        self.is_path_read_only_for_mount(
            &mapping.final_path,
            read_only_paths,
            excluded_real_paths,
            storage_path,
        )
    }

    pub(super) fn is_path_read_only_for_mount(
        &self,
        path: &str,
        read_only_paths: &[String],
        excluded_real_paths: &[String],
        storage_path: &str,
    ) -> bool {
        let (included_read_only_paths, excluded_read_only_paths) =
            paths::split_exclusion_rules(read_only_paths);
        let read_only_excluded_paths = paths::overlapping_exclusion_rules(
            &included_read_only_paths,
            &excluded_read_only_paths,
        );
        if excluded_real_paths
            .iter()
            .chain(read_only_excluded_paths.iter())
            .filter_map(|rule| self.resolve_mount_rule_path(rule, storage_path))
            .any(|excluded| paths::matches(&excluded, path, true))
        {
            return false;
        }

        included_read_only_paths
            .iter()
            .filter_map(|rule| self.resolve_mount_rule_path(rule, storage_path))
            .any(|read_only| paths::matches(&read_only, path, true))
    }

    pub fn can_record_read_only_mapping_denials(
        &self,
        path_mappings: &[PathMapping],
        read_only_paths: &[String],
        excluded_real_paths: &[String],
    ) -> bool {
        if path_mappings.is_empty() || read_only_paths.is_empty() {
            return false;
        }

        let storage_path = paths::storage_user_root_for_user(self.user_id);
        let real_storage_anchor = paths::join(
            module_paths::REAL_STORAGE_TMP_DIR,
            &self.user_id.to_string(),
        );
        let resolved_mappings = self.resolve_path_mappings(path_mappings, &storage_path);
        let mut has_read_only_mapping = false;

        for mapping in resolved_mappings {
            if !self.is_mapping_read_only(
                &mapping,
                read_only_paths,
                excluded_real_paths,
                &storage_path,
            ) {
                continue;
            }
            has_read_only_mapping = true;

            let Some(target_relative) =
                paths::relative_child_path(&mapping.final_path, &storage_path)
            else {
                log::warn!(
                    "recordable map fallback unavailable, target outside storage: {}",
                    mapping.final_path
                );
                return false;
            };
            let anchor_source = paths::join(&real_storage_anchor, target_relative);
            if !fs::is_directory(&anchor_source) {
                log::warn!(
                    "recordable map fallback unavailable, anchor source missing: {}",
                    anchor_source
                );
                return false;
            }
        }

        has_read_only_mapping
    }

    fn resolve_mount_rule_path(&self, path: &str, storage_path: &str) -> Option<String> {
        let mut resolved =
            self.resolve_user_path(&self.resolve_placeholders(&self.normalize_path(path)));
        if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
            return None;
        }
        if !paths::is_absolute(&resolved) {
            resolved = self.normalize_path(&paths::join(storage_path, &resolved));
        }
        if paths::eq_ignore_case(&resolved, storage_path)
            || !paths::is_child(&resolved, storage_path)
        {
            return None;
        }
        Some(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::MountPlanner;
    use crate::domain::PathMapping;

    #[test]
    fn mapping_is_read_only_when_target_hits_read_only_path() {
        let planner = MountPlanner::new("com.example", 10123, "", "/data/local/tmp/srx", false);
        let mapping = PathMapping::new(
            "/storage/emulated/0/DCIM/MyApp".to_string(),
            "/storage/emulated/0/Pictures/MyApp".to_string(),
        );

        assert!(planner.is_mapping_read_only(
            &mapping,
            &["/storage/emulated/0/Pictures/MyApp".to_string()],
            &[],
            "/storage/emulated/0",
        ));
    }

    #[test]
    fn mapping_read_only_respects_excluded_target() {
        let planner = MountPlanner::new("com.example", 10123, "", "/data/local/tmp/srx", false);
        let mapping = PathMapping::new(
            "/storage/emulated/0/DCIM/MyApp".to_string(),
            "/storage/emulated/0/Pictures/MyApp".to_string(),
        );

        assert!(!planner.is_mapping_read_only(
            &mapping,
            &["/storage/emulated/0/Pictures/MyApp".to_string()],
            &["/storage/emulated/0/Pictures/MyApp".to_string()],
            "/storage/emulated/0",
        ));
    }

    #[test]
    fn mapping_request_under_read_only_parent_uses_writable_excluded_target() {
        let planner = MountPlanner::new("com.example", 10123, "", "/data/local/tmp/srx", false);
        let mapping = PathMapping::new(
            "/storage/emulated/0/Download/QQ".to_string(),
            "/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
        );

        assert!(!planner.is_mapping_read_only(
            &mapping,
            &[
                "/storage/emulated/0/Download".to_string(),
                "!/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
            ],
            &[],
            "/storage/emulated/0",
        ));
    }

    #[test]
    fn mapping_target_inherits_parent_read_only_without_exclusion() {
        let planner = MountPlanner::new("com.example", 10123, "", "/data/local/tmp/srx", false);
        let mapping = PathMapping::new(
            "/storage/emulated/0/Download/QQ".to_string(),
            "/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
        );

        assert!(planner.is_mapping_read_only(
            &mapping,
            &["/storage/emulated/0/Download".to_string()],
            &[],
            "/storage/emulated/0",
        ));
    }

    #[test]
    fn mapping_request_read_only_rule_does_not_make_target_read_only() {
        let planner = MountPlanner::new("com.example", 10123, "", "/data/local/tmp/srx", false);
        let mapping = PathMapping::new(
            "/storage/emulated/0/Download/QQ".to_string(),
            "/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
        );

        assert!(!planner.is_mapping_read_only(
            &mapping,
            &["/storage/emulated/0/Download/QQ".to_string()],
            &[],
            "/storage/emulated/0",
        ));
    }

    #[test]
    fn map_only_missing_request_path_is_not_materialized_in_public_storage() {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "srx_map_mount_point_{}_{}",
            std::process::id(),
            millis
        ));
        let storage = root.join("storage");
        std::fs::create_dir_all(&storage).expect("create storage root");
        let storage_path = storage.to_string_lossy().replace('\\', "/");
        let request_path = format!("{storage_path}/Download/MissingRequest");

        let planner = MountPlanner::new("com.example", 10123, "", "/data/local/tmp/srx", false);
        assert!(!planner.ensure_mapping_request_mount_point(
            &request_path,
            &storage_path,
            false,
            false,
        ));
        assert!(!std::path::Path::new(&request_path).exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_path_mappings_skips_android_private_targets() {
        let planner = MountPlanner::new("com.example", 10123, "", "/data/local/tmp/srx", false);
        let mappings = vec![
            PathMapping::new(
                "Download/App".to_string(),
                "Android/data/com.example/files".to_string(),
            ),
            PathMapping::new(
                "Download/Obb".to_string(),
                "/storage/emulated/0/Android/obb/com.example".to_string(),
            ),
            PathMapping::new(
                "Download/Media".to_string(),
                "Android/media/com.example/files".to_string(),
            ),
            PathMapping::new("Download/Public".to_string(), "Pictures/Public".to_string()),
        ];

        let resolved = planner.resolve_path_mappings(&mappings, "/storage/emulated/0");

        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|mapping| {
            mapping.request_path == "/storage/emulated/0/Download/Media"
                && mapping.final_path == "/storage/emulated/0/Android/media/com.example/files"
        }));
        assert!(resolved.iter().any(|mapping| {
            mapping.request_path == "/storage/emulated/0/Download/Public"
                && mapping.final_path == "/storage/emulated/0/Pictures/Public"
        }));
    }
}
