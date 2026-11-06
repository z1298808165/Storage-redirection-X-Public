use super::MountPlanner;
use crate::domain::{
    PathMapping, expand_namespace_path_mappings, sort_path_mappings_shortest_request_first,
};
use crate::platform::{fs, paths};

pub(super) struct PathMappingApplyOptions {
    pub(super) should_chown_current_dirs: bool,
    pub(super) should_create_missing_request_path: bool,
    pub(super) should_use_existing_target_source_only: bool,
}

// 挂载点只要求请求路径存在目录或文件节点，具体类型由映射目标决定。
fn mapping_mount_point_exists(path: &str) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_dir() || metadata.is_file())
        .unwrap_or(false)
}

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

            if current_path == "/" || target_path == "/" {
                log::warn!(
                    "skip map (root path not supported): cur={} tgt={}",
                    current_path,
                    target_path
                );
                continue;
            }

            if !paths::is_safe_namespace_path(&current_path)
                || !paths::is_safe_namespace_path(&target_path)
            {
                log::warn!(
                    "skip map (unsafe namespace path): cur={} tgt={}",
                    current_path,
                    target_path
                );
                continue;
            }

            if paths::is_application_private_root(&current_path) {
                log::warn!(
                    "skip map (application private request root is not allowed): {}",
                    current_path
                );
                continue;
            }

            if paths::eq_ignore_case(&current_path, &target_path) {
                continue;
            }

            resolved.push(PathMapping::new(current_path, target_path));
        }

        let mut resolved = expand_namespace_path_mappings(&resolved);

        sort_path_mappings_shortest_request_first(&mut resolved);

        resolved
    }

    // quality-allow(lint-suppression): 该方法需同时接收多组独立路径集合与映射选项，拆分反而降低可读性，故保留当前参数数量。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_resolved_path_mappings(
        &self,
        resolved_mappings: &[PathMapping],
        storage_path: &str,
        target_source_roots: &[String],
        preferred_target_source_roots: &[String],
        read_only_paths: &[String],
        excluded_real_paths: &[String],
        options: PathMappingApplyOptions,
    ) -> bool {
        let mut is_any_applied = false;

        for mapping in resolved_mappings {
            let Some((target_source, should_fix_target_permissions)) = self
                .resolve_mapping_target_source_path(
                    &mapping.final_path,
                    storage_path,
                    target_source_roots,
                    options.should_use_existing_target_source_only,
                )
            else {
                continue;
            };
            if should_fix_target_permissions {
                if !self.ensure_writable_mapped_directory(&target_source, self.app_uid) {
                    log::warn!("map target permission fix failed: {}", target_source);
                    continue;
                }
                self.ensure_shared_mapping_parent_chain(&target_source);
            }
            // 与 should_fix_target_permissions 无关：映射目标目录必须归调用应用所有
            // （裸权限访问路径下的必要状态，FUSE 虚拟化设备无副作用），且元数据判定
            // 在锚点路径上会漏判，故目标源解析成功后总是执行。
            if !self.ensure_app_owned_mapping_target(&target_source, self.app_uid) {
                log::warn!("map target app ownership fix failed: {}", target_source);
                continue;
            }

            if !self.ensure_mapping_request_mount_point(
                &mapping.request_path,
                storage_path,
                options.should_chown_current_dirs,
                options.should_create_missing_request_path,
            ) {
                log::warn!("map mount point unavailable: {}", mapping.request_path);
                continue;
            }

            let mut is_current_path_mounted = false;
            if self.is_storage_path(&mapping.request_path, storage_path) {
                // 别名挂载由 bind_overlay_mount_with_storage_aliases 内部完成（它带
                // should_skip_self_shadowing_alias 等保护）。此前这里额外逐别名调用
                // bind_mount_overlay，会因为源与别名同 inode 而走「已挂载」捷径并返回
                // true，看起来全部成功但实际没有建立任何挂载——探针据此误报 15/15 成功，
                // 而应用 mountinfo 里只有后端别名一条。不要再逐别名自行挂载。
                let _ = self.bind_overlay_mount_with_storage_aliases(
                    &target_source,
                    &mapping.request_path,
                    true,
                    super::PrimaryMountFailure::ContinueAliases,
                    Some("map primary mount failed"),
                    Some("map alias mount failed"),
                    Some("map alias ok"),
                    Some(&mut is_current_path_mounted),
                );
                log::warn!(
                    "map primary result request={} target_source={} mounted={} alias_count={}",
                    mapping.request_path,
                    target_source,
                    is_current_path_mounted,
                    self.expand_storage_alias_paths(&mapping.request_path).len()
                );
            } else {
                let recursive = std::fs::metadata(&target_source)
                    .map(|metadata| metadata.is_dir())
                    .unwrap_or(true);
                is_current_path_mounted =
                    self.bind_mount_overlay(&target_source, &mapping.request_path, recursive);
            }

            if is_current_path_mounted {
                is_any_applied = true;
                log::info!("map {} -> {}", mapping.request_path, mapping.final_path);

                if preferred_target_source_roots
                    .iter()
                    .any(|root| paths::is_same_or_child(&mapping.final_path, root))
                    && self.ensure_directory_exists(&mapping.final_path, false)
                {
                    let mut is_target_path_mounted = false;
                    let _ = self.bind_overlay_mount_with_storage_aliases(
                        &target_source,
                        &mapping.final_path,
                        true,
                        super::PrimaryMountFailure::ContinueAliases,
                        Some("map target primary mount failed"),
                        Some("map target alias mount failed"),
                        Some("map target alias ok"),
                        Some(&mut is_target_path_mounted),
                    );
                    if is_target_path_mounted {
                        log::info!(
                            "map target restored {} -> {}",
                            mapping.final_path,
                            target_source
                        );
                    }
                }

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
                        super::PrimaryMountFailure::ContinueAliases,
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

        is_any_applied
    }

    fn resolve_mapping_target_source(
        &self,
        target_relative: &str,
        target_source_roots: &[String],
        should_use_existing_target_source_only: bool,
    ) -> Option<(String, bool)> {
        let target_data_media = paths::join(
            &paths::data_media_user_root_for_user(self.user_id),
            target_relative,
        );
        for root in target_source_roots {
            let candidate = paths::join(root, target_relative);
            if fs::is_directory(&candidate) {
                // 元数据判定必须落在真实后端路径上：锚点路径（tmp/real_storage）不是
                // /data/media 子路径，直接判定会得到 false，导致映射目标目录的属主与
                // 模式从未被修正——在以真实后端为访问路径的设备上（锚点为 /data/media
                // 直绑、或应用视图本身按裸权限检查），目标目录会保持创建时的 restrictive
                // 模式，应用列目录/按名访问全部被拒。
                let metadata_path = self
                    .metadata_operations_path(&candidate)
                    .unwrap_or_else(|| candidate.clone());
                let should_fix_permissions =
                    mapping_target_source_needs_app_writable_metadata(&metadata_path, self.user_id);
                return Some((candidate, should_fix_permissions));
            }
        }

        // 已有公共映射目标应保留 FUSE 后端来源，使调用方通过 Android 存储权限模型读取。
        // 仅在目标目录尚不存在时创建 /data/media 后端。
        if should_use_existing_target_source_only && !fs::is_directory(&target_data_media) {
            log::warn!("map target missing: {}", target_data_media);
            return None;
        }
        if !self.ensure_real_public_directory_exists(&target_data_media) {
            log::warn!("map target missing and mkdir failed: {}", target_data_media);
            return None;
        }

        for root in target_source_roots {
            if paths::eq_ignore_case(root, &paths::data_media_user_root_for_user(self.user_id)) {
                continue;
            }
            let candidate = paths::join(root, target_relative);
            if fs::is_directory(&candidate) {
                return Some((candidate, false));
            }
        }

        Some((target_data_media, true))
    }

    /// 共享存储下的映射需要切换到对应真实后端；其它绝对路径直接作为
    /// bind mount 源使用。后者使 `/data/user`、`/data/data` 等路径不再被
    /// 错误地拼接到 `/data/media`。
    fn resolve_mapping_target_source_path(
        &self,
        final_path: &str,
        storage_path: &str,
        target_source_roots: &[String],
        should_use_existing_target_source_only: bool,
    ) -> Option<(String, bool)> {
        if let Some(relative) = paths::relative_child_path(final_path, storage_path) {
            return self.resolve_mapping_target_source(
                relative,
                target_source_roots,
                should_use_existing_target_source_only,
            );
        }

        if std::fs::metadata(final_path).is_err() && !self.ensure_directory_exists(final_path, true)
        {
            log::warn!("map target directory unavailable: {}", final_path);
            return None;
        }
        let metadata = std::fs::metadata(final_path).ok()?;
        if !metadata.is_dir() && !metadata.is_file() {
            return None;
        }
        Some((final_path.to_string(), false))
    }

    fn is_storage_path(&self, path: &str, storage_path: &str) -> bool {
        paths::is_same_or_child(path, storage_path)
    }

    fn ensure_mapping_request_mount_point(
        &self,
        request_path: &str,
        storage_path: &str,
        should_chown_current_dirs: bool,
        should_prefer_redirect_fallback: bool,
    ) -> bool {
        if !should_prefer_redirect_fallback {
            return mapping_mount_point_exists(request_path);
        }

        if mapping_mount_point_exists(request_path) {
            return true;
        }

        // 非共享存储目标（例如当前应用的 `/data/user` 目录）同样不创建：请求路径是
        // 映射视图里的挂载点，缺它就说明这条映射本轮无法成立，补一个空目录只是把
        // 「规则没生效」伪装成「规则已生效」。
        if !paths::is_same_or_child(request_path, storage_path) {
            log::warn!(
                "map mount point missing outside storage, skip bind: {}",
                request_path
            );
            return false;
        }

        // 共享存储上的请求路径**永远不创建**，创建它等于在用户可见的公共目录里凭空
        // 留一个空目录（用户配置映射/沙盒就是为了不让它出现）。bind 挂载点必须存在于
        // 目标命名空间，所以这条路径缺失时 namespace bind 在语义上无法表达该规则：
        // 这里按「本次映射未应用」返回，规则由共享宿主 FUSE 视图（虚拟呈现该路径）
        // 或进程内重定向 hook 承接，两条都不改动真实存储。
        //
        // 重定向后端目录仍在此补建：那是本模块的私有落点，与请求路径是否会出现在
        // 公共目录无关；不建它后续 bind 的源侧会缺失。
        if let Some(current_relative) = paths::relative_child_path(request_path, storage_path) {
            let current_fallback = self.normalize_path(&paths::join(
                &self.redirect_target,
                current_relative,
            ));
            if !self.ensure_directory_exists(&current_fallback, should_chown_current_dirs) {
                log::warn!(
                    "map redirect backend unavailable request={} backend={}",
                    request_path,
                    current_fallback
                );
            }
        }
        log::warn!(
            "map mount point missing, skip bind (request path must not be created): {}",
            request_path
        );
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

fn mapping_target_source_needs_app_writable_metadata(source: &str, user_id: i32) -> bool {
    paths::is_same_or_child(source, &paths::data_media_user_root_for_user(user_id))
}
