"""prune_ci_drafts 的草稿处置判定测试。"""

import importlib.util
import sys
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS_DIR))
SCRIPT_PATH = SCRIPTS_DIR / "prune_ci_drafts.py"
SPEC = importlib.util.spec_from_file_location("prune_ci_drafts", SCRIPT_PATH)
PRUNE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = PRUNE
SPEC.loader.exec_module(PRUNE)


NOW = datetime(2026, 10, 26, 15, 0, 0, tzinfo=timezone.utc)
RUN_AT = datetime(2026, 10, 26, 14, 0, 0, tzinfo=timezone.utc)
CURRENT_TAG = "ci-build-500"
CURRENT_VERSION = (2, 2, 0, 1)


def iso(moment: datetime) -> str:
    return moment.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def make_release(
    tag: str,
    name: str,
    created: datetime,
    *,
    draft: bool = True,
    assets: list[str] | None = None,
) -> dict:
    release = {
        "tag_name": tag,
        "name": name,
        "draft": draft,
        "created_at": iso(created),
    }
    if assets:
        release["assets"] = [{"name": asset} for asset in assets]
    return release


def decide(release: dict, *, published: bool = True, run_at: datetime | None = RUN_AT) -> PRUNE.Decision:
    return PRUNE.decide(
        release,
        current_tag=CURRENT_TAG,
        current_version=CURRENT_VERSION,
        current_run_at=run_at,
        published=published,
        now=NOW,
    )


def plan(
    releases: list[dict],
    *,
    published: bool = True,
    run_at: datetime | None = RUN_AT,
) -> list[tuple[str, PRUNE.Decision]]:
    return PRUNE.plan(
        releases,
        current_tag=CURRENT_TAG,
        current_version=CURRENT_VERSION,
        current_run_at=run_at,
        published=published,
        now=NOW,
    )


