"""挂载元数据作用域、scoped FUSE 失败隔离与根上限分级的源码级不变量检查。

这些约束都缺少可执行的运行时测试：私有目录属主修复只在真机重定向运行时生效，
挂载子进程卡死依赖内核态不可中断等待才能复现，scoped FUSE 单根启动失败与根数
超限降级又依赖具体设备能力和内核版本。因此这里退化为源码级不变量，防止后续
改动重新扩大作用域或收回隔离：让仅映射模式的应用被改写私有目录属主、让单个
应用的挂载超时连坐整机、让单根 FUSE 失败丢失规则覆盖，或让多会话预算超限后退回
精度较弱的 namespace 通配展开。
"""

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]

def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def read_daemon_mount_module() -> str:
    """读取 `src/daemon_mount*.rs` 的合并内容（主文件在前，诊断与回收模块在后）。

    诊断取证与子进程回收已拆到独立模块，守卫需要看到合并后的内容，
    否则按函数名截取片段时会因文件边界而失败。
    """
    files = sorted((ROOT / "src").glob("daemon_mount*.rs"))
    ordered = [p for p in files if p.name == "daemon_mount.rs"] + [
        p for p in files if p.name != "daemon_mount.rs"
    ]
    return "".join(path.read_text(encoding="utf-8") for path in ordered)


def function_body(source: str, signature: str) -> str:
    """取出以 `signature` 开头的函数体，按花括号配对，跳过字符串字面量。"""
    start = source.index(signature)
    open_index = source.index("{", start)
    depth = 0
    index = open_index
    in_string = False
    while index < len(source):
        char = source[index]
        if in_string:
            if char == "\\":
                index += 2
                continue
            if char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[open_index : index + 1]
        index += 1
    raise AssertionError(f"未找到完整函数体: {signature}")


def const_value(source: str, name: str) -> int:
    match = re.search(rf"{name}: usize = (\d+);", source)
    if match is None:
        raise AssertionError(f"未找到常量定义: {name}")
    return int(match.group(1))


class MountMetadataScopeTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.monitor_roots = read("src/daemon_monitor/roots.rs")
        cls.daemon_mount = read_daemon_mount_module()
        cls.companion_mount = read("src/lifecycle/companion_mount.rs")
        cls.mount_core = read("src/mount/planner.rs")
        cls.fuse_mod = read("src/fuse_redirect/mod.rs")
        cls.fuse_config = read("src/fuse_redirect/config.rs")
        cls.runtime = read("src/hook/runtime.rs")
        cls.owner_migration = read("src/owner_migration.rs")

    def test_mapping_mode_only_apps_skip_private_owner_repair(self) -> None:
        # 私有外部存储由 MediaProvider 管理，监视侧不应再建立任何私有目录属主修复根。
        body = function_body(
            self.monitor_roots,
            "fn build_private_owner_repair_roots(spec: &MonitorAppSpec) -> Vec<WatchRoot>",
        )
        self.assertIn("MediaProvider", body)
        self.assertIn("Vec::new()", body)

    def test_mapping_target_created_when_missing(self) -> None:
        # 映射目标目录缺失时必须在挂载阶段补建：应用视图把它 bind 到请求路径，
        # 源不存在会让应用写入报 ENOENT，系统代写者（下载提供器）对请求路径的
        # mkdir 也会被 MediaProvider 判为拒绝，表现成 DownloadManager 的
        # "Failed to create parent"（run 40277331532 五版本一致实证）。
        body = function_body(
            self.mount_core,
            "pub(super) fn ensure_app_owned_mapping_target(&self, path: &str, owner_uid: i32) -> bool",
        )
        self.assertIn("fs::create_directory(&metadata_path, creation_uid)", body)
        self.assertIn("mapping target mkdir failed", body)
        self.assertNotIn("这里不做补建", body)

    def test_stuck_mount_circuit_is_scoped_to_one_package(self) -> None:
        # 熔断判据必须带上包名，否则单个应用的挂载超时会连坐整机其它应用。
        skip_body = function_body(
            self.daemon_mount,
            "fn should_skip_for_stuck_children(request: &MountRequest) -> bool",
        )
        self.assertIn("stuck_mount_child_counts(&request.package_name)", skip_body)

        counts_body = function_body(
            self.daemon_mount, "fn stuck_mount_child_counts(package_name: &str) -> (usize, usize)"
        )
        self.assertIn("child.package_name == package_name", counts_body)

    def test_stuck_child_records_expire_and_stay_bounded(self) -> None:
        # 卡死子进程必须记录登记时间（让熔断窗口失效）、记录包名（隔离作用域），
        # 并且回收列表需要有长度上限。
        self.assertIn("struct StuckMountChild", self.daemon_mount)
        self.assertIn("since_ms", self.daemon_mount)
        counts_body = function_body(
            self.daemon_mount, "fn stuck_mount_child_counts(package_name: &str) -> (usize, usize)"
        )
        self.assertIn("STUCK_MOUNT_CHILD_BLOCK_WINDOW_MS", counts_body)
        prune_body = function_body(self.daemon_mount, "fn prune_stuck_mount_children()")
        self.assertIn("MAX_TRACKED_STUCK_MOUNT_CHILDREN", prune_body)

    def test_existing_mapped_directory_keeps_its_metadata(self) -> None:
        # 已存在且属主就是目标 uid 的映射目录必须原样保留，一个元数据系统调用都不发：
        # 无条件 chmod 会把 MediaProvider 维护的既有模式覆盖成固定值，反而让原本能正常
        # 访问自有 Android/data|media|obb/<pkg> 的应用失去写权限。
        body = function_body(
            self.mount_core,
            "pub(crate) fn ensure_writable_mapped_directory(&self, path: &str, owner_uid: i32) "
            "-> bool",
        )
        self.assertIn("st.st_uid == uid", body)
        keep_index = body.index("st.st_uid == uid")
        chmod_index = body.index("chmod(c_path.as_ptr(), target_mode)")
        self.assertLess(keep_index, chmod_index)
        # 属主已正确时必须在 chown/chmod 之前直接返回，跳过整段元数据修正。
        self.assertIn("return true", body[keep_index:chmod_index])

        # 组与 mode 按路径形态二选一：私有树回到原生口径（组交给父目录 setgid 继承、
        # mode 02770），公共映射目标沿用 media_rw(1023) + 02771。写死 1023 会让
        # Android/data 一支长期偏离真机形态，也让 owner_migration 每次开机都要改回来。
        target_body = function_body(
            self.mount_core, "fn mapped_dir_target(&self, backend_path: &str)"
        )
        self.assertIn("is_android_private_backend_path(backend_path, self.user_id)", target_body)
        self.assertIn("(u32::MAX, 0o2770)", target_body)
        self.assertIn("(MEDIA_RW_GID, MAPPED_DIR_MODE)", target_body)
        # 两个写入点都必须用派生值，不能再各自写常量。
        self.assertIn("chown(c_path.as_ptr(), uid, target_gid)", body)
        self.assertNotIn("chown(c_path.as_ptr(), uid, MEDIA_RW_GID)", body)
        owned_body = function_body(
            self.mount_core,
            "pub(super) fn ensure_app_owned_mapping_target(&self, path: &str, owner_uid: i32)",
        )
        self.assertIn("chown(c_path.as_ptr(), owner_uid as u32, target_gid)", owned_body)
        self.assertIn("chmod(c_path.as_ptr(), target_mode)", owned_body)

    def test_scoped_fuse_sessions_are_retired(self) -> None:
        # per-app scoped FUSE 会话已退役：它是应用命名空间的自持引用，应用退出后仍在
        # 服务自己的挂载、永远等不到退出条件（issue #4：单机累积 1198 个常驻会话）。
        # 契约改为「只接共享宿主」：任一根本地 fork 出常驻会话都算回归。
        session = read("src/fuse_session.rs")
        self.assertNotIn("fn start_scoped_fuse_services(", session)
        self.assertNotIn("libc::fork()", session)
        # 不可用原因集中定义在共享模块，供两侧与日志共用同一套取值。
        for reason in (
            "NoSharedHost",
            "AttachGateClosed",
            "PolicyRegistrationFailed",
            "NoPolicyRules",
            "AttachFailed",
        ):
            self.assertIn(reason, session)
        for path in ("src/daemon_mount.rs", "src/lifecycle/companion_mount.rs"):
            source = read(path)
            # 只允许一个接入点：接入在挂载前完成，规划阶段一律 bind。事后再接会在已 bind
            # 的目标上叠一层 FUSE，清理时按路径只能摘栈顶，底层 bind 残留成永久叠挂。
            self.assertEqual(
                source.count("fn preattach_shared_host("),
                1,
                f"{path} 只能有一个宿主接入点",
            )
            self.assertNotIn("attach_shared_host_sessions", source, path)
            # 已由当前宿主层服务的视图根也要按宿主挂载记账，否则状态文件缺这条目标、
            # 下一轮清理会漏摘，挂载层失去归属判据。
            self.assertIn("crate::fuse_session::storage_view_top_dev(", source, path)
            self.assertIn("host_session: Some((view.child_pid, view.child_start_time_ticks))", source, path)
            # 两条挂载路径都不得再 fork per-app 会话。
            self.assertNotIn("srx_fuse\\0", source, path)

    def test_fuse_child_cleanup_rechecks_process_identity(self) -> None:
        # 状态文件携带启动时间后，SIGTERM/SIGKILL 两个阶段都必须重复校验 PID 身份，
        # 避免原服务退出后把信号发给复用该 PID 的其它进程。daemon 与 companion 的
        # 终止实现已统一到共享模块，两侧回收都必须走同一入口。
        body = function_body(read("src/fuse_terminate.rs"), "fn terminate_fuse_process(")
        self.assertIn("start_time_ticks", body)
        self.assertIn("process_identity_alive(pid, start_time_ticks)", body)
        self.assertIn("wait_for_exit(pid, start_time_ticks)", body)
        for path in ("src/daemon_mount.rs", "src/lifecycle/companion_mount.rs"):
            self.assertIn("crate::fuse_terminate::terminate_fuse_process(", read(path), path)

    def test_existing_android_private_metadata_is_preserved(self) -> None:
        # 完整隔离和 FUSE 初始化都不能对已有 Android/data、media、obb 后端目录强制
        # chown/chmod；这些目录由 MediaProvider 管理，改写会造成应用必须额外配置放行路径。
        mount_body = function_body(
            self.mount_core,
            "pub(crate) fn ensure_writable_mapped_directory(&self, path: &str, owner_uid: i32) "
            "-> bool",
        )
        self.assertIn("is_android_private_backend_path", mount_body)
        self.assertIn("keep existing Android private metadata", mount_body)
        self.assertIn("is_android_private_package_root", mount_body)
        policy = read("src/fuse_redirect/policy.rs")
        self.assertIn("is_android_private_backend_path", policy)
        self.assertIn("keep existing Android private metadata", policy)
        # 私有树事后属主修复链已于 2026-10-20 随属主翻转案定案整体拆除：守护侧不再
        # 对 Android/{data,media,obb} 做 chown/chmod 修复，元数据保持由 MediaProvider
        # 所有权管理与上方 mount/policy 的 keep-existing 守卫共同保证。断言修复链与
        # 类别级私有修复入口没有回潮。
        monitor = read("src/daemon_monitor/events.rs")
        self.assertNotIn("chown_android_private_path_if_needed", monitor)
        self.assertNotIn("repair_private_category_owner", monitor)

    def test_system_writer_owner_fix_preserves_native_gid(self) -> None:
        # 系统代写 hook 只负责把 owner 修回应用 UID；1078(ext_data_rw) 与
        # 1023(media_rw) 都是设备上可能出现的原生私有目录组，不能固定覆盖。
        body = function_body(self.runtime, "fn chown_private_path_if_needed(")
        self.assertIn("st.st_uid == owner_uid as u32", body)
        self.assertNotIn("st.st_uid == owner_uid as u32 && st.st_gid", body)
        self.assertIn("owner_uid as u32, u32::MAX", body)

    def test_owner_migration_restores_only_private_data_directory_gid(self) -> None:
        # 一次性迁移只把 Android/data 下、owner 正确且无 other 权限位的目录恢复到
        # AOSP 定义的 ext_data_rw；文件与共享媒体目录保留 1023。两种漂移都要收：
        # gid=media_rw(1023)（旧 hook 覆盖了继承来的组）与 gid=应用自身 uid
        # （目录创建入口 chown(uid, uid) 的产物，系统代写进程连穿越都做不到）。
        self.assertIn("owner-migration-1023-v5.done", self.owner_migration)
        self.assertIn("const EXT_DATA_RW_GID: u32 = 1078", self.owner_migration)
        body = function_body(
            self.owner_migration,
            "fn should_restore_ext_data_gid(",
        )
        self.assertIn('metadata.is_dir()', body)
        self.assertIn('metadata.uid() == target_uid || metadata.uid() == MEDIA_RW_UID', body)
        self.assertIn('metadata.gid() == MEDIA_RW_UID || metadata.gid() == target_uid', body)
        self.assertIn('metadata.mode() & 0o007 == 0', body)
        # 「02773 + 组漂移」双重污染必须能在同一次扫描里收敛，因此判据按收敛后的形态取。
        self.assertIn('0o2773', body)
        self.assertIn('path.rfind("/Android/data/")', body)
        self.assertNotIn('"/Android/media/"', body)
        self.assertNotIn('"/Android/obb/"', body)

    def test_storage_alias_binding_skips_source_self_bind(self) -> None:
        # 私有目录恢复会同时遍历 /storage、/mnt 和 /data/media 别名；源路径对应的
        # /data/media 别名必须跳过，避免 mount(source, source) 污染 FUSE/namespace 挂载栈。
        alias = read("src/mount/alias.rs")
        bind_body = function_body(alias, "pub(super) fn bind_mount_with_storage_aliases(")
        overlay_body = function_body(alias, "pub(super) fn bind_overlay_mount_with_storage_aliases(")
        self.assertIn("eq_ignore_case(source, &target)", bind_body)
        self.assertIn("eq_ignore_case(source, &target)", overlay_body)

    def test_own_private_restore_prefers_system_fuse_anchor(self) -> None:
        # 自有 Android 私有目录不能从 /data/media 直绑到应用视图；直绑会绕过
        # MediaProvider FUSE 并带入 media_rw_data_file 上下文，应用仍会被 SELinux 拒绝。
        # 2026-10-25 起（f84a29dd）源改为独立的系统视图锚点；2026-11-02 起锚点改为
        # **临时**（create_transient_system_view_anchor + Drop 摘除）：锚点常驻会在应用
        # 挂载表里留下 /data/adb/modules/.../tmp/system_view/<user> 这条模块私有路径。
        # 锚点缺失时仍以 "own private system view anchor unavailable" 留痕等待重试。
        apply = read("src/mount/apply.rs")
        body = function_body(apply, "fn restore_own_private_directories(")
        self.assertIn("create_transient_system_view_anchor", body)
        self.assertIn("backend_source", body)
        self.assertIn("own private system view anchor unavailable", body)
        self.assertIn('format!("Android/media/{}", self.package_name)', body)
        self.assertNotIn("unwrap_or_else(|| backend_source.clone())", body)
        self.assertNotIn('relative.starts_with("Android/data/")', body)

        # 锚点必须"建立→用完即摘"：守卫靠 Drop 保证任何提前返回也不会把它留在命名空间里。
        #
        # 两条分支都必须走 **克隆**（`open_tree(OPEN_TREE_CLONE)` + `move_mount`），
        # 不得退化成 `bind_mount`：本函数被 companion 与 daemon 各调用一次，克隆随
        # `move_mount` 一次性消费，第二次必然进回退分支；而 `mount(MS_BIND)` 的源必须
        # 属于调用方**当前命名空间**（内核 `do_loopback` 的 `check_mnt`），跨命名空间的
        # `/proc/1/root/...` magic link 作源恒失败——锚点拿不到，自有私有目录就整段
        # 没交回系统视图（真机 2026-11-13 AVD(A15)：`restored count=0`，场景 35 判
        # `file_missing`）。修复后同点位 `restored count=3`。
        anchor_body = function_body(apply, "fn create_transient_system_view_anchor(")
        self.assertIn("clone.attach_at(&anchor_root)", anchor_body)
        self.assertIn("SystemViewClone::capture(", anchor_body)
        self.assertNotIn("bind_mount(&source", anchor_body)
        self.assertIn("detach_mount_if_present(&anchor_root)", anchor_body)
        drop_body = function_body(apply, "impl Drop for SystemViewAnchor {")
        self.assertIn("detach_mount_if_present(&self.root)", drop_body)

    def test_auto_backend_prefers_fuse_full_view_root(self) -> None:
        # auto 语义为 FUSE 优先：能力可用时整棵视图根由本会话接管，规则仍在策略层
        # 逐请求判定；能力不可用时由能力门禁返回空根，挂载层回落 namespace。
        body = function_body(
            self.fuse_config,
            "pub fn scoped_fuse_mount_roots_for_request",
        )
        self.assertIn("storage_user_root_for_user(user_id)", body)
        self.assertIn("scoped_mount_allowed_for_scope", body)
        self.assertNotIn("scoped_mount_roots_for_hybrid_rules", body)

    def test_fuse_mode_prepares_mapping_target_dirs(self) -> None:
        # bind 布局在挂载时由规划器顺带建好映射目标目录；FUSE 布局没有对应挂载动作，
        # 必须在接入前由特权进程预建，否则下载提供器等系统代写链路会判定目标父目录缺失
        # 而放弃写入（CI 场景 39：Failed to create parent for …）。
        planner = read("src/mount/planner.rs")
        body = function_body(planner, "pub(crate) fn prepare_mapping_target_dirs(")
        self.assertIn("ensure_writable_mapped_directory", body)
        self.assertIn("final_path", body)
        # 两条挂载路径都必须预建：daemon 执行自己那份挂载，companion 负责应用侧接入。
        # 只补一边会让走另一边的平台继续失败（A14/A17 走 companion、A13/15/16 走 daemon）。
        daemon = read("src/daemon_mount.rs")
        self.assertIn("prepare_mapping_target_dirs", daemon)
        # daemon 侧必须在 fork 之前准备：子进程只允许 setns/mount/write，禁止堆分配。
        self.assertLess(
            daemon.index("prepare_mapping_target_dirs(request);"),
            daemon.index("run_mount_in_forked_child(request);"),
        )
        companion = read("src/lifecycle/companion_mount.rs")
        self.assertIn("mount_mgr.prepare_mapping_target_dirs(", companion)

    def test_path_mapping_resolution_chains_to_fixpoint(self) -> None:
        # 场景 37 的契约「父映射结果继续命中子映射」：bind 布局下两层挂载天然串联，FUSE 只在
        # 策略层解析，因此 resolve_mapping 必须迭代到不动点。退回单跳会让中间路径落在后端不存在
        # 的位置，应用侧表现为写入 ENOENT（CI run 42259826817 的 A17 现场即此）。
        policy = read("src/fuse_redirect/policy.rs")
        body = function_body(policy, "pub(super) fn resolve_mapping(&self, storage_path: &str)")
        self.assertIn("MAX_MAPPING_HOPS", body)
        # 双步振荡与恒等映射必须提前收敛，否则手写的 A→B / B→A 环会让解析永不停止。
        self.assertIn("previous.as_deref() == Some(next.as_str())", body)
        self.assertIn("next == path", body)
        # 自嵌套映射（目标落在自己请求路径的子树内）只能生效一次：bind 布局下它只挂一层，
        # 连续串联会把目录名逐跳叠加。判据必须按路径分段，不能按字符串前缀——`Download/X`
        # 与 `Download/XMapped` 只共享前缀，用 starts_with 会把正常映射判成自嵌套并整跳丢失。
        self.assertIn("paths::is_same_or_child", body)
        self.assertNotIn("starts_with(mapping.request_path", body)
        # 终止条件只决定「还有没有下一跳」：必须先落这一跳再判停，否则等同于映射未命中。
        self.assertLess(body.index("previous = current;"), body.index("if stop {"))
        # 单跳实现的标志：命中第一条映射就直接 return。出现即说明链式语义被改回去了。
        self.assertNotIn("return Some(mapping.final_path.clone());", body)

    def test_public_directory_mode_is_repaired_only_when_deficient(self) -> None:
        # 既有共享目录（DCIM/Pictures/Android…）的权限由 MediaProvider 决定：原型可能是
        # 02773 也可能是 02771。把这些目录归一成模块常量会改写用户磁盘上的既有权限，
        # 属于应用侧可观测的模块痕迹。因此只允许"缺能力位才补位"，且只追加不回收。
        planner_paths = read("src/mount/planner_paths.rs")
        helper = function_body(planner_paths, "pub(super) fn repair_missing_mode_bits(")
        self.assertIn("mode & required == required", helper)
        self.assertIn("mode | repair", helper)
        self.assertNotIn("chmod(c_path.as_ptr(), required)", helper)
        for signature in (
            "pub(super) fn fix_real_public_directory_metadata(path: &str)",
            "pub(super) fn fix_allowed_real_directory_metadata(path: &str)",
        ):
            body = function_body(planner_paths, signature)
            self.assertIn("repair_missing_mode_bits(", body)
            self.assertNotIn("chmod(c_path.as_ptr(), REAL_PUBLIC_DIR_MODE)", body)
            self.assertNotIn("chmod(c_path.as_ptr(), ALLOWED_REAL_DIR_MODE)", body)
        # 归一常量本身必须已经不在实现里，否则改回"等于常量才跳过"的旧口径不会被发现。
        self.assertNotIn("REAL_PUBLIC_DIR_MODE", read("src/mount/planner.rs"))
        self.assertNotIn("ALLOWED_REAL_DIR_MODE", read("src/mount/planner.rs"))
        # allowed-real 修复必须先 stat 再决定是否写，既有目录一个元数据系统调用都不发。
        allowed_body = function_body(
            planner_paths,
            "pub(super) fn fix_allowed_real_directory_metadata(path: &str)",
        )
        self.assertIn("allowed real stat", allowed_body)

    def test_lifted_alias_expansion_keeps_mnt_aliases(self) -> None:
        # 上提一层后主目标是用户根的父级，旧的"同层或更深"判据会退化成单条，三个 /mnt 别名
        # 会同时失效。展开必须先识别上提根并补齐别名集合，否则应用经别名路径访问存储直接断。
        alias = read("src/mount/alias.rs")
        body = function_body(alias, "pub(super) fn expand_storage_alias_paths(")
        lift_at = body.find("lifted_alias_roots(canonical_path)")
        root_at = body.find("storage_user_root_for_user(self.user_id)")
        self.assertGreater(lift_at, -1, "缺少上提根的别名展开分支")
        self.assertGreater(root_at, lift_at, "上提根判定必须早于用户根判据")
        helper = function_body(alias, "fn lifted_alias_roots(")
        self.assertIn('ends_with("/emulated")', helper)
        for root in ("/storage/emulated", "/mnt/user/", "/mnt/installer/", "/mnt/androidwritable/"):
            self.assertIn(root, helper)

    def test_lifted_view_root_entries_synthesized_before_backend(self) -> None:
        # 视图根上提后根 inode 的条目必须由我们自己合成（0/999/obb），且必须发生在候选缓存、
        # 虚拟目录与重试循环之前；否则空 rel 会去读后端根，把 Download/DCIM 直接列成根条目。
        callbacks = read("src/fuse_redirect/callbacks.rs")
        body = function_body(callbacks, "fn opendir(")
        synth_at = body.find("policy.view_lifted && rel.is_empty()")
        cache_at = body.find("dir_candidate_cache")
        loop_at = body.find("let (fh, entry_count) = loop {")
        self.assertGreater(synth_at, -1, "opendir 里缺少视图根合成分支")
        self.assertGreater(cache_at, synth_at, "合成必须早于候选缓存")
        self.assertGreater(loop_at, synth_at, "合成必须早于扫描/重试循环")
        self.assertIn("lifted_root_names(policy.user_id)", body)
        names = function_body(callbacks, "fn lifted_root_names(")
        self.assertIn('"obb"', names)
        self.assertIn("is_ascii_digit", names)

    def test_view_lift_switch_follows_mount_shape(self) -> None:
        # P5-c 的开关必须由挂载点形态唯一决定（`…/emulated` 为已上提），不能另开一份配置项，
        # 否则开关与真实挂载点会漂移：开关说已上提、挂载点还在用户目录级，解析就会少剥一层。
        policy = read("src/fuse_redirect/policy.rs")
        lift = function_body(policy, "pub(super) fn is_lifted_view_root(")
        self.assertIn('ends_with("/emulated")', lift)
        self.assertIn("Self::is_lifted_view_root(&mount_root)", policy)
        resolve = function_body(policy, "fn resolve_relative(")
        self.assertIn("if self.view_lifted", resolve)
        self.assertIn("self.lifted_view_rel(rel)", resolve)
        # 上提后根自身与"非本用户子树"必须分开处理：根为空 rel（不能 ENOENT，否则视图根
        # 自身 stat 失败），非本用户/obb 直通系统视图（基线里它们是普通目录）。
        self.assertIn("rel.trim_matches('/').is_empty()", resolve)
        self.assertIn("passthrough_foreign_view_rel", resolve)
        passthrough = function_body(policy, "fn passthrough_foreign_view_rel(")
        # 能力必须与基线一致（应用域自检实测）：`obb` 写入在基线失败、其它用户目录写入
        # 在基线被拒，因此两条直通都按只读处理——放开写才是"多给能力"。
        self.assertIn("is_read_only: true", passthrough)
        # `obb` 在系统视图里是独立挂载（OBB 的 FUSE），必须直通 `/storage/emulated/<rel>`。
        self.assertIn('paths::join("/storage/emulated", &rel)', passthrough)
        # 其它用户要走那个用户自己的视图根，否则落到本会话对系统视图的 uid 0 请求上
        # （应用域自检实测：目录可进但列表为空）。
        self.assertIn('"/mnt/user/{other_user}/emulated/{other_user}"', passthrough)
        self.assertIn('head == "obb"', passthrough)

    def test_lifted_view_probe_stays_side_effect_free(self) -> None:
        # P5-c 之前必须先证明"视图根上提一层"是等价变换：`<user>/…` 与今天的 `…` 必须
        # 解析到同一后端。探针只允许读，不允许影响真实解析，也不允许在非 debug 档位
        # 产生任何开销/日志。
        policy = read("src/fuse_redirect/policy.rs")
        wrapper = function_body(
            policy,
            "pub(super) fn backend_for_relative(",
        )
        self.assertIn("self.resolve_relative(rel, operation)", wrapper)
        self.assertIn("self.probe_lifted_view_resolution(", wrapper)
        probe = function_body(policy, "fn probe_lifted_view_resolution(")
        self.assertIn("is_debug_logging_enabled()", probe)
        self.assertIn("self.lifted_view_rel(", probe)
        lift = function_body(policy, "pub(super) fn lifted_view_rel<")
        self.assertIn("parse::<i32>()", lift)
        self.assertIn("self.user_id", lift)

    def test_dead_layer_cleanup_protects_system_view_roots(self) -> None:
        # 死宿主层清理按 dev ∈ known_module_fuse_devs() 在整个命名空间摘层。本模块对系统视图
        # 做过 bind，bind 继承系统 MediaProvider 的超级块，其 dev 会随账本混进 known 集合；
        # 若不做保护，系统自己的 `…/emulated` 根层会被当成本模块死层摘掉（真机表现：
        # 重定向应用别名根层消失、只剩 `…/emulated/<user>`）。因此清理前必须取
        # system_view_root_devs 并跳过这些 dev。
        companion = read("src/lifecycle/companion_mount.rs")
        self.assertIn("system_view_root_devs", companion)
        propagation = read("src/mount/propagation.rs")
        body = function_body(propagation, "pub fn system_view_root_devs(user_id: i32)")
        self.assertIn("/storage/emulated", body)
        self.assertIn("/mnt/user/", body)
        self.assertIn("/mnt/androidwritable/", body)
        self.assertIn("/mnt/pass_through/", body)

    def test_daemon_public_directory_fix_keeps_native_mode(self) -> None:
        # 守护侧同一口径：监控到公共目录事件时只补缺失能力位，不改写成固定模式。
        monitor = read("src/daemon_monitor/events.rs")
        self.assertIn("REQUIRED_PUBLIC_DIR_ACCESS_MODE", monitor)
        self.assertNotIn("current_mode != REAL_PUBLIC_DIR_MODE", monitor)
        self.assertNotIn("const REAL_PUBLIC_DIR_MODE", monitor)


if __name__ == "__main__":
    unittest.main()
