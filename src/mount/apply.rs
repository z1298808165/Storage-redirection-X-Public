use super::apply_targets::{
    build_allowed_real_source_candidates, build_mapping_source_roots, build_read_only_source_roots,
    collect_restored_read_only_excluded_children, detach_mount_if_present,
    is_covered_by_scoped_fuse_mount, mountinfo_device_for_target,
    mountinfo_has_own_private_restore_layer, mountinfo_has_target, mountinfo_root_is_app_sandbox,
    namespace_mappings_outside_scoped_fuse, path_overlaps_mapping_request,
    path_shadows_mapping_request, read_mountinfo,
};
use super::map::PathMappingApplyOptions;
use super::{MountPlanner, concrete_mount_fallback_parent};
use crate::domain::PathMapping;
use crate::platform::{fs, module_paths, paths};

/// Android 应用 uid 起点。低于该值的是系统 uid，不具备 app-specific 外部存储语义。
const SYSTEM_UID_LIMIT: i32 = 10000;

impl MountPlanner {
    fn prepare_real_storage_anchor(&mut self, _storage_path: &str) -> Option<String> {
        self.real_storage_anchor = None;
        let anchor_mount = super::apply_targets::real_storage_anchor_mount_path(self.user_id);
        let usable = super::apply_targets::real_storage_anchor_usable_path(self.user_id);

        if self.real_storage_anchor_is_usable(&anchor_mount, &usable) {
            self.real_storage_anchor = Some(usable.clone());
            return Some(usable);
        }

        // 重建锚点。源按优先级取 **emulated 根**：
        //   1) `/mnt/user/<user>/emulated`：MediaProvider FUSE 的真实视图；
        //   2) `/data/media`：f2fs 后端（MP 不可用时的回退）。
        //
        // 两者结构相同（第一段 `<user>` 才是用户存储根），因此可用根口径一致。
        //
        // 为什么源必须是 emulated 根、且必须**非递归**：本模块会把
        // `<emulated>/<user>`（主视图根与三个别名）换成自己的 FUSE。直接 bind
        // `/storage/emulated/<user>`、或递归 bind emulated 根本身，拿到的都是
        // **被本模块重定向过的**树——锚点于是指向沙箱，映射目标会落进沙箱而不是真实
        // 公共路径（历史故障：场景 29 `Download/Test` 落点错、热重载后写入 Permission
        // denied）。emulated 根本身我们从不覆盖，非递归 bind 只复制那一层，`<user>`
        // 回落到 MediaProvider 的普通目录节点，正是需要的干净真实视图。
        //
        // 由此还消掉了一条更危险的旧路径：过去热重载时锚点可能被污染，修复方式是在
        // 「存储根已重定向」分支里 **摘掉视图根 → 用真实视图重建锚点 → 立刻绑回**，
        // 中间有三次系统调用的 fail-open 窗口（窗口内应用写入会落到公共路径）。新源
        // 不可能被自己的重定向污染，这条分支整条去掉。
        detach_mount_if_present(&anchor_mount);
        if !self.ensure_directory_exists(&anchor_mount, false) {
            log::warn!(
                "real storage anchor dir unavailable pkg={} anchor={}",
                self.package_name,
                anchor_mount
            );
            return None;
        }
        let sources = [
            format!("/mnt/user/{}/emulated", self.user_id),
            "/data/media".to_string(),
        ];
        for source in sources {
            if !fs::is_directory(&source) {
                log::debug!(
                    "real storage anchor source absent pkg={} source={}",
                    self.package_name,
                    source
                );
                continue;
            }
            detach_mount_if_present(&anchor_mount);
            if !self.bind_mount(&source, &anchor_mount, false) {
                continue;
            }
            if !fs::is_directory(&usable) {
                log::warn!(
                    "real storage anchor has no user root pkg={} source={} usable={}",
                    self.package_name,
                    source,
                    usable
                );
                detach_mount_if_present(&anchor_mount);
                continue;
            }
            // 兜底复核：锚点的 root 不得指向本应用沙箱（正常不会发生，但一旦发生，
            // 用它解析映射源会把映射目标落进沙箱，属静默数据错位，必须有明确判据）。
            let fresh_mountinfo = read_mountinfo().unwrap_or_default();
            if mountinfo_root_is_app_sandbox(&fresh_mountinfo, &anchor_mount, &self.package_name) {
                log::warn!(
                    "real storage anchor points at app sandbox, rejected pkg={} source={} anchor={}",
                    self.package_name,
                    source,
                    anchor_mount
                );
                detach_mount_if_present(&anchor_mount);
                continue;
            }
            log::info!(
                "real storage anchored {} -> {} (usable {}) pkg={}",
                source,
                anchor_mount,
                usable,
                self.package_name
            );
            self.real_storage_anchor = Some(usable.clone());
            return Some(usable);
        }
        log::warn!(
            "real storage anchor unavailable pkg={} anchor={}",
            self.package_name,
            anchor_mount
        );
        None
    }

