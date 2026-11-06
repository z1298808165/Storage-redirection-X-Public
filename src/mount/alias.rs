use super::{MountPlanner, PrimaryMountFailure};
use crate::platform::paths;
use libc::{lstat, stat as c_stat};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};

const ALIAS_SUCCESS_LOG_STEP: u64 = 128;
static ALIAS_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);

#[inline]
fn should_log_step(count: u64, step: u64) -> bool {
    count == 1 || count.is_multiple_of(step)
}

impl MountPlanner {
    /// P5-c 修订：内容 bind 的落点**不再上提**，保持用户根原样。
    ///
    /// 根因（真机 2026-11-04，CI Test-flow 场景 2 全矩阵失败后逐提交二分定位）：用户根
    /// `/storage/emulated/<user>` 是 OS 级寻址约定，应用永远按它读写。把内容 bind（沙盒/
    /// 私有/只读/恢复）从用户根抬到视图根后，应用视图里多出一层未剥离的 `<user>` 段——
    /// 应用写 `<user>/Download/x` 实际落进 `sdcard/<user>/Download/x`，harness 在
    /// `PRIVATE_ROOT/Download/SrtProbe` 处找不到文件，companion/bind 应用全矩阵失败。
    ///
    /// 上提的正确作用域是 **FUSE 视图自身的挂载点**：接入根由 `host_attach_root_for_user`
    /// 派生（fuse_host 侧），别名族的视图根展开由下方 `lifted_alias_roots` 承接（主目标
    /// 已是视图根时仍然生效）。内容 bind 回到用户根、叠在 FUSE 之上，两种接入形态
    /// （宿主 FUSE / companion bind）的视图都自洽：`<user>` 下是内容 bind，`<user>` 之外
    /// （999/obb）由 FUSE 或系统视图承接。
    fn lifted_target(&self, target: &str) -> String {
        target.to_string()
    }

    /// 主目标若已是"上提一层"的视图根（`…/emulated`），返回它的别名等价集合。
    ///
    /// 只认形态（以 `/emulated` 结尾），不认配置开关：与 `RedirectPolicy::view_lifted` 同一
    /// 套判据，避免"挂载点已上提、别名展开还按用户根"的半吊子状态。
    fn lifted_alias_roots(&self, canonical_path: &str) -> Option<Vec<String>> {
        let trimmed = canonical_path.trim_end_matches('/');
        if !trimmed.ends_with("/emulated") {
            return None;
        }
        let user_id = self.user_id;
        Some(vec![
            "/storage/emulated".to_string(),
            format!("/mnt/user/{user_id}/emulated"),
            format!("/mnt/installer/{user_id}/emulated"),
            format!("/mnt/androidwritable/{user_id}/emulated"),
        ])
    }

    pub(super) fn expand_storage_alias_paths(&self, canonical_path: &str) -> Vec<String> {
        if canonical_path.is_empty() {
            return Vec::new();
        }
        // P5-c：主目标若是用户根，先抬成视图根再展开；`lifted_alias_roots` 负责给出别名集合。
        let lifted = self.lifted_target(canonical_path);
        let canonical_path = lifted.as_str();

        // P5-c：视图根上提一层后，主目标变成 `…/emulated`（用户根的父级）。下面那条
        // "与用户根同层或更深"的判据对它为假，会退化成只返回单条——`/mnt/{user,installer,
        // androidwritable}` 三个别名会同时失效，应用经别名路径访问存储直接断。
        if let Some(expanded) = self.lifted_alias_roots(canonical_path) {
            return expanded;
        }

        let storage_root = paths::storage_user_root_for_user(self.user_id);
        if !paths::is_same_or_child(canonical_path, &storage_root) {
            return vec![canonical_path.to_string()];
        }

        let suffix = &canonical_path[storage_root.len()..];
        let alias_roots = paths::storage_alias_roots_for_user(self.user_id);

        let mut expanded = Vec::with_capacity(alias_roots.len());
        for root in alias_roots {
            append_unique(&mut expanded, format!("{}{}", root, suffix));
        }
        expanded
    }

