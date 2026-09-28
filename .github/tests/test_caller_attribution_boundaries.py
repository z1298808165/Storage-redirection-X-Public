import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HARNESS = ROOT / ".github" / "tests" / "harness" / "should_passthrough_provider_allowed_parent_mkdir.rs"
OWN_PRIVATE_HARNESS = ROOT / ".github" / "tests" / "harness" / "own_android_private_path_access.rs"


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def read_paths_module() -> str:
    """读取 `src/platform/paths*.rs` 的全部内容。

    paths 已按职责拆出 paths_alias / paths_roots / paths_rules / paths_safety 同级
    模块，守卫测试需要看到合并后的内容，否则抽取函数时会因文件边界而失败。
    """
    platform_dir = ROOT / "src" / "platform"
    files = sorted(platform_dir.glob("paths*.rs"))
    return "\n".join(path.read_text(encoding="utf-8") for path in files)


def extract_fn(source: str, fn_name: str) -> str:
    """用大括号计数从源码中抽出整个 fn（含签名与函数体），不依赖固定结尾标记。

    这样即使 dir.rs 在目标函数之后增删函数，也能稳定取到完整的 should_passthrough_*
    实现，避免按函数名下标截断导致抄错。
    """
    marker = f"fn {fn_name}("
    start = source.index(marker)
    brace_start = source.index("{", start)
    depth = 0
    idx = brace_start
    while idx < len(source):
        ch = source[idx]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                idx += 1
                break
        idx += 1
    return source[start:idx]


def build_harness(template: str, fn_src: str) -> str:
    """把抽出的真实 fn 注入 harness 模板的占位符处。"""
    placeholder = "// __INJECT_SHOULD_PASSTHROUGH_FN__"
    if placeholder not in template:
        raise AssertionError("harness 模板缺少注入占位符")
    return template.replace(placeholder, fn_src)


def remove_virtual_branch(fn_src: str) -> str:
    """反向验证用：去掉 scope 条件里的 virtual 分支，使其只接受 passthrough。

    若原函数真的包含 virtual 作用域放行（修复点），去掉后 virtual-only 用例会从 TRUE 变 FALSE，
    从而让 harness 运行非 0，证明 harness 在验证真实函数而非空壳。
    """
    marker = "|| crate::hook::is_provider_virtual_scope_active()"
    if marker not in fn_src:
        raise AssertionError("抽取的函数不含 virtual 作用域分支，无法做反向验证")
    return fn_src.replace(marker, "")