    /// 锚点是否可直接复用：挂载仍在、可用根有内容、且没有指向本应用沙箱。
    fn real_storage_anchor_is_usable(&self, anchor_mount: &str, usable: &str) -> bool {
        if !fs::is_directory(usable) {
            return false;
        }
        let mountinfo = read_mountinfo().unwrap_or_default();
        if !mountinfo_has_target(&mountinfo, anchor_mount) {
            return false;
        }
        if mountinfo_root_is_app_sandbox(&mountinfo, anchor_mount, &self.package_name) {
            log::warn!(
                "real storage anchor polluted by sandbox, rebuilding pkg={} anchor={}",
                self.package_name,
                anchor_mount
            );
            return false;
        }
        ["Android", "Download", "DCIM"]
            .iter()
            .any(|child| fs::is_directory(&paths::join(usable, child)))
    }

    /// 摘除真实存储锚点（用完即摘）。
    ///
    /// 锚点只是映射源与元数据换算用的临时真实视图，不承载任何应用可见路径；留在应用
    /// 挂载表里就是一条模块私有路径（`tmp/real_storage/<user>`），也就是 namespace 后端
    /// 唯一剩下的字符串痕迹。摘除只改挂载表：已建立的映射层各自持有超级块与 dentry
    /// 引用，内容与访问都不受影响。
    ///
    /// 幂等：没有锚点（或已被外部摘除）时只清空字段。
    pub fn detach_real_storage_anchor(&mut self) {
        if self.real_storage_anchor.take().is_none() {
            return;
        }
        let anchor_mount = super::apply_targets::real_storage_anchor_mount_path(self.user_id);
        detach_mount_if_present(&anchor_mount);
        log::info!(
            "real storage anchor detached anchor={} pkg={}",
            anchor_mount,
            self.package_name
        );
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

        let redirect_root_preexisting = fs::is_directory(&resolved_target);
        if !self.ensure_directory_exists(&resolved_target, false) {
            log::error!("mkdir redirect failed: {}", resolved_target);
            return false;
        }
        // 本次新建的重定向根必须当场归位成原生形态（应用属主 + 继承组 + 02770）：
        // 随后两处「已存在的私有目录不得改写」守卫会把新建目录一并跳过，属主就固定成
        // 了运行本流程的 root，系统代写进程只剩组 r-x、进不了沙盒。
        if !redirect_root_preexisting {
            self.ensure_native_private_dir_owner(&resolved_target, self.app_uid);
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
        // P5-c：主挂载目标与会话挂载点必须同源。整体重定向时上提一层（`…/emulated`），
        // scoped 子路径保持原位；两处不一致会让策略的 `view_lifted`（由挂载点形态派生）
        // 与实际落点错位，症状是根目录列错或整棵视图 ENOENT。
        // P5-c 修订：内容 bind（沙盒根）的落点不上提，保持用户根原样——用户根
        // `/storage/emulated/<user>` 是 OS 级寻址约定，应用永远按它读写；把沙盒 bind 抬到
        // 视图根会让应用视图多出一层未剥离的 `<user>` 段，写入落点全部错位（CI Test-flow
        // 场景 2 全矩阵失败的根因，真机二分 + 应用视图对照实证）。上提的作用域是 FUSE
        // 视图自身的挂载点（接入根由 host_attach_root_for_user 派生），与内容 bind 无关。
        let view_root = storage_path.clone();
        if !self.bind_mount_with_storage_aliases(
            &resolved_target,
            &view_root,
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
        let Some(anchor) = self.create_transient_system_view_anchor() else {
            log::warn!(
                "own private system view anchor unavailable pkg={}",
                self.package_name
            );
            return 0;
        };
        let system_view = anchor.user_view.clone();
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
            // 只恢复**已存在**的自有目录：原生行为下应用不写就不会有
            // `Android/{data,media,obb}/<pkg>`，这里无条件 mkdir 会让每个被管理的
            // 应用在真实后端留一个空目录（2026-10-27 issue #2 用户复测实证：
            // `Android/media` 下出现全量空目录）。应用真正创建该目录后（经宿主
            // 数据面的 Real 直写，属原生允许路径），下一次挂载事件/reconcile 会在
            // 此恢复系统视图绑定，内容落点与跨进程共享语义不受影响。
            if !fs::is_directory(&backend_source) {
                log::debug!(
                    "own private backend absent, skip restore: {}",
                    backend_source
                );
                continue;
            }
            // 元数据修正只针对已存在的目录，不再承担创建职责。
            if !self.ensure_writable_mapped_directory(&backend_source, self.app_uid) {
                log::debug!(
                    "own private backend metadata fix skipped: {}",
                    backend_source
                );
            }
            // 自有 media 子树随后会被 bind 回系统 MediaProvider 视图，而 MP 的
            // FuseDaemon 只能用自身身份打开下层文件（附加组 media_rw/ext_data_rw，
            // CapEff=0）：owner-only 入口在系统视图里连属主自己都打不开（原生同样
            // 打不开，这是 MP 的既有语义）。这里**不改任何权限位**，只把这种
            // 「系统视图无法服务」的入口记进诊断，便于现场按日志定位。
            if is_media {
                self.report_unserviceable_private_entries(&backend_source);
            }
            // 三类自有私有目录统一保留系统 FUSE 的访问控制与上下文：源取系统视图锚点，
            // 缺失时不把 /data/media 直接暴露给应用，保留诊断等待后续恢复。
            let source = paths::join(&system_view, &relative);
            if !fs::is_directory(&source) {
                log::warn!("own private system view missing: {}", source);
                continue;
            }
            // 幂等：热重载与 reconcile 会重复进入这里，已经指向系统视图（与源同一
            // dev+ino）时不再叠一层 bind，否则同一 target 上的层数会随重挂次数增长。
            //
            // dev+ino 之外再比 mountinfo root 形态（真机 2026-10-27 issue #2 实证）：
            // 应用视图的 `/storage/emulated/<user>` 上叠着 MediaProvider 的二级用户
            // FUSE 视图（fs 0:340），锚点 system_view 是 0:294 的 bind，stat 的 dev
            // 恒不相等，仅凭 dev+ino 会把「已恢复」判成「未恢复」，每轮 reconcile
            // 全量重 bind（单包媒体目录一小时内 18 次、含同秒并发 3 次），层数叠加
            // 且 MP FUSE 请求风暴最终把 FuseDaemon 拖挂（`__fuse_simple_request`
            // 永久停等 → 全局存储停摆）。root 精确等于 `/<user>/<relative>` 的记录
            // 存在即视为已恢复；MP 若在其建用户视图，内容仍经其视图取自同一棵
            // 底层树，重复 bind 只会触发下一轮重建循环。
            //
            // **幂等判定必须在建挂载点之前**：挂载子进程是 root，而应用视图
            // （`/storage/emulated/<user>`）由模块自己的数据面提供，数据面对未登记
            // uid 一律 fail-closed（`deny_all` → ENOENT）。root 的 stat 只有在 dentry
            // 已被内核缓存的极少数情况下才成功，拿它做判据必然假阴性，于是每轮都
            // 误判成「未恢复」。mountinfo 判据不依赖任何路径解析，放在最前。
            let expected_restore_root = format!("/{}/{}", self.user_id, relative);
            if mountinfo_has_own_private_restore_layer(
                &read_mountinfo().unwrap_or_default(),
                &storage_target,
                &expected_restore_root,
            ) || same_mount_identity(&storage_target, &source)
            {
                log::debug!("own private already on system view {}", storage_target);
                if is_media {
                    crate::mount::own_private_pending::note_success(&self.package_name);
                }
                restored_count += 1;
                continue;
            }
            // metadata_operations_path 将 storage alias 转到 /data/media，因此这一步
            // 只保证**后端**目录存在（真实磁身上得先有 dentry，系统视图才有可绑的
            // 内容），不代表应用视图里的挂载点已就绪——后者见下面的分支。
            if !self.ensure_directory_exists(&storage_target, true) {
                log::warn!(
                    "own private target mkdir failed: {} errno={}",
                    storage_target,
                    fs::last_create_directory_errno()
                );
                if is_media {
                    // 视图未就绪（root 连存储根都解析不到）是 MP 窗口内的必然失败，
                    // 不消耗重试预算；就绪后仍失败才是要按预算放弃的真问题。
                    let view_ready = fs::is_directory(storage_path);
                    crate::mount::own_private_pending::note_failure(&self.package_name, view_ready);
                }
                continue;
            }

            let mut is_restored = false;
            // 恢复只用 root 身份：目标挂载点在应用视图里，但它的可解析性由
            // `/storage/emulated` 层（MediaProvider 的 FUSE）决定。MP 对**注册过的
            // uid/pid** 才放行，对模块 fork 出的子进程（无论 root 还是降权到应用
            // uid）在就绪窗口内一律拒绝——真机 2026-11-04 duchamp 实测：同一时刻
            // root 逐级可达而应用 uid 整片 EACCES，且降权子进程拿到的是更严的
            // EACCES 而非 ENOENT。因此「降权取句柄」救不了这个场景，恢复的成败
            // 只取决于 MP 视图是否已就绪（root 侧 `root_view` 链），见重试闭环。
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
                if is_media {
                    crate::mount::own_private_pending::note_success(&self.package_name);
                }
                // **不做属主归一**：原生 MediaProvider FUSE 下文件的属主是创建者
                // 应用 uid（duchamp A16 原生对照实证：9 月20 日微信经原生 FUSE 创建
                // 的 `Android/media/com.tencent.mm` 属主为微信 uid，组 media_rw），
                // 访问放行由 MP 权限层（media 子树共享，见 MP hook 放行）与宿主
                // 数据面的 root 身份保证，不依赖 DAC 属主。此前的整树 chown 到
                // 宿主包 uid 会与 companion fchown 的创建者 uid 语义来回翻转
                // （2026-10-27 issue #2 用户实测：media 文件属主随写入/恢复交替
                // 变化），并破坏跨应用共享场景（插件写的文件被改成宿主包 uid）。
                //
                // **只归一化 media 子树**的历史约束随之失效一并移除；`data`/`obb`
                // 的属组漂移仍由一次性迁移（owner-migration）与场景 6 的
                // 「漂移不被运行期修复」契约管辖，本就未做运行期修复。
                restored_count += 1;
            } else {
                // 失败必须留痕：这条恢复是跨进程 SQLite 共享缓存层的唯一保证，
                // 静默失败会直接回到两层页缓存分裂（真机表现为每隔若干次热重载
                // 掉一次登录）。
                if is_media {
                    let view_ready = fs::is_directory(storage_path);
                    crate::mount::own_private_pending::note_failure(&self.package_name, view_ready);
                }
                log::warn!(
                    "own private restore failed target={} source={} bind_errno={} create_errno={}",
                    storage_target,
                    source,
                    crate::mount::own_private_pending::last_bind_errno(),
                    fs::last_create_directory_errno()
                );
                log::warn!(
                    "own private view snapshot pkg={} {}",
                    self.package_name,
                    self.describe_storage_view(storage_path)
                );
            }
        }
        restored_count
    }