    /// 自有私有目录恢复专用的 bind：只覆盖**应用可见**的 `/storage` 别名。
    ///
    /// 与 [`Self::bind_mount_with_storage_aliases`] 的差异在别名范围，原因有三（真机
    /// 2026-10-27 issue #2 日志实证）：
    ///
    /// 1. `/data/media/<user>` 与 `/mnt/pass_through/<user>/emulated/<user>` 是 f2fs
    ///    直连后端，正是 MediaProvider FuseDaemon 处理 FUSE 请求时做下层 I/O 的路径。
    ///    把 MP FUSE 视图（system_view 锚点）递归 bind 上去之后，FuseDaemon 的下层
    ///    I/O 会经内核 VFS 重新进入同一条 FUSE 连接，请求只能由 FuseDaemon 的其它
    ///    工作线程应答；并发稍高即线程耗尽互等，连接整体挂死——日志里的
    ///    `__fuse_simple_request` + `fuse_dentry_revalidate` 永久停等、全局存储停摆、
    ///    应用集体卡启动画面与关机卡黑屏就是这条链。
    /// 2. `/mnt/*` 技术别名只服务系统进程；运行中应用的命名空间已被模块改为私有
    ///    传播，恢复 bind 根本传不过去，逐个 bind 只产生无效 FUSE 请求。
    /// 3. 恢复bind 的消费方是应用视图（`/storage/emulated/<user>` 与其符号链接别名），
    ///    收窄后语义不变。
    // quality-allow(lint-suppression): 恢复收窄版与共用实现的失败处理契约对齐，
    // 参数列表沿用既有 alias bind 形态，避免调用方拼装底层细节。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_own_private_restore(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        let alias_targets: Vec<String> = self
            .expand_storage_alias_paths(primary_target)
            .into_iter()
            .filter(|target| target.starts_with("/storage/"))
            .collect();
        self.bind_mount_with_storage_alias_targets(
            source,
            primary_target,
            alias_targets,
            is_recursive,
            primary_failure_log,
            alias_failure_log,
            alias_success_log,
            is_any_mounted_out,
        )
    }

    /// 按**显式给定**的别名目标列表执行 bind（`bind_own_private_restore` 的执行核心）。
    ///
    /// 失败处理契约与 [`Self::bind_mount_with_storage_aliases`] 一致：主目标失败按
    /// `StopCurrentTarget` 处理（返回 true 但不置位 `is_any_mounted_out`），别名失败仅留痕。
    // quality-allow(lint-suppression): 显式别名列表使签名比共用实现多一参，保持与
    // 既有 alias bind 相同的失败处理契约，避免调用方拼装底层细节。
    #[allow(clippy::too_many_arguments)]
    fn bind_mount_with_storage_alias_targets(
        &self,
        source: &str,
        primary_target: &str,
        alias_targets: Vec<String>,
        is_recursive: bool,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        // P5-c：直接调用本核心的路径（沙箱/只读/恢复链）不经过外层包装，抬升必须在这里也做一次；
        // 否则同一份规则会同时产出"抬过的别名集合"和"没抬的 primary"，is_primary_target 判定失真。
        let primary_target = self.lifted_target(primary_target);
        let primary_target = primary_target.as_str();
        let mut is_any_mounted = false;
        let mut bound_keys: Vec<String> = Vec::new();

        for target in alias_targets {
            let is_primary_target = target == primary_target;
            if paths::eq_ignore_case(source, &target) {
                log::debug!("alias: skip self bind src={} dst={}", source, target);
                continue;
            }
            let bind_key = bind_target_key(&target);
            if bound_keys.contains(&bind_key) {
                log::debug!(
                    "alias: skip duplicate real path bind src={} dst={} real={}",
                    source,
                    target,
                    bind_key
                );
                continue;
            }
            if !is_primary_target && !path_exists(&target) {
                log::debug!(
                    "alias diag skip missing target={} source={}",
                    target,
                    source
                );
                continue;
            }
            if !is_primary_target && should_skip_self_shadowing_alias(source, &target) {
                log::debug!(
                    "alias diag skip self-shadowing target={} source={}",
                    target,
                    source
                );
                log::debug!(
                    "alias: skip self-shadowing bind src={} dst={}",
                    source,
                    target
                );
                continue;
            }

            let mounted = self.bind_mount(source, &target, is_recursive);
            log::debug!(
                "alias diag bind target={} primary={} mounted={} source={}",
                target,
                is_primary_target,
                mounted,
                source
            );
            if !mounted {
                if is_primary_target && let Some(log_text) = primary_failure_log {
                    log::warn!("alias: {} dst={}", log_text, target);
                }
                if let Some(log_text) = alias_failure_log {
                    log::warn!("alias: {} src={} dst={}", log_text, source, target);
                }
                if is_primary_target {
                    if let Some(out) = is_any_mounted_out {
                        *out = is_any_mounted;
                    }
                    return true;
                }
                continue;
            }

            is_any_mounted = true;
            bound_keys.push(bind_key);
            if !is_primary_target && let Some(log_text) = alias_success_log {
                let alias_ok_count = ALIAS_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(alias_ok_count, ALIAS_SUCCESS_LOG_STEP) {
                    log::debug!(
                        "alias: {} src={} dst={} n={}",
                        log_text,
                        source,
                        target,
                        alias_ok_count
                    );
                }
            }
        }

        if let Some(out) = is_any_mounted_out {
            *out = is_any_mounted;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_mount_with_storage_aliases(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_mode: PrimaryMountFailure,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        let primary_target = self.lifted_target(primary_target);
        let primary_target = primary_target.as_str();
        let mut is_any_mounted = false;
        let alias_targets = self.expand_storage_alias_paths(primary_target);
        let mut bound_keys: Vec<String> = Vec::new();

        for target in alias_targets {
            let is_primary_target = target == primary_target;
            if paths::eq_ignore_case(source, &target) {
                log::debug!("alias: skip self bind src={} dst={}", source, target);
                continue;
            }
            let bind_key = bind_target_key(&target);
            if bound_keys.contains(&bind_key) {
                log::debug!(
                    "alias: skip duplicate real path bind src={} dst={} real={}",
                    source,
                    target,
                    bind_key
                );
                continue;
            }
            if !is_primary_target && !path_exists(&target) {
                // 诊断：该出口此前不产生任何日志，导致「别名路径不存在」与「别名已挂载」
                // 在 CI 日志里无法区分。Android 13 场景 29 的映射只落地后端别名，需要确认
                // 其余别名的挂载点是否在此被跳过。
                log::debug!(
                    "alias diag skip missing target={} source={}",
                    target,
                    source
                );
                continue;
            }
            if !is_primary_target && should_skip_self_shadowing_alias(source, &target) {
                log::debug!(
                    "alias diag skip self-shadowing target={} source={}",
                    target,
                    source
                );
                log::debug!(
                    "alias: skip self-shadowing bind src={} dst={}",
                    source,
                    target
                );
                continue;
            }

            let mounted = self.bind_mount(source, &target, is_recursive);
            // 诊断：成功路径原先只有按 128 节流的 debug 日志，失败路径仅在有 log_text 时
            // 才记录，导致设备侧无法判断某个别名究竟挂上了还是被内核拒绝。Android 13
            // 场景 29 中应用视图别名未出现在 skip 名单却也没在 mountinfo 出现，需要用这条
            // 记录区分「bind 返回 false」与「bind 成功但未在当前命名空间生效」。
            log::debug!(
                "alias diag bind target={} primary={} mounted={} source={}",
                target,
                is_primary_target,
                mounted,
                source
            );
            if !mounted {
                if is_primary_target {
                    if let Some(log_text) = primary_failure_log {
                        log::warn!("alias: {} dst={}", log_text, target);
                    }

                    match primary_failure_mode {
                        PrimaryMountFailure::AbortAll => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return false;
                        }
                        PrimaryMountFailure::StopCurrentTarget => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return true;
                        }
                        PrimaryMountFailure::ContinueAliases => {}
                    }
                }

                if let Some(log_text) = alias_failure_log {
                    log::warn!("alias: {} src={} dst={}", log_text, source, target);
                }
                continue;
            }

            is_any_mounted = true;
            bound_keys.push(bind_key);
            if !is_primary_target && let Some(log_text) = alias_success_log {
                let alias_ok_count = ALIAS_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(alias_ok_count, ALIAS_SUCCESS_LOG_STEP) {
                    log::debug!(
                        "alias: {} src={} dst={} n={}",
                        log_text,
                        source,
                        target,
                        alias_ok_count
                    );
                }
            }
        }

        if let Some(out) = is_any_mounted_out {
            *out = is_any_mounted;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_overlay_mount_with_storage_aliases(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_mode: PrimaryMountFailure,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        let mut is_any_mounted = false;
        let alias_targets = self.expand_storage_alias_paths(primary_target);
        let mut bound_keys: Vec<String> = Vec::new();

        for target in alias_targets {
            let is_primary_target = target == primary_target;
            if paths::eq_ignore_case(source, &target) {
                log::debug!("alias: skip self overlay src={} dst={}", source, target);
                continue;
            }
            let bind_key = bind_target_key(&target);
            if bound_keys.contains(&bind_key) {
                log::debug!(
                    "alias: skip duplicate real path overlay src={} dst={} real={}",
                    source,
                    target,
                    bind_key
                );
                continue;
            }
            if !is_primary_target && !path_exists(&target) {
                // 诊断：此出口原先完全静默。路径映射走的是本 overlay 版本（`map.rs` 调
                // `bind_overlay_mount_with_storage_aliases`），而此前只在非 overlay 版本
                // 加了探针，导致 Android 13 场景 29 的映射别名既没有 skip 也没有 bind
                // 记录，无法区分「别名不存在」与「挂载未生效」。
                log::debug!(
                    "alias diag overlay skip missing target={} source={}",
                    target,
                    source
                );
                continue;
            }
            if !is_primary_target && should_skip_self_shadowing_alias(source, &target) {
                log::debug!(
                    "alias diag overlay skip self-shadowing target={} source={}",
                    target,
                    source
                );
                log::debug!(
                    "alias: skip self-shadowing overlay src={} dst={}",
                    source,
                    target
                );
                continue;
            }

            let overlay_bind_ok = self.bind_mount_overlay(source, &target, is_recursive);
            // 诊断：成功路径原先只有按步长节流的 debug 日志，设备侧不可见。
            log::debug!(
                "alias diag overlay bind target={} primary={} mounted={} source={}",
                target,
                is_primary_target,
                overlay_bind_ok,
                source
            );
            if !overlay_bind_ok {
                if is_primary_target {
                    if let Some(log_text) = primary_failure_log {
                        log::warn!("alias: {} dst={}", log_text, target);
                    }

                    match primary_failure_mode {
                        PrimaryMountFailure::AbortAll => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return false;
                        }
                        PrimaryMountFailure::StopCurrentTarget => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return true;
                        }
                        PrimaryMountFailure::ContinueAliases => {}
                    }
                }

                if let Some(log_text) = alias_failure_log {
                    log::warn!("alias: {} src={} dst={}", log_text, source, target);
                }
                continue;
            }

            is_any_mounted = true;
            bound_keys.push(bind_key);
            if !is_primary_target && let Some(log_text) = alias_success_log {
                let alias_ok_count = ALIAS_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(alias_ok_count, ALIAS_SUCCESS_LOG_STEP) {
                    log::debug!(
                        "alias: {} src={} dst={} n={}",
                        log_text,
                        source,
                        target,
                        alias_ok_count
                    );
                }
            }
        }

        if let Some(out) = is_any_mounted_out {
            *out = is_any_mounted;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_read_write_mount_with_storage_aliases(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_mode: PrimaryMountFailure,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        let mut is_any_mounted = false;
        let alias_targets = self.expand_storage_alias_paths(primary_target);
        let mut bound_keys: Vec<String> = Vec::new();

        for target in alias_targets {
            let is_primary_target = target == primary_target;
            let bind_key = bind_target_key(&target);
            if bound_keys.contains(&bind_key) {
                log::debug!(
                    "alias: skip duplicate real path readwrite src={} dst={} real={}",
                    source,
                    target,
                    bind_key
                );
                continue;
            }
            if !is_primary_target && !path_exists(&target) {
                continue;
            }
            if !is_primary_target && should_skip_self_shadowing_alias(source, &target) {
                log::debug!(
                    "alias: skip self-shadowing readwrite src={} dst={}",
                    source,
                    target
                );
                continue;
            }

            if !self.bind_mount_read_write_overlay(source, &target, is_recursive) {
                if is_primary_target {
                    if let Some(log_text) = primary_failure_log {
                        log::warn!("alias: {} dst={}", log_text, target);
                    }

                    match primary_failure_mode {
                        PrimaryMountFailure::AbortAll => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return false;
                        }
                        PrimaryMountFailure::StopCurrentTarget => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return true;
                        }
                        PrimaryMountFailure::ContinueAliases => {}
                    }
                }

                if let Some(log_text) = alias_failure_log {
                    log::warn!("alias: {} src={} dst={}", log_text, source, target);
                }
                continue;
            }

            is_any_mounted = true;
            bound_keys.push(bind_key);
            if !is_primary_target && let Some(log_text) = alias_success_log {
                let alias_ok_count = ALIAS_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(alias_ok_count, ALIAS_SUCCESS_LOG_STEP) {
                    log::debug!(
                        "alias: {} src={} dst={} n={}",
                        log_text,
                        source,
                        target,
                        alias_ok_count
                    );
                }
            }
        }

        if let Some(out) = is_any_mounted_out {
            *out = is_any_mounted;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_read_only_mount_with_storage_aliases(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_mode: PrimaryMountFailure,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        self.bind_read_only_mount_with_storage_aliases_inner(
            source,
            primary_target,
            is_recursive,
            primary_failure_mode,
            primary_failure_log,
            alias_failure_log,
            alias_success_log,
            is_any_mounted_out,
            false,
        )
    }

    // quality-allow(lint-suppression): 保持 storage alias 挂载失败处理契约一致，避免复制分支逻辑。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_read_only_mount_with_storage_aliases_preserving_backend(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_mode: PrimaryMountFailure,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
    ) -> bool {
        self.bind_read_only_mount_with_storage_aliases_inner(
            source,
            primary_target,
            is_recursive,
            primary_failure_mode,
            primary_failure_log,
            alias_failure_log,
            alias_success_log,
            is_any_mounted_out,
            true,
        )
    }

    // quality-allow(lint-suppression): 共享只读 alias 遍历需接收与两个公开 helper 相同的完整失败策略。
    #[allow(clippy::too_many_arguments)]
    fn bind_read_only_mount_with_storage_aliases_inner(
        &self,
        source: &str,
        primary_target: &str,
        is_recursive: bool,
        primary_failure_mode: PrimaryMountFailure,
        primary_failure_log: Option<&str>,
        alias_failure_log: Option<&str>,
        alias_success_log: Option<&str>,
        is_any_mounted_out: Option<&mut bool>,
        preserve_data_media_backend: bool,
    ) -> bool {
        let mut is_any_mounted = false;
        let alias_targets = self.expand_storage_alias_paths(primary_target);
        let mut bound_keys: Vec<String> = Vec::new();

        for target in alias_targets {
            let is_primary_target = target == primary_target;
            let bind_key = bind_target_key(&target);
            if bound_keys.contains(&bind_key) {
                log::debug!(
                    "alias: skip duplicate real path readonly src={} dst={} real={}",
                    source,
                    target,
                    bind_key
                );
                continue;
            }
            if !is_primary_target && !path_exists(&target) {
                continue;
            }
            // 别名展开会同时包含主目标本身和真实后端路径。只读来源取自真实后端时，
            // 别名里的同路径目标与来源是同一个目录对象：bind 到自身不会增加任何只读
            // 限制，却会在挂载表里把真实后端目录变成只读挂载点，遮蔽该目录可见别名
            // 视图的读取，使应用侧列举为空。主目标由 namespace 只读 bind 承担，因此
            // 这里只跳过非主目标的同名项（读写别名沿用相同判定）。
            if !is_primary_target && paths::eq_ignore_case(source, &target) {
                log::debug!(
                    "alias: skip self bind readonly src={} dst={}",
                    source,
                    target
                );
                continue;
            }
            if preserve_data_media_backend && is_data_media_backend_alias(&target, self.user_id) {
                log::debug!(
                    "alias: skip backend readonly alias src={} dst={}",
                    source,
                    target
                );
                continue;
            }
            if !is_primary_target && should_skip_self_shadowing_alias(source, &target) {
                log::debug!(
                    "alias: skip self-shadowing readonly src={} dst={}",
                    source,
                    target
                );
                continue;
            }

            if !self.bind_mount_read_only(source, &target, is_recursive) {
                if is_primary_target {
                    if let Some(log_text) = primary_failure_log {
                        log::warn!("alias: {} dst={}", log_text, target);
                    }

                    match primary_failure_mode {
                        PrimaryMountFailure::AbortAll => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return false;
                        }
                        PrimaryMountFailure::StopCurrentTarget => {
                            if let Some(out) = is_any_mounted_out {
                                *out = is_any_mounted;
                            }
                            return true;
                        }
                        PrimaryMountFailure::ContinueAliases => {}
                    }
                }

                if let Some(log_text) = alias_failure_log {
                    log::warn!("alias: {} src={} dst={}", log_text, source, target);
                }
                continue;
            }

            is_any_mounted = true;
            bound_keys.push(bind_key);
            if !is_primary_target && let Some(log_text) = alias_success_log {
                let alias_ok_count = ALIAS_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(alias_ok_count, ALIAS_SUCCESS_LOG_STEP) {
                    log::debug!(
                        "alias: {} src={} dst={} n={}",
                        log_text,
                        source,
                        target,
                        alias_ok_count
                    );
                }
            }
        }

        if let Some(out) = is_any_mounted_out {
            *out = is_any_mounted;
        }
        true
    }
}