class PruneCiDraftsTest(unittest.TestCase):
    # ---- 本轮草稿永不删除 ----

    def test_current_run_draft_is_never_deleted(self) -> None:
        # 本轮草稿是 module/app 产物的唯一副本，失败后要靠它续接发布。
        release = make_release(CURRENT_TAG, "CI Build 2.2.0-ci.1", NOW - timedelta(days=7))
        decision = decide(release, published=False)
        self.assertEqual(decision.action, "keep")
        self.assertIn("current run", decision.reason)

    # ---- 已发布：被本轮取代的立即清理 ----

    def test_superseded_same_version_draft_is_deleted(self) -> None:
        # 同名不同内容：失败轮留下的 2.2.0-ci.1 与本轮发布版本相同、创建更早。
        release = make_release("ci-build-400", "CI Build 2.2.0-ci.1", RUN_AT - timedelta(hours=3))
        decision = decide(release)
        self.assertEqual(decision.action, "delete")
        self.assertIn("superseded", decision.reason)

    def test_superseded_older_version_draft_is_deleted(self) -> None:
        release = make_release("ci-build-399", "CI Build 2.1.0-ci.2", RUN_AT - timedelta(hours=3))
        self.assertEqual(decide(release).action, "delete")

    def test_newer_version_draft_is_kept(self) -> None:
        # 版本比本轮更新说明是更晚的轮次，不能用本轮去判定它废弃。
        release = make_release("ci-build-501", "CI Build 2.2.0-ci.3", RUN_AT - timedelta(hours=3))
        decision = decide(release)
        self.assertEqual(decision.action, "keep")

    def test_parallel_run_draft_is_kept(self) -> None:
        # 创建时间晚于本轮：可能是并行跑的另一轮，删了会让它没法续发。
        release = make_release("ci-build-501", "CI Build 2.2.0-ci.1", RUN_AT + timedelta(minutes=20))
        self.assertEqual(decide(release).action, "keep")

    def test_supersede_check_disabled_without_run_timestamp(self) -> None:
        release = make_release("ci-build-400", "CI Build 2.2.0-ci.1", RUN_AT - timedelta(hours=3))
        decision = decide(release, run_at=None)
        self.assertEqual(decision.action, "keep")

    # ---- 同一版本的多份失败草稿：逐个处理，不去重 ----

    def test_multiple_same_version_drafts_all_deleted(self) -> None:
        # 连续几轮都失败时序号不推进，后台会堆出多份同版本草稿；必须全部清掉。
        releases = [
            make_release(CURRENT_TAG, "CI Build 2.2.0-ci.1", NOW - timedelta(hours=4)),
            make_release("ci-build-401", "CI Build 2.2.0-ci.1", RUN_AT - timedelta(hours=3)),
            make_release("ci-build-402", "CI Build 2.2.0-ci.1", RUN_AT - timedelta(hours=2)),
            make_release("ci-build-403", "CI Build 2.2.0-ci.1", RUN_AT - timedelta(minutes=30)),
            make_release("ci-build-398", "CI Build 2.1.0-ci.2", RUN_AT - timedelta(hours=6)),
        ]
        decisions = dict(plan(releases))
        self.assertEqual(len(decisions), len(releases))
        self.assertEqual(decisions[CURRENT_TAG].action, "keep")
        for tag in ("ci-build-401", "ci-build-402", "ci-build-403", "ci-build-398"):
            self.assertEqual(decisions[tag].action, "delete", tag)

    def test_without_publish_older_version_deleted_but_same_version_kept(self) -> None:
        # 未发布时同版本草稿都还能续发，全部保留；更老版本已被本轮取代，直接清。
        releases = [
            make_release("ci-build-401", "CI Build 2.2.0-ci.1", RUN_AT - timedelta(hours=3)),
            make_release("ci-build-402", "CI Build 2.2.0-ci.1", RUN_AT - timedelta(hours=2)),
            make_release("ci-build-398", "CI Build 2.1.0-ci.2", RUN_AT - timedelta(hours=6)),
        ]
        decisions = dict(plan(releases, published=False))
        self.assertEqual(decisions["ci-build-401"].action, "keep")
        self.assertEqual(decisions["ci-build-402"].action, "keep")
        self.assertEqual(decisions["ci-build-398"].action, "delete")

    def test_parallel_run_same_version_survives_publish(self) -> None:
        # 并行轮次晚于本轮创建：即使本轮发布成功也不能删，它可能还要自己发。
        releases = [
            make_release("ci-build-501", "CI Build 2.2.0-ci.1", RUN_AT + timedelta(minutes=25)),
        ]
        self.assertEqual(plan(releases)[0][1].action, "keep")

    # ---- 未发布：保留续发窗口，超期清理 ----

    def test_unpublished_run_keeps_recent_draft(self) -> None:
        release = make_release("ci-build-400", "CI Build 2.2.0-ci.1", NOW - timedelta(hours=1))
        decision = decide(release, published=False)
        self.assertEqual(decision.action, "keep")
        self.assertIn("window", decision.reason)

    def test_unpublished_run_deletes_stale_draft(self) -> None:
        release = make_release("ci-build-400", "CI Build 2.2.0-ci.1", NOW - timedelta(hours=25))
        decision = decide(release, published=False)
        self.assertEqual(decision.action, "delete")
        self.assertIn("stale", decision.reason)

    def test_stale_draft_deleted_even_when_version_unknown(self) -> None:
        # 版本解析不出来时不做「已取代」判定，但超过窗口一样按废弃清理。
        release = make_release("ci-build-400", "构建与测试进行中。", NOW - timedelta(hours=30))
        self.assertEqual(decide(release).action, "delete")

    def test_recent_unknown_version_draft_is_kept(self) -> None:
        release = make_release("ci-build-400", "构建与测试进行中。", NOW - timedelta(hours=2))
        self.assertEqual(decide(release).action, "keep")

    # ---- 非目标对象一律不动 ----

    def test_published_release_is_ignored(self) -> None:
        release = make_release("ci-build-400", "CI Build 2.2.0-ci.1", NOW - timedelta(days=9), draft=False)
        decision = decide(release)
        self.assertEqual(decision.action, "keep")
        self.assertIn("already published", decision.reason)

    def test_non_ci_draft_is_ignored(self) -> None:
        release = make_release("v3.0.0-rc1", "Storage Redirect X v3.0.0-rc1", NOW - timedelta(days=9))
        decision = decide(release)
        self.assertEqual(decision.action, "keep")
        self.assertIn("not a ci-build", decision.reason)

    # ---- 版本解析 ----

    def test_version_falls_back_to_asset_name(self) -> None:
        release = make_release(
            "ci-build-400",
            "构建与测试进行中。",
            RUN_AT - timedelta(hours=3),
            assets=[
                "storage-redirect-x-manager-v2.2.0-ci.1.apk",
                "storage.redirect.x-v2.2.0-ci.1.zip",
            ],
        )
        self.assertEqual(decide(release).action, "delete")

    def test_version_comparison_is_numeric_not_lexical(self) -> None:
        self.assertLess(PRUNE.parse_ci_version("2.2.0-ci.9"), PRUNE.parse_ci_version("2.2.0-ci.10"))
        self.assertLess(PRUNE.parse_ci_version("2.1.0-ci.99"), PRUNE.parse_ci_version("2.2.0-ci.1"))

    def test_parse_rejects_non_ci_versions(self) -> None:
        self.assertIsNone(PRUNE.parse_ci_version("2.2.0"))
        self.assertIsNone(PRUNE.parse_ci_version("v2.2.0"))
        self.assertIsNone(PRUNE.parse_ci_version(""))
        self.assertIsNone(PRUNE.parse_ci_version(None))


if __name__ == "__main__":
    unittest.main()