    /// 只读诊断：统计自有 media 子树里「系统视图打不开」的 sqlite 入口。
    ///
    /// MediaProvider 的 FuseDaemon 用自身身份打开下层文件（AOSP `pf_open` 的
    /// `open(io_path, flags)`，直通只是把这个自开的 fd 交给内核），而它的附加组只有
    /// media_rw(1023)/ext_data_rw(1078)、`CapEff: 0`。owner-only 入口在系统视图里
    /// **连属主自己都打不开**——真机 2026-11-04 duchamp 实证：root 经 MP 视图 open
    /// 一个 0600 文件同样 EACCES，说明拒绝来自 MP 而非内核 DAC。这是 MP 的既有语义，
    /// 模块**不去改权限位绕开它**（改了等于替用户决定别人的文件该有什么权限，其它
    /// 场景的后果不可预知），只把现场记进 verbose 日志，便于按 `mode=` 定位到底是
    /// 哪个入口、由哪条绕过系统视图的写入路径产生的。
    ///
    /// 判据只看「属主有、组没有」这一条：组位覆盖了属主位时 MP 就能服务，无需上报。
    /// 深度与条目数都有上界，避免在大媒体目录上做全量遍历。
    /// 判据只看「属主有、组没有」这一条：组位覆盖了属主位时 MP 就能服务，无需上报。
    /// 深度与条目数都有上界，避免在大媒体目录上做全量遍历。
    /// 失败现场的存储视图快照：本进程命名空间里 `/storage/emulated*` 的挂载清单 +
    /// root 身份逐级可达性。
    ///
    /// 恢复失败有两种完全不同的成因，日志必须能直接分开：
    /// - **视图还没就位**（`root_dir=false` 且清单里没有对应挂载）→ 那是挂载时序问题，
    ///   重试/延后才有可能成功；
    /// - **视图在、但该名字被拒绝**（`root_dir=true`，只有目标级 errno≠0）→ 那是数据面
    ///   或 DAC 语义问题，重试永远不会成功，必须换路径。
    fn describe_storage_view(&self, storage_path: &str) -> String {
        let mut mounts = String::new();
        for line in read_mountinfo().unwrap_or_default().lines() {
            // 字段：id parent major:minor root mountpoint options ... - fstype source super
            let fields: Vec<&str> = line.split(' ').collect();
            if fields.len() < 5 {
                continue;
            }
            let mount_point = fields[4];
            if !mount_point.starts_with("/storage/emulated") {
                continue;
            }
            let sep = fields
                .iter()
                .position(|field| *field == "-")
                .and_then(|index| {
                    fields
                        .get(index + 1)
                        .zip(fields.get(index + 2))
                        .map(|(fs_type, source)| (*fs_type, *source))
                });
            if !mounts.is_empty() {
                mounts.push(',');
            }
            match sep {
                Some((fs_type, source)) => {
                    // source 直接区分服务方：`srx_fuse_host[pid]` 是本模块宿主会话，
                    // `/dev/fuse` 是 MediaProvider 视图——失败到底卡在谁那里一目了然。
                    mounts.push_str(&format!(
                        "{}<-{}({},{})",
                        mount_point, fields[3], fs_type, source
                    ));
                }
                None => mounts.push_str(&format!("{}<-{}(?)", mount_point, fields[3])),
            }
        }
        let root_chain = [
            "/storage",
            "/storage/emulated",
            storage_path,
            &format!("{}/Android", storage_path),
            &format!("{}/Android/media", storage_path),
        ]
        .iter()
        .map(|path| format!("{}={}", path, Self::stat_chain_entry(path)))
        .collect::<Vec<_>>()
        .join(" ");
        // 宿主会话的策略登记快照：uid-0 请求走 namespace 回退，若应用 uid 尚未出现在
        // fingerprints 里，说明本轮挂载的策略登记还没落到宿主会话——恢复的 ENOENT
        // 即 fail-closed 所致，而不是 MP 拒绝。
        let fingerprints = crate::fuse_host::read_host_session_view()
            .map(|view| {
                let registered: Vec<String> = view
                    .policy_fingerprints
                    .keys()
                    .map(|uid| uid.to_string())
                    .collect();
                format!(
                    "uids=[{}] app_uid_registered={}",
                    registered.join(","),
                    view.policy_fingerprints
                        .contains_key(&(self.app_uid as u32))
                )
            })
            .unwrap_or_else(|| "host_session=none".to_string());
        format!(
            "mounts=[{}] root_view={} host_policy={}",
            mounts, root_chain, fingerprints
        )
    }

