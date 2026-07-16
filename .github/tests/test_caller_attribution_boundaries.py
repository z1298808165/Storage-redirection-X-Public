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
        self.assertIn("get_caller_read_only_paths(caller_package, caller_uid)", writer)
        self.assertIn("inferred_uid != *effective_caller_uid", writer)
        self.assertIn("policy::is_system_writer_package(effective_caller_package)", writer)


if __name__ == "__main__":
    unittest.main()