fn is_data_media_backend_alias(path: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    paths::is_same_or_child(path, &root)
}

fn path_exists(path: &str) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
    let ret = unsafe { lstat(c_path.as_ptr(), st.as_mut_ptr()) };
    ret == 0
}

fn append_unique(list: &mut Vec<String>, value: String) {
    if value.is_empty() {
        return;
    }
    if !list.iter().any(|item| item == &value) {
        list.push(value);
    }
}

/// 绑定目标的去重键：解析符号链接后的真实路径。
///
/// 别名根里 `/storage/self/primary`（以及 user 0 的 `/storage/emulated/legacy`）是
/// 指向 `/storage/emulated/0` 的符号链接。`mount(2)` 按解析后的真实路径建立挂载，
/// 因此对这两个别名绑定会在同一挂载点再叠一层 `source` 与 `root` 都与 primary 完全
/// 相同的 bind（真机 mountinfo 实测：同一目标出现两条记录，后一条的父挂载就是前一条）。
/// 内容一致所以不会立刻出错，但白白多占一层，并让「同挂载点取栈顶」的判定出现并列项。
/// 这里用真实路径去重，保证同一真实目标在一轮别名绑定里只挂一次。
fn bind_target_key(target: &str) -> String {
    std::fs::canonicalize(target)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| target.to_string())
}

fn should_skip_self_shadowing_alias(source: &str, target: &str) -> bool {
    !paths::eq_ignore_case(source, target) && paths::is_child(source, target)
}
