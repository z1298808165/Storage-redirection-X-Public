"""对照本轮创建回执、旧媒体行、扫描错误和夹具目录行。"""

import runpy
import shlex
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock


HERE = Path(__file__).resolve().parent
HELPER = HERE / "media-fixture-rows.py"


class MediaCleanupTest(unittest.TestCase):
    def test_only_successful_creates_own_a_row(self):
        uri = "content://media/external/file/44"
        create = f"PASS [mediastore_create_file] create succeeded (12ms) uri={uri}, fileName=srt_file_1.txt"
        lines = [
            create, create, create.replace("file/44", "file/44x"),
            create.replace("PASS ", "FAIL "), create.replace("create_file", "query_file"),
            create.replace("file/44", "file/"), create.replace("external/file", "internal/file"),
            "Row: 0 _id=12, _display_name=.pending-123-srt_file_1.txt",
            "Row: 1 _id=13, _display_name=.trashed-123-srt_file_1.txt",
        ]
        self.assertEqual([uri], list(runpy.run_path(str(HELPER))["created_uris"](lines)))

    def test_seed_and_created_rows_use_shell_identity_and_exact_ownership(self):
        parent = "/storage/emulated/0/Pictures/SrtReadOnlyMedia"
        collection = "content://media/external_primary/file"
        generated = "content://media/external/images/media"
        fixture_rows = runpy.run_path(str(HELPER))["fixture_rows"]
        for adb_uid in ("2000", "0"):
            with self.subTest(adb_uid=adb_uid), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                paths = {17: parent, 18: parent + "/srt_read_only_media.jpg",
                         41: "srt_image_2.jpg", 42: ".pending-123-srt_image_3.jpg",
                         43: "srt_image_1.jpg", 44: parent + "/family.jpg"}
                files = {i: root / p.rsplit("/", 1)[-1] for i, p in paths.items()}
                for path in files.values():
                    path.write_bytes(b"original")
                rows = dict(paths)
                calls = []
                def run(argv, **kwargs):
                    self.assertEqual(argv[:4], ["adb", "-s", "emulator-test", "shell"])
                    command = shlex.split(argv[4])
                    if command == ["id", "-u"]:
                        return subprocess.CompletedProcess(argv, 0, adb_uid, "")
                    if adb_uid == "0":
                        self.assertEqual(command[:2], ["su", "2000"])
                        command = command[2:]
                    self.assertEqual(command[0], "content")
                    op, uri = command[1], command[5]
                    self.assertEqual(command[2:5], ["--user", "0", "--uri"])
                    ids = list(rows) if uri == collection else [int(uri.rsplit("/", 1)[-1])]
                    if "--where" in command:
                        predicate = command[command.index("--where") + 1]
                        ids = [i for i in ids if i in rows and predicate == f"_data='{rows[i]}'"]
                    ids = [i for i in ids if i in rows]
                    if op == "query":
                        self.assertIn("--projection", command)
                        reply = "\n".join(f"Row: {n} _id={i}" for n, i in enumerate(ids)) or "No result found."
                    else:
                        self.assertEqual(op, "delete")
                        calls.append(uri)
                        reply = ""
                        for i in ids:
                            if i in (17, 18):
                                self.assertEqual(command[-2:], ["--where", f"_data='{paths[i]}'"])
                            files[i].unlink()
                            del rows[i]
                    return subprocess.CompletedProcess(argv, 0, reply, "")

                receipt = root / "result.txt"
                receipt.write_text("".join(
                    f"PASS [mediastore_create_image] create succeeded (5ms) uri={generated}/{i}, fileName=srt_image_1.jpg\n"
                    for i in (41, 42, 42)))
                with mock.patch.object(subprocess, "run", side_effect=run):
                    with self.assertRaisesRegex(ValueError, "索引冲突"):
                        fixture_rows("check", "emulator-test")
                    self.assertEqual(calls, [])
                    fixture_rows("clean", "emulator-test", receipt)
                    fixture_rows("check", "emulator-test")
                self.assertEqual(calls, [f"{generated}/41", f"{generated}/42", f"{collection}/18", f"{collection}/17"])
                self.assertEqual(set(rows), {43, 44})
                self.assertEqual({i for i, path in files.items() if path.is_file()}, {43, 44})
                self.assertTrue(all(files[i].read_bytes() == b"original" for i in rows))

    def test_provider_failure_or_unchanged_row_fails_cleanup(self):
        fixture_rows = runpy.run_path(str(HELPER))["fixture_rows"]
        raw = "Row: 0 _id=18"
        for replies, error in (
            (["",], "无法核对媒体索引"),
            (["Provider unavailable"], "无法核对媒体索引"),
            ([raw, "Error while accessing provider:media"], "删除媒体条目"),
            ([raw, "", raw], "删除后仍有夹具索引"),
        ):
            with self.subTest(replies=replies):
                output = [subprocess.CompletedProcess([], 0, value, "") for value in ["2000", *replies]]
                with mock.patch.object(subprocess, "run", side_effect=output), self.assertRaisesRegex(ValueError, error):
                    fixture_rows("clean", "")
        with tempfile.TemporaryDirectory() as tmp:
            receipt = Path(tmp) / "result"
            receipt.write_text("".join(
                f"PASS [mediastore_create_file] create succeeded (4ms) uri=content://media/external/file/{i}, fileName=test\n"
                for i in (41, 42)))
            replies = ["2000", "Error while accessing provider:media", "", "No result found.", "No result found.", "No result found."]
            output = [subprocess.CompletedProcess([], 0, value, "") for value in replies]
            with mock.patch.object(subprocess, "run", side_effect=output) as adb, self.assertRaisesRegex(RuntimeError, "/41"):
                fixture_rows("clean", "", receipt)
            self.assertEqual(adb.call_count, len(replies))
