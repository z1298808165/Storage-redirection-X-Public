import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


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


if __name__ == "__main__":
    unittest.main()