class CallerAttributionBoundariesTest(unittest.TestCase):
    def test_access_policy_rules_do_not_infer_anonymous_callers(self) -> None:
        attribution_sources = "\n".join(
            read(path)
            for path in (
                "src/config/merge.rs",
                "src/redirect/engine.rs",
                "src/redirect/engine/caller.rs",
                "src/hook/jni_query/rewrite.rs",
            )
        )
        for forbidden in (
            "resolve_read_only_package_by_path_for_user",
            "PackagePathMatchMode::ReadOnly",
            "has_system_writer_read_only_owner_hint",
            "resolve_read_only_owner_package_by_path",
            "resolve_read_only_path_owner_context",
            "read-only path infer",
        ):
            self.assertNotIn(forbidden, attribution_sources)

    def test_ownership_bearing_caller_hints_remain_available(self) -> None:
        merge = read("src/config/merge.rs")
        caller = read("src/redirect/engine/caller.rs")
        engine = read("src/redirect/engine.rs")
        rewrite = read("src/hook/jni_query/rewrite.rs")

        self.assertIn("resolve_mapping_request_package_by_path_for_user", merge)
        self.assertIn("resolve_mapping_request_caller_context", rewrite)
        self.assertIn("infer_recent_path_caller_identity", caller)
        self.assertIn("has_system_writer_recent_public_caller_hint", engine)
        self.assertIn("resolve_android_private_path_owner", caller)

    def test_public_media_collection_children_are_not_ownership_hints(self) -> None:
        """公共媒体子目录不得被当作应用所有权提示。

        回归背景：判定式原先只要路径存在第二级就返回真，`DCIM/Camera` 因此被认领
        为「携带所有权」。任何应用往该目录的保存都由 MediaProvider 代写，配置了
        以它为源映射的应用会把所有应用的保存一起收走，图片被写进自己的映射目标，
        发起应用回读失败。这里锁定「公共集合根 + 公共媒体子目录」的组合不携带所有权。
        """
        merge = read("src/config/merge.rs")
        hint_fn = extract_fn(merge, "is_specific_storage_owner_hint")

        self.assertIn("is_public_media_collection_child", hint_fn)
        self.assertNotIn("segments.next().is_some()", hint_fn)

        child_fn = extract_fn(merge, "is_public_media_collection_child")
        for segment in ("camera", "screenshot", "screenshots"):
            self.assertIn(segment, child_fn)

    def test_proxied_write_prefers_recent_caller_hint_before_mapping(self) -> None:
        """代写归属必须先查真实调用方提示，再回退到配置映射反推。

        回归背景：MediaProvider 提交 pending 文件时以自身身份发起，没有 binder
        调用方；若直接按配置反推归属，公共目录上的并发保存会互相串味。因此归属
        解析的第一顺位必须是按路径登记的真实调用方提示，配置映射只作兜底。
        """
        rewrite = read("src/hook/jni_query/rewrite.rs")
        resolver = extract_fn(rewrite, "resolve_proxied_write_caller_context")

        self.assertIn("resolve_path_hint_caller_context", resolver)
        self.assertIn("resolve_mapping_request_caller_context", resolver)
        self.assertLess(
            resolver.index("resolve_path_hint_caller_context"),
            resolver.index("resolve_mapping_request_caller_context"),
        )

        hint_ctx = extract_fn(rewrite, "resolve_path_hint_caller_context")
        self.assertIn("infer_recent_path_caller_identity", hint_ctx)
        self.assertIn("is_system_writer_package", hint_ctx)
        self.assertIn("ANDROID_APP_UID_START", hint_ctx)

        context = rewrite[
            rewrite.index("fn resolve_storage_caller_context") : rewrite.index(
                "fn resolve_mapping_request_caller_context"
            )
        ]
        system_writer_branch = context[
            context.index("let system_writer = is_system_writer_uid(caller_uid);") : context.index(
                "let caller_package = resolve_caller_package(caller_uid, path_text);"
            )
        ]
        self.assertIn("resolve_proxied_write_caller_context", system_writer_branch)
        self.assertNotIn("resolve_mapping_request_caller_context", system_writer_branch)

    def test_mapping_target_owner_is_used_before_system_writer_fallback(self) -> None:
        caller = read("src/redirect/engine/caller.rs")
        resolver = caller[
            caller.index("fn resolve_mapping_request_owner_package_by_path") : caller.index(
                "fn should_query_download_owner_for_writer"
            )
        ]

        self.assertIn("resolve_mapping_request_package_by_path_for_user", resolver)
        self.assertIn("resolve_mapping_target_package_by_path_for_user", resolver)
        self.assertIn("if !request_owner.is_empty()", resolver)

    def test_private_mapping_share_keeps_owner_context_for_external_openers(self) -> None:
        rewrite = read("src/hook/jni_query/rewrite.rs")
        context = rewrite[
            rewrite.index("fn resolve_storage_caller_context") : rewrite.index(
                "fn resolve_mapping_request_caller_context"
            )
        ]

        self.assertIn("extract_android_private_path_owner", context)
        self.assertIn("resolve_mapping_request_caller_context(user_id, caller_uid, path_text, true)", context)
        self.assertIn("private_owner", context)

    def test_private_mapping_alias_repairs_public_target_file_access(self) -> None:
        runtime = read("src/hook/runtime.rs")
        open_hook = read("src/hook/ops/open.rs")
        self.assertIn("fix_mapped_private_alias_access", runtime)
        self.assertIn("mode | 0o006", runtime)
        self.assertGreaterEqual(open_hook.count("fix_mapped_private_alias_access"), 2)

    def test_media_file_columns_keep_mapping_during_pending_publish(self) -> None:
        hooker = read("java_src/org/srx/hook/Hooker.java")
        callback = hooker[
            hooker.index("public Object providerMediaFileColumnCallback") : hooker.index(
                "private static void restoreContentValue"
            )
        ]
        self.assertIn('!"update".equals(mutationMethod)', callback)

    def test_known_callers_still_apply_their_read_only_policy(self) -> None:
        policy = read("src/redirect/engine/policy.rs")
        writer = read("src/redirect/writer.rs")

        self.assertIn("read_only_check_path_by_caller_paths", policy)
        self.assertIn("CallerRealPathKind::ReadOnly", writer)
        self.assertIn("inferred_uid != *effective_caller_uid", writer)
        self.assertIn("policy::is_system_writer_package(effective_caller_package)", writer)

    def test_private_hint_inference_reuses_one_lazy_package_snapshot(self) -> None:
        source = read("src/monitor/source_hint.rs")
        infer = source[source.index("fn infer_from_hints") : source.index("fn infer_from_path_hints")]
        matcher = source[
            source.index("fn resolve_matching_hint(") : source.index("fn private_hint_window_ms(")
        ]
        resolver = source[
            source.index("fn infer_package_by_private_path_tokens(") : source.index(
                "fn read_running_packages()"
            )
        ]

        self.assertIn("PackageInferenceSnapshot::default()", infer)
        self.assertIn("resolve_matching_hint", infer)
        self.assertNotIn("infer_package_by_private_path_tokens", infer)
        self.assertEqual(1, matcher.count("infer_package_by_private_path_tokens"))
        self.assertIn("snapshot.shared_uid_cache_refreshed", resolver)
        self.assertIn("snapshot.running_packages.is_none()", resolver)
        self.assertNotIn('read_dir("/proc")', resolver)

    def test_provider_directory_reports_existing_target_and_cleans_empty_source(self) -> None:
        java = read("java_src/org/srx/hook/Hooker.java")
        jni = read("src/java_hook/hooker_class.rs")
        directory = read("src/hook/ops/mutation/dir.rs")

        callback = java[
            java.index("public Object providerDirectoryCallback") : java.index(
                "public Object providerFileParentCallback"
            )
        ]
        mutation = java[
            java.index("public Object providerMutationCallback") : java.index(
                "public Object providerFuseCallback"
            )
        ]
        self.assertIn("created || directDirectory.isDirectory()", callback)
        self.assertIn("rememberProviderRedirectSourceDirectory(sourcePath, directPath)", callback)
        self.assertIn('b"rememberProviderRedirectSourceDirectory\\0"', jni)
        self.assertIn(
            "redirectEnabled ? callBackup(args) : callBackupWithProviderPassthrough(args)",
            mutation,
        )
        self.assertIn("enterProviderVirtualScope()", mutation)
        self.assertIn("exitProviderVirtualScope()", mutation)
        self.assertIn("for (source, target) in crate::hook::exit_provider_passthrough()", jni)
        self.assertIn("for (source, target) in crate::hook::exit_provider_virtual_scope()", jni)
        self.assertIn("remember_provider_redirect_source_directory", jni)
        self.assertIn("cleanup_provider_redirect_source_directory", jni)
        self.assertIn("is_public_default_sandbox_redirect(source_path, target_path)", directory)
        self.assertIn("libc::rmdir(c_path.as_ptr())", directory)

    def test_disabled_provider_open_does_not_enter_mapped_file_branch(self) -> None:
        java = read("java_src/org/srx/hook/Hooker.java")
        callback = java[
            java.index("public Object providerOpenCallback") : java.index(
                "public Object providerMutationCallback"
            )
        ]
        disabled_branch = callback[
            callback.index("if (!redirectEnabled)") : callback.index(
                "captureMediaSourceFileDescriptor", callback.index("if (!redirectEnabled)")
            )
        ]
        self.assertNotIn("tryOpenMappedMediaFile", disabled_branch)
        self.assertIn("callBackupPassthrough(args)", disabled_branch)

    def test_public_media_store_values_fall_back_to_physical_storage(self) -> None:
        java = read("java_src/org/srx/hook/Hooker.java")
        resolver = java[
            java.index("private static String resolveMediaStoreDirectPathForValues") : java.index(
                "private static String mediaStorePhysicalRoot"
            )
        ]

        self.assertIn("isRedirectEnabledForCallerUid(callerUid)", resolver)
        self.assertIn("isSafePublicMediaValuePath(path)", resolver)
        self.assertIn("directPath = mediaStorePublicPhysicalFallback(path, callerUid)", resolver)
        self.assertIn("return physicalPath", resolver)
        self.assertNotIn("normalizeRelativeDataPath(path, callerUid)", resolver)

    def test_public_media_store_fallback_follows_parent_redirect_target(self) -> None:
        java = read("java_src/org/srx/hook/Hooker.java")
        fallback = java[
            java.index("private static String mediaStorePublicPhysicalFallback") : java.index(
                "private static String mediaStorePhysicalRoot"
            )
        ]

        # MediaProvider 在 insert 期间用 .pending-<随机>-<文件名> 构造结果文件，native 重写未必
        # 为这个临时名返回目标。回退若直接落到公共物理目录，mkdir 仍会被 native 改写进沙箱，
        # 公共目录实际不存在，pending 文件创建失败会让 insert 返回 null。
        self.assertIn("resolveMediaStoreDirectPath(parent, callerUid)", fallback)
        self.assertIn("isSrxSandboxFallbackPath(parentTarget, callerUid)", fallback)
        self.assertIn('candidate = parentTarget + "/" + value.substring(end + 1)', fallback)

    def test_enotconn_cleanup_detaches_owned_dead_fuse_mount(self) -> None:
        rust = read("src/fuse_redirect/config.rs")
        cleanup = rust[
            rust.index("fn finish_failed_session") : rust.index(
                "/// scoped 挂载使用的挂载源前缀"
            )
        ]

        self.assertIn("if app_exited || is_already_unmounted_errno(error_no)", cleanup)
        self.assertIn("if detach_mount_point(mount_point, identity)", cleanup)
        self.assertIn("matches!(error_no, libc::EINVAL | libc::ENOENT)", cleanup)


    def test_should_passthrough_provider_allowed_parent_mkdir_boundary_guard(self) -> None:
        # 静态边界守卫：直接读取 src/hook/ops/mutation/dir.rs 中真实函数的条件，逐维度确认
        # should_passthrough_provider_allowed_parent_mkdir 的边界没有被改坏。即便 rustc harness
        # 没有执行，这一层也能在条件结构被回退（例如 virtual 作用域被移除）时报警。
        directory = read("src/hook/ops/mutation/dir.rs")
        fn = extract_fn(directory, "should_passthrough_provider_allowed_parent_mkdir")

        # redirect 是前提：非 redirect 直接放行 false。
        self.assertIn("!redirect_result.is_redirect()", fn)
        # mapping 重定向不走该分支。
        self.assertIn("redirect_result.is_mapping", fn)
        # 系统代写预装模式同样执行 mkdir 策略，不能独立禁用祖先放行。
        self.assertNotIn("hub.is_monitor_only()", fn)
        # 修复点：作用域条件必须同时接受 passthrough 与 virtual，且以 `!(A || B)` 形式表达。
        # 只要 is_provider_virtual_scope_active 出现在函数里，就说明 virtual-only 仍被放行，
        # 回退成「仅 passthrough」会让该标识符消失。
        self.assertIn("is_provider_passthrough_active()", fn)
        self.assertIn("is_provider_virtual_scope_active()", fn)
        normalized_scope = " ".join(
            "!(crate::hook::is_provider_passthrough_active() || crate::hook::is_provider_virtual_scope_active())".split()
        )
        self.assertIn(normalized_scope, " ".join(fn.split()))
        # 仅 system-writer 包才走该分支。
        self.assertIn("policy::is_system_writer_package", fn)
        # caller 必须非空且路径是其放行真实路径的父级。
        self.assertIn("hub.get_current_caller_package()", fn)
        self.assertIn("!caller_package.is_empty()", fn)
        self.assertIn("is_path_parent_of_caller_allowed_real_path", fn)

    def test_should_passthrough_provider_allowed_parent_mkdir_executed(self) -> None:
        # 从真实源码抽取整个 fn，注入 harness 模板后由 rustc 独立编译运行边界真值表。
        # 项目禁止在 src/ 内新增内联 Rust 测试，因此 harness 放在 .github/tests/harness/ 由
        # rustc 独立编译运行。模板只含真实模块桩与类型，绝不抄写实现——真正执行的是抽取到的源码。
        if not HARNESS.exists():
            self.skipTest(f"harness 模板缺失: {HARNESS}")
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest(
                "rustc 不可用：未编译执行 stub harness，仅静态边界守卫生效 "
                "(test_should_passthrough_provider_allowed_parent_mkdir_boundary_guard)"
            )

        directory = read("src/hook/ops/mutation/dir.rs")
        fn_src = extract_fn(directory, "should_passthrough_provider_allowed_parent_mkdir")
        template = HARNESS.read_text(encoding="utf-8")
        harness_src = build_harness(template, fn_src)

        with tempfile.TemporaryDirectory() as tmp:
            harness_path = os.path.join(tmp, "harness.rs")
            Path(harness_path).write_text(harness_src, encoding="utf-8")
            bin_path = os.path.join(tmp, "passthrough_harness")
            compile_proc = subprocess.run(
                [rustc, harness_path, "-O", "-o", bin_path],
                capture_output=True,
                text=True,
            )
            if compile_proc.returncode != 0:
                self.fail(
                    "harness 编译失败:\n" + compile_proc.stdout + compile_proc.stderr
                )
            run_proc = subprocess.run([bin_path], capture_output=True, text=True)
            print(run_proc.stdout)
            self.assertEqual(
                run_proc.returncode,
                0,
                "harness 边界用例未全部通过:\n" + run_proc.stdout + run_proc.stderr,
            )
            self.assertIn("ALL", run_proc.stdout)

            # 恢复旧的模式拒绝条件，必须在实际运行时击中两个系统代写预装用例。
            monitor_mutated = fn_src.replace(
                "|| redirect_result.is_mapping",
                "|| redirect_result.is_mapping || hub.is_monitor_only()",
                1,
            )
            self.assertNotEqual(monitor_mutated, fn_src)
            monitor_path = os.path.join(tmp, "harness_monitor_mutated.rs")
            Path(monitor_path).write_text(build_harness(template, monitor_mutated), encoding="utf-8")
            monitor_bin = os.path.join(tmp, "monitor_harness_mutated")
            monitor_compile = subprocess.run(
                [rustc, monitor_path, "-O", "-o", monitor_bin],
                capture_output=True, text=True,
            )
            self.assertEqual(monitor_compile.returncode, 0, monitor_compile.stderr)
            monitor_run = subprocess.run([monitor_bin], capture_output=True, text=True)
            self.assertNotEqual(monitor_run.returncode, 0)
            self.assertIn("FAIL monitor_writer_passthrough_true", monitor_run.stdout)
            self.assertIn("FAIL monitor_writer_virtual_true", monitor_run.stdout)

            # 反向验证：在内存里去掉 virtual 分支再编译运行，必须非 0。
            # 若 harness 只是空壳，去掉 virtual 分支后仍会通过；只有真正执行原函数时，
            # virtual-only 用例才会从 TRUE 变 FALSE，使 harness 退出码非 0。
            mutated = remove_virtual_branch(fn_src)
            mutated_src = build_harness(template, mutated)
            mutated_path = os.path.join(tmp, "harness_mutated.rs")
            Path(mutated_path).write_text(mutated_src, encoding="utf-8")
            mutated_bin = os.path.join(tmp, "passthrough_harness_mutated")
            mut_compile = subprocess.run(
                [rustc, mutated_path, "-O", "-o", mutated_bin],
                capture_output=True,
                text=True,
            )
            if mut_compile.returncode != 0:
                self.fail(
                    "反向验证 harness 编译失败:\n"
                    + mut_compile.stdout
                    + mut_compile.stderr
                )
            mut_run = subprocess.run([mutated_bin], capture_output=True, text=True)
            print(mut_run.stdout)
            self.assertNotEqual(
                mut_run.returncode,
                0,
                "反向验证失败：去掉 virtual 分支后边界用例仍全部通过，"
                "说明 harness 没有真正验证原函数",
            )


    def test_own_android_private_path_access_boundary_guard(self) -> None:
        # 静态边界守卫：should_allow_own_android_private_path_access 的放行条件缺一不可——
        # 2026-09 真机报障（微信/QQ 打开自身私有目录内文件报“权限有问题”）的修复点就在
        # 这条判定链。任何一维被回退（owner 相等性、用户一致性、系统写入/媒体中间包排除、
        # 存储 uid 区间）都应在这里报警，而不是等真机再次进入绑定缺失状态。
        media_fuse = read("src/hook/media_fuse.rs")
        fn = extract_fn(media_fuse, "should_allow_own_android_private_path_access")

        # 调用 uid 必须达到应用区间。
        self.assertIn("caller_uid < writer::ANDROID_APP_UID_START", fn)
        # 路径必须归一化为 /storage/emulated/ 前缀。
        self.assertIn("normalize_storage_path(path)", fn)
        # 路径 user 与调用 uid 折算的 user 必须一致。
        self.assertIn("paths::extract_user_id_from_storage_path", fn)
        self.assertIn("platform::user_id_from_uid(caller_uid) != user_id", fn)
        # 目录所属包解析 + 系统写入包与媒体中间包排除。
        self.assertIn("paths::extract_android_private_path_owner", fn)
        self.assertIn("policy::is_media_intermediate_package", fn)
        self.assertIn("policy::is_system_writer_package", fn)
        # owner uid 与调用 uid 必须完全相等才放行。
        self.assertIn(
            "owner_uid >= writer::ANDROID_APP_UID_START && owner_uid == caller_uid", fn
        )

        # 链路接线守卫：判定必须在 native bridge 的 accessible 链尾生效，且 hook.rs 导出
        # 对应 C ABI；任何一环被移除，真机报障会原样复发。
        bridge = read("native/srx_lsplant_bridge.cpp")
        chain = bridge[
            bridge.index("bool ShouldAllowSrxAccessiblePath(") : bridge.index(
                "bool SrxFuseFixIsAppAccessiblePath("
            )
        ]
        self.assertIn("ShouldAllowOwnAndroidPrivatePathAccess", chain)
        hook = read("src/hook.rs")
        self.assertIn("srx_should_allow_fuse_own_android_private_path_access", hook)
        self.assertIn("should_allow_own_android_private_path_access", hook)

    def test_own_android_private_path_access_executed(self) -> None:
        # 从真实源码抽取 should_allow_own_android_private_path_access 及其纯函数依赖，
        # 注入 harness 模板后由 rustc 独立编译运行边界真值表；uid 查询与包名集合用桩。
        if not OWN_PRIVATE_HARNESS.exists():
            self.skipTest(f"harness 模板缺失: {OWN_PRIVATE_HARNESS}")
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest(
                "rustc 不可用：未编译执行 own-private harness，仅静态边界守卫生效 "
                "(test_own_android_private_path_access_boundary_guard)"
            )

        media_fuse_src = read("src/hook/media_fuse.rs")
        paths_src = read_paths_module()
        # 抽取落在 harness 的 `pub mod paths` 里，被 media_fuse 侧函数跨模块调用；
        # extract_fn 从 "fn name(" 起抽取会丢掉源码里的 `pub ` 前缀，这里补回。
        def pubify(fn_src: str) -> str:
            return fn_src if fn_src.startswith("pub ") else "pub " + fn_src

        media_fuse_fns = "\n".join(
            [
                extract_fn(media_fuse_src, "should_allow_own_android_private_path_access"),
                extract_fn(media_fuse_src, "normalize_storage_path"),
                extract_fn(media_fuse_src, "resolve_private_owner_uid"),
            ]
        )
        paths_fns = "\n".join(
            [
                pubify(extract_fn(paths_src, "extract_user_id_from_storage_path")),
                pubify(extract_fn(paths_src, "extract_android_private_path_owner")),
                pubify(extract_fn(paths_src, "is_valid_package_name")),
            ]
        )

        template = OWN_PRIVATE_HARNESS.read_text(encoding="utf-8")
        harness_src = template
        for placeholder, body in (
            ("// __INJECT_PATHS__", paths_fns),
            ("// __INJECT_MEDIA_FUSE__", media_fuse_fns),
        ):
            if placeholder not in harness_src:
                self.fail(f"harness 模板缺少占位符: {placeholder}")
            harness_src = harness_src.replace(placeholder, body)
        # 把抽取函数里的 `log::debug!` 改成 crate 根的 `debug!`（模板用 #[macro_use] 提供）。
        harness_src = harness_src.replace("log::debug!", "debug!")
        # 后续断言与反向验证替换都以替换后的注入文本为基准：模板里已不存在
        # 含 `log::debug!` 的原始函数文本。
        injected_media_fuse_fns = media_fuse_fns.replace("log::debug!", "debug!")

        with tempfile.TemporaryDirectory() as tmp:
            harness_path = os.path.join(tmp, "harness.rs")
            Path(harness_path).write_text(harness_src, encoding="utf-8")
            bin_path = os.path.join(tmp, "own_private_harness")
            compile_proc = subprocess.run(
                [rustc, harness_path, "-O", "-o", bin_path],
                capture_output=True,
                text=True,
            )
            if compile_proc.returncode != 0:
                self.fail(
                    "harness 编译失败:\n" + compile_proc.stdout + compile_proc.stderr
                )
            run_proc = subprocess.run([bin_path], capture_output=True, text=True)
            print(run_proc.stdout)
            self.assertEqual(
                run_proc.returncode,
                0,
                "harness 边界用例未全部通过:\n" + run_proc.stdout + run_proc.stderr,
            )
            self.assertIn("ALL", run_proc.stdout)

            # 反向验证：改坏 owner 相等判定（等效于修复合入前的形态）后，
            # 三条 owner 放行用例必须从通过变失败，退出码非 0。
            # 若 harness 只是空壳，改坏后仍会全过。
            mutated = injected_media_fuse_fns.replace(
                "owner_uid >= writer::ANDROID_APP_UID_START && owner_uid == caller_uid",
                "false",
            )
            self.assertNotEqual(mutated, injected_media_fuse_fns)
            self.assertIn(injected_media_fuse_fns, harness_src)
            mutated_src = harness_src.replace(injected_media_fuse_fns, mutated)
            mutated_path = os.path.join(tmp, "harness_mutated.rs")
            Path(mutated_path).write_text(mutated_src, encoding="utf-8")
            mutated_bin = os.path.join(tmp, "own_private_harness_mutated")
            mut_compile = subprocess.run(
                [rustc, mutated_path, "-O", "-o", mutated_bin],
                capture_output=True,
                text=True,
            )
            if mut_compile.returncode != 0:
                self.fail(
                    "反向验证 harness 编译失败:\n"
                    + mut_compile.stdout
                    + mut_compile.stderr
                )
            mut_run = subprocess.run([mutated_bin], capture_output=True, text=True)
            print(mut_run.stdout)
            self.assertNotEqual(
                mut_run.returncode,
                0,
                "反向验证失败：改坏 owner 相等判定后边界用例仍全部通过，"
                "说明 harness 没有真正验证原函数",
            )
            self.assertIn("FAIL own_media_owner_allowed", mut_run.stdout)
            self.assertIn("FAIL own_data_owner_allowed", mut_run.stdout)
            self.assertIn("FAIL own_obb_owner_allowed", mut_run.stdout)

    def test_redirect_parent_chain_blocks_target_self_nesting(self) -> None:
        """重定向父链创建必须先过目标自嵌套绊线。

        历史事故：readlink 反解回归窗口（已修复）曾让 MediaStore 往返把目标显示
        形态 `Android/data/<pkg>/sdcard` 再次送入改写，每轮深一层；父链的
        mkdir -p 把中间层全部物理创建，测试应用私有树一晚长出 284 层空目录，
        监视树重建被拖死（场景 24/25/27 全灭）。绊线要求：目标段在改写结果
        路径中出现两次即判定自嵌套，跳过父链创建并以告警日志留痕，操作以
        ENOENT 自然失败，让回归在场景断言层响亮暴露而不是静默长树。
        """
        runtime = read("src/hook/runtime.rs")
        ensure_fn = extract_fn(runtime, "ensure_redirect_parent_dirs")
        self.assertIn(
            "redirect_parent_chain_contains_target_cycle",
            ensure_fn,
            "ensure_redirect_parent_dirs 缺少目标自嵌套绊线调用",
        )
        # 绊线必须在任何父链创建之前执行，否则链已经创建再告警没有防护意义。
        self.assertLess(
            ensure_fn.index("redirect_parent_chain_contains_target_cycle"),
            ensure_fn.index("create_storage_parent_dirs_recursive"),
            "目标自嵌套绊线必须先于父链创建执行",
        )
        cycle_fn = extract_fn(runtime, "redirect_parent_chain_contains_target_cycle")
        self.assertIn("resolve_system_writer_redirect_target", cycle_fn)
        self.assertIn("path.matches(target_segment).count()", cycle_fn)
        self.assertIn("occurrences >= 2", cycle_fn)
        # 绊线命中必须直接返回（跳过创建），不能只告警不拦截。
        guard_block = cycle_fn[
            cycle_fn.index("let occurrences") : cycle_fn.index("\n    false\n}")
        ]
        self.assertIn("return true", guard_block)


if __name__ == "__main__":
    unittest.main()
