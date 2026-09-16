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
        cls.daemon_mount = read("src/daemon_mount.rs")
        cls.companion_mount = read("src/lifecycle/companion_mount.rs")
        cls.mount_core = read("src/mount/core.rs")
        cls.fuse_mod = read("src/fuse_redirect/mod.rs")
        cls.fuse_config = read("src/fuse_redirect/config.rs")

    def test_mapping_mode_only_apps_skip_private_owner_repair(self) -> None:
        # 私有外部存储由 MediaProvider 管理，监视侧不应再建立任何私有目录属主修复根。
        body = function_body(
            self.monitor_roots,
            "fn build_private_owner_repair_roots(spec: &MonitorAppSpec) -> Vec<WatchRoot>",
        )
        self.assertIn("MediaProvider", body)
        self.assertIn("Vec::new()", body)

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
            "pub(super) fn ensure_writable_mapped_directory(&self, path: &str, owner_uid: i32) "
            "-> bool",
        )
        self.assertIn("st.st_uid == uid", body)
        keep_index = body.index("st.st_uid == uid")
        chmod_index = body.index("chmod(c_path.as_ptr(), MAPPED_DIR_MODE)")
        self.assertLess(keep_index, chmod_index)
        # 属主已正确时必须在 chown/chmod 之前直接返回，跳过整段元数据修正。
        self.assertIn("return true", body[keep_index:chmod_index])

    def test_scoped_fuse_start_isolates_single_root_failure(self) -> None:
        # 单根启动失败后以存储根会话恢复规则覆盖；daemon 与 companion 行为保持一致。
        # 根会话重试同样失败时才交给 namespace 回退与能力失败记账。
        for path in ("src/daemon_mount.rs", "src/lifecycle/companion_mount.rs"):
            body = function_body(read(path), "fn start_scoped_fuse_services(")
            self.assertIn("failed_roots", body, path)
            self.assertIn("storage_root", body, path)
            self.assertIn("collapsed to storage root", body, path)
            # 全部根都失败时仍要返回 None，保住"FUSE 整体不可用"的降级记账语义。
            self.assertIn("states.is_empty()", body, path)

    def test_scoped_fuse_budget_preserves_dynamic_rules(self) -> None:
        # 多会话预算只控制进程/内存开销，预算超限必须折叠为单个存储根 FUSE 会话，
        # 不能返回空列表让 namespace 按当前已存在目录展开而丢失动态通配语义。
        hard_limit = const_value(self.fuse_mod, "MAX_SCOPED_FUSE_ROOTS")
        self.assertGreater(hard_limit, 0)

        target_limit = const_value(self.fuse_mod, "TARGET_SCOPED_FUSE_ROOTS")
        self.assertLessEqual(target_limit, hard_limit)

        # 一级压缩用软目标、二级降级用硬上限，两级判据不能退回同一个常量。
        compact_body = function_body(self.fuse_config, "fn compact_scoped_mount_roots(")
        target_index = compact_body.index("super::TARGET_SCOPED_FUSE_ROOTS")
        hard_index = compact_body.index("super::MAX_SCOPED_FUSE_ROOTS")
        self.assertLess(target_index, hard_index)
        self.assertIn("vec![storage_root.to_string()]", compact_body)
        self.assertIn("collapse to single storage-root fuse session", compact_body)

    def test_fuse_child_cleanup_rechecks_process_identity(self) -> None:
        # 状态文件携带启动时间后，SIGTERM/SIGKILL 两个阶段都必须重复校验 PID 身份，
        # 避免原服务退出后把信号发给复用该 PID 的其它进程。
        body = function_body(self.daemon_mount, "fn terminate_fuse_child(")
        self.assertIn("start_time_ticks", body)
        self.assertIn("process_identity_alive", body)
        companion_body = function_body(self.companion_mount, "fn terminate_fuse_service(")
        self.assertIn("start_time_ticks", companion_body)
        self.assertIn("process_identity_alive", companion_body)

    def test_existing_android_private_metadata_is_preserved(self) -> None:
        # 完整隔离和 FUSE 初始化都不能对已有 Android/data、media、obb 后端目录强制
        # chown/chmod；这些目录由 MediaProvider 管理，改写会造成应用必须额外配置放行路径。
        mount_body = function_body(
            self.mount_core,
            "pub(super) fn ensure_writable_mapped_directory(&self, path: &str, owner_uid: i32) "
            "-> bool",
        )
        self.assertIn("is_android_private_backend_path", mount_body)
        self.assertIn("keep existing Android private metadata", mount_body)
        self.assertIn("is_android_private_package_root", mount_body)
        policy = read("src/fuse_redirect/policy.rs")
        self.assertIn("is_android_private_backend_path", policy)
        self.assertIn("keep existing Android private metadata", policy)
        monitor = read("src/daemon_monitor/events.rs")
        repair_body = function_body(monitor, "fn chown_android_private_path_if_needed(")
        self.assertIn("private_root", repair_body)
        self.assertIn("eq_ignore_case(path, private_root)", repair_body)

    def test_storage_alias_binding_skips_source_self_bind(self) -> None:
        # 私有目录恢复会同时遍历 /storage、/mnt 和 /data/media 别名；源路径对应的
        # /data/media 别名必须跳过，避免 mount(source, source) 污染 FUSE/namespace 挂载栈。
        alias = read("src/mount/alias.rs")
        bind_body = function_body(alias, "pub(super) fn bind_mount_with_storage_aliases(")
        overlay_body = function_body(alias, "pub(super) fn bind_overlay_mount_with_storage_aliases(")
        self.assertIn("eq_ignore_case(source, &target)", bind_body)
        self.assertIn("eq_ignore_case(source, &target)", overlay_body)

    def test_own_private_restore_prefers_system_fuse_anchor(self) -> None:
        # 自有 Android/data|media|obb 不能从 /data/media 直绑到应用视图；直绑会绕过
        # MediaProvider FUSE 并带入 media_rw_data_file 上下文，应用仍会被 SELinux 拒绝。
        apply = read("src/mount/apply.rs")
        body = function_body(apply, "fn restore_own_private_directories(")
        self.assertIn("real_storage_anchor", body)
        self.assertIn("backend_source", body)
        self.assertIn("FUSE anchor unavailable", body)
        self.assertNotIn("unwrap_or_else(|| backend_source.clone())", body)
        self.assertNotIn('relative.starts_with("Android/data/")', body)

    def test_wildcard_mapping_from_android_private_subtree_uses_scoped_fuse(self) -> None:
        # Android/data/<pkg>/file/* -> Download/AAA 这类规则必须纳入 scoped FUSE 根索引，
        # 否则 namespace 只能展开当前已存在目录，后续新建匹配目录无法动态映射。
        body = function_body(
            self.fuse_config,
            "pub fn scoped_mount_roots_for_hybrid_rules(",
        )
        self.assertIn("mapping_wildcard_rules", body)
        self.assertIn(".chain(mapping_wildcard_rules)", body)
        self.assertIn("resolve_scoped_path_mappings(path_mappings, user_id, &storage_root)", body)


if __name__ == "__main__":
    unittest.main()