    /// 快照用的单级可达性：`dir` / `no(errno)`，errno 直接区分 ENOENT（fail-closed
    /// 未登记）与 EACCES（MP 注册制拒绝）等成因。
    fn stat_chain_entry(path: &str) -> String {
        let Ok(c_path) = std::ffi::CString::new(path) else {
            return "bad".to_string();
        };
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: c_path 是 NUL 结尾路径，st 指向栈上未初始化空间，调用期间均有效。
        if unsafe { libc::stat(c_path.as_ptr(), st.as_mut_ptr()) } == 0 {
            return "dir".to_string();
        }
        format!("no({})", crate::platform::errno::last())
    }

    fn report_unserviceable_private_entries(&self, media_root: &str) {
        use std::os::unix::fs::MetadataExt as _;
        use std::os::unix::fs::PermissionsExt as _;
        const MAX_DEPTH: usize = 4;
        const MAX_ENTRIES: usize = 512;

        let mut queue = vec![(std::path::PathBuf::from(media_root), 0usize)];
        let mut visited = 0usize;
        let mut unserviceable = 0usize;
        let mut sample = String::new();
        while let Some((dir, depth)) = queue.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                visited += 1;
                if visited > MAX_ENTRIES {
                    log::debug!(
                        "own private serviceability scan truncated root={} visited={}",
                        media_root,
                        visited
                    );
                    break;
                }
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if file_type.is_dir() {
                    if depth < MAX_DEPTH {
                        queue.push((path, depth + 1));
                    }
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if !is_sqlite_entry_name(&name) {
                    continue;
                }
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if metadata.uid() != self.app_uid as u32 {
                    continue;
                }
                let mode = metadata.permissions().mode() & 0o7777;
                let owner_bits_for_group = (mode & 0o700) >> 3;
                if owner_bits_for_group == 0 || mode & owner_bits_for_group == owner_bits_for_group
                {
                    continue;
                }
                unserviceable += 1;
                if sample.is_empty() {
                    sample = format!("{} mode={:o}", path.display(), mode);
                }
            }
        }
        if unserviceable > 0 {
            log::debug!(
                "own private sqlite entries unserviceable by system view count={} root={} sample={}",
                unserviceable,
                media_root,
                sample
            );
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

    /// 建立一个**临时**的系统存储视图锚点，返回持有它的守卫。
    ///
    /// 源取 `/mnt/user/<user>/emulated`：它在应用命名空间里恒为一层未被模块覆盖的
    /// MediaProvider FUSE 视图（模块只覆盖 `<user>` 子树与存储根，不覆盖 emulated
    /// 本身）。整根 bind 之后，锚点下多一层 `<user>` 才是存储根结构，因此可用视图是
    /// `<anchor>/<user>`；该层次不经过被覆盖的路径解析，是干净的系统视图。
    ///
    /// 锚点本身不承载任何应用可见路径，**用完必须摘除**：留着它就是应用 mountinfo 里
    /// 一条模块私有路径（`/data/adb/modules/<id>/tmp/system_view/<user>`），真机
    /// 2026-11-02 实测这是整份挂载表里最后一条 `leak_module_path`。摘除只改挂载表，
    /// 已完成的 bind 各自持有超级块与 dentry 引用，内容不受影响。摘除动作放在
    /// [`SystemViewAnchor`] 的 `Drop` 里，任何提前返回（包括后续迭代里的 `return`）
    /// 都不会把锚点留在命名空间里。
    fn create_transient_system_view_anchor(&self) -> Option<SystemViewAnchor> {
        let anchor_root = paths::join(module_paths::SYSTEM_VIEW_TMP_DIR, &self.user_id.to_string());
        let user_view = paths::join(&anchor_root, &self.user_id.to_string());
        // 首选：附着"本模块挂载落地之前"捕获的系统视图克隆。它给出的是纯系统视图，
        // 不受别名视图影响；附着点仍是临时锚点，用完即摘，不留私有路径痕迹。
        if let Some(clone) = self.system_view_clone.borrow_mut().take() {
            detach_mount_if_present(&anchor_root);
            let _ = fs::create_directory(&anchor_root, -1);
            if fs::is_directory(&anchor_root)
                && clone.attach_at(&anchor_root)
                && fs::is_directory(&user_view)
            {
                log::debug!("system view anchor from clone root={}", anchor_root);
                return Some(SystemViewAnchor {
                    root: anchor_root,
                    user_view,
                });
            }
            log::warn!("system view clone attach unusable root={}", anchor_root);
            detach_mount_if_present(&anchor_root);
        }
        // 回退：没有克隆（例如 daemon 路径）时按原样从系统视图 bind 一个锚点。
        // 这条回退在坍缩形态下可能失败——那时锚点里的 `<user>` 已被本模块别名视图接管
        // （root 请求无会话策略 → `fallback_deny` → ENOENT）；有克隆时不会走到这里。
        // 与 SystemViewClone::capture 同源：`/mnt/user/<u>/emulated` 已被本模块别名视图覆盖。
        let source = format!("/mnt/pass_through/{}/emulated", self.user_id);
        if !fs::is_directory(&source) {
            log::warn!("system view source unavailable: {}", source);
            return None;
        }
        if same_mount_identity(&anchor_root, &source)
            && anchor_generation_is_current(&anchor_root, &source)
            && fs::is_directory(&paths::join(&user_view, "Android"))
        {
            // 既有锚点仍指向当前系统视图：直接复用，避免每轮摘挂抖动。返回值是
            // RAII 守卫，`Drop` 仍会按"临时锚点"语义在离开作用域时摘除。
            return Some(SystemViewAnchor {
                root: anchor_root,
                user_view,
            });
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
        if !fs::is_directory(&paths::join(&user_view, "Android")) {
            log::warn!("system view anchor incomplete anchor={}", anchor_root);
            detach_mount_if_present(&anchor_root);
            return None;
        }
        Some(SystemViewAnchor {
            root: anchor_root,
            user_view,
        })
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

/// 文件名是否属于 sqlite 主库或边车（小写化后的文件名）。
///
/// 与 `hook::media_fuse` 的 sqlite 判定同源，但这里只需要布尔结论且本模块不依赖
/// hook 模块，因此本地实现。
fn is_sqlite_entry_name(name_lower: &str) -> bool {
    const SUFFIXES: &[&str] = &[
        ".db",
        ".db-wal",
        ".db-shm",
        ".db-journal",
        ".sqlite",
        ".sqlite-wal",
        ".sqlite-shm",
        ".sqlite-journal",
        ".sqlite3",
        ".sqlite3-wal",
        ".sqlite3-shm",
        ".sqlite3-journal",
    ];
    name_lower.ends_with("-wal")
        || name_lower.ends_with("-shm")
        || SUFFIXES.iter().any(|suffix| name_lower.ends_with(suffix))
}

/// 锚点是否仍属于**当前这一代**系统视图 FUSE 连接。
///
/// `same_mount_identity` 只比 dev+ino：锚点是 `source` 的 bind，两者本来就是同一个
/// superblock 的根 dentry，所以 MediaProvider 重启、`vold` 重新拉起用户视图（minor
/// 换代）之后这个判据依然成立——但锚点实际绑在上一代连接上，访问会 ENOTCONN 或
/// 直接 ENOENT（真机 2026-11-03 duchamp：`bind mount failed src=.../tmp/system_view/0/0/
/// Android/media/<pkg> errno=2`，紧接着 `own private restore failed`，自有私有目录
/// 退回直连层，随后主库被 WAL 回滚成 4096B）。
///
/// 这里用 mountinfo 里两个挂载点的 `major:minor` 判定代际：任一侧取不到记录就按
/// 「不是当前代」处理，宁可重建锚点也不复用可能已失效的连接。
fn anchor_generation_is_current(anchor_root: &str, source: &str) -> bool {
    let mountinfo = read_mountinfo().unwrap_or_default();
    let anchor_dev = mountinfo_device_for_target(&mountinfo, anchor_root);
    let source_dev = mountinfo_device_for_target(&mountinfo, source);
    match (anchor_dev, source_dev) {
        (Some(anchor_dev), Some(source_dev)) => anchor_dev == source_dev,
        _ => false,
    }
}

/// 两个路径是否指向同一个挂载对象。///
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

/// 临时系统视图锚点：析构即摘除，命名空间里不留持久挂载。
///
/// 见 [`MountPlanner`] 上 `create_transient_system_view_anchor` 的说明。
struct SystemViewAnchor {
    root: String,
    user_view: String,
}

impl Drop for SystemViewAnchor {
    fn drop(&mut self) {
        // 锚点（无论来自系统视图克隆还是回退的 bind）用完即摘：留着它就是应用
        // mountinfo 里一条模块私有路径。已建立的恢复 bind 各自持有超级块与 dentry
        // 引用，摘除不影响内容。
        detach_mount_if_present(&self.root);
        log::debug!("system view anchor detached root={}", self.root);
    }
}
