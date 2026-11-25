"""对原始记录有意造错，防止恢复框架把故障构造失败或边界保护误报为通过。"""

import copy
import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("storage_view_runner", Path(__file__).with_name("run-storage-view-recovery.py"))
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)
TARGET = "/storage/emulated"


def mount(identity: int, parent: int, dev: str) -> str:
    return f"{identity} {parent} {dev} / {TARGET} rw - fuse /dev/fuse rw\n"


class MountObservationTest(unittest.TestCase):
    def test_incomplete_mount_line_cannot_confirm_a_visible_layer(self):
        snapshot = {"visible": {TARGET: "7"}, "mountinfo": f"7 1 0:38 / {TARGET} rw\n"}
        self.assertIsNone(RUNNER.visible_layer(snapshot, TARGET))
        snapshot["mountinfo"] += mount(7, 1, "0:38")
        self.assertEqual(RUNNER.visible_layer(snapshot, TARGET)["dev"], "0:38")


class StorageViewReportingTest(unittest.TestCase):
    def setUp(self) -> None:
        self.before = dict(pid="42", start="20", uid="10100", namespace="mnt:[1]", config="policy", global_="global", host="pid=51\nstart_ticks=10\ndev=0:38\n", mountinfo=mount(200, 1, "0:38"))
        self.before["global"] = self.before.pop("global_")
        self.before["init_mountinfo"] = mount(99, 1, "0:95").replace(TARGET, "/mnt/user/0/emulated")
        self.fault = {**self.before, "mountinfo": self.before["mountinfo"] + mount(5, 200, "0:95")}
        self.after = {**self.before, "mountinfo": mount(3, 1, "0:38")}

    def check(self, before=None, fault=None, after=None, foreign=False):
        RUNNER.validate(before or self.before, fault or self.fault, after or self.after, [TARGET], foreign)

    def test_fault_witness_precedes_recovery_between_device_calls(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            device = RUNNER.Device("emulator-test", Path(tmp))
            calls = []

            def shell(command):
                calls.append(command)
                if "printf '\\000'" in command:
                    fixture = "cat() { printf '%s' \"$view\"; }; view=healthy; "
                    return subprocess.run(["sh", "-ec", fixture + command], capture_output=True, text=True, check=True).stdout
                if command.endswith("/mountinfo"):
                    return "recovered"
                if command.endswith("/stat"):
                    return "42 (app) " + " ".join(["0"] * 20)
                if command.endswith("/status"):
                    return "Uid: 10100"
                return "metadata"

            device.su = shell
            witness = device.snapshot("42", "fault", inject=("view=covered", "printf 'applied\\n'"))
            self.assertIn("view=covered && printf 'applied\\n'", calls[0])
            self.assertEqual(witness["mountinfo"], "covered")
            self.assertEqual(witness["init_mountinfo"], "recovered")
            self.assertEqual((Path(tmp) / "fault-injection.txt").read_text(), "applied\n")
            self.assertEqual(json.loads((Path(tmp) / "fault.json").read_text()), witness)
            with self.assertRaises(subprocess.CalledProcessError):
                device.snapshot("42", "incomplete", inject=("false", "view=covered"))
            self.assertFalse((Path(tmp) / "incomplete.json").exists())

    def test_privileged_commands_use_root_adb_directly(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        calls = []

        def adb(*args, **kwargs):
            calls.append(args)
            return "0\n" if args == ("shell", "id -u") else "done\n"

        device.adb = adb
        self.assertEqual(device.su("touch /tmp/test"), "done\n")
        self.assertEqual(calls, [("shell", "id -u"), ("shell", "touch /tmp/test")])

    def test_privileged_commands_use_kernel_su_uid_syntax(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        calls = []

        def adb(*args, **kwargs):
            calls.append(args)
            if args == ("shell", "id -u"):
                return "2000\n"
            if args == ("shell", "su 0 id -u"):
                return "0\n"
            return "done\n"

        device.adb = adb
        self.assertEqual(device.su("touch /tmp/test"), "done\n")
        self.assertEqual(calls, [
            ("shell", "id -u"),
            ("shell", "su 0 id -u"),
            ("shell", "su 0 sh -c 'touch /tmp/test'"),
        ])

    def test_permission_preparation_budget_does_not_expand_observation_calls(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        device._root_shell = True
        result = RUNNER.subprocess.CompletedProcess([], 0, "ready\n", "")
        with patch.object(RUNNER.subprocess, "run", return_value=result) as run:
            self.assertEqual(device.su("permissions", timeout=180), "ready\n")
            self.assertEqual(run.call_args.kwargs["timeout"], 180)
            device.su("cat /proc/1/mountinfo")
            self.assertEqual(run.call_args.kwargs["timeout"], 45)

    def test_invalid_app_result_preserves_raw_record(self) -> None:
        raw = "FAIL [file_write] EACCES"
        with self.assertRaises(AssertionError) as error:
            RUNNER.validate_app_result(raw, "file_write")
        self.assertIn(raw, str(error.exception))

    def test_alias_uses_app_credentials_and_checks_physical_destination(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        parent = "/mnt/user/10/emulated"
        trailer = "\n__SRT_RESULT_END__=1\n__SRT_RESULT_TOKEN__=" + "a" * 32
        denied = "FAIL [file_stat] stat failed: EACCES (Permission denied)"
        with patch.object(device, "app_case", return_value=denied + trailer) as stat, patch.object(device, "app_write") as write:
            self.assertFalse(device.check_alias_io(10, "alias", parent))
            stat.assert_called_once_with(10, "alias-stat", "file_stat", file_path=parent + "/10/Download")
            write.assert_not_called()
            for raw in (denied, "FAIL [file_stat] stat failed: ENOENT" + trailer, "FAIL [file_write] EACCES" + trailer):
                stat.return_value = raw
                with self.assertRaises(AssertionError):
                    device.check_alias_io(10, "alias", parent)
            stat.return_value = "PASS [file_stat] stat succeeded" + trailer
            write.return_value = dict(raw="PASS [file_write] success" + trailer, payload="data", private="data", real="__absent__", metadata="1000:1078:660")
            self.assertTrue(device.check_alias_io(10, "alias", parent))
            write.assert_called_once_with(10, "alias", "issue18-alias", parent=parent)
            write.return_value["real"] = "data"
            with self.assertRaises(AssertionError):
                device.check_alias_io(10, "alias", parent)

    def test_normal_and_foreign_controls(self) -> None:
        self.check()
        foreign = {**self.fault, "mountinfo": self.fault["mountinfo"].replace("fuse /dev/fuse", "tmpfs issue18-foreign")}
        self.check(fault=foreign, after=foreign, foreign=True)
        self.assertEqual(RUNNER.top(self.fault["mountinfo"], TARGET)["id"], 5)

    def test_identity_policy_or_host_changes_are_failures(self) -> None:
        for stage in ("fault", "after"):
            for key in ("pid", "start", "uid", "namespace", "config", "global", "host"):
                with self.subTest(stage=stage, field=key), self.assertRaises(AssertionError):
                    record = copy.deepcopy(getattr(self, stage))
                    record[key] = "changed"
                    self.check(**{stage: record})

    def test_other_user_view_and_identity_must_remain_unchanged(self) -> None:
        before = {**self.before, "ledger": f"mount=200\t0:38\t/dev/fuse\t{TARGET}\n"}
        RUNNER.validate_isolation(before, before)
        with self.assertRaisesRegex(AssertionError, "健康的 SRX 视图"):
            RUNNER.validate_isolation({**before, "ledger": "__absent__"}, {**before, "ledger": "__absent__"})
        for key in ("pid", "uid", "namespace", "host", "mountinfo", "ledger"):
            with self.subTest(field=key), self.assertRaises(AssertionError):
                RUNNER.validate_isolation(before, {**before, key: "changed"})

    def test_missing_fault_and_false_recovery_are_failures(self) -> None:
        cases = [
            dict(fault=self.before),
            dict(fault={**self.fault, "mountinfo": ""}),
            dict(after=self.fault),
            dict(after={**self.after, "mountinfo": ""}),
            dict(after={**self.after, "mountinfo": mount(200, 1, "0:95") + mount(3, 200, "0:38")}),
            dict(after=self.after, foreign=True),
        ]
        for case in cases:
            with self.subTest(case=case), self.assertRaises(AssertionError):
                self.check(**case)

    def test_recovery_requires_exact_ledger_identity(self) -> None:
        after = {**self.after, "ledger": f"mount=3\t0:38\t/dev/fuse\t{TARGET}\n"}
        self.check(after=after)
        self.assertTrue(RUNNER.view_restored(after, TARGET, "0:38"))
        for ledger in ("", f"mount=200\t0:38\t/dev/fuse\t{TARGET}\n", f"mount=3\t0:95\t/dev/fuse\t{TARGET}\n"):
            self.assertFalse(RUNNER.view_restored({**after, "ledger": ledger}, TARGET, "0:38"))
            with self.subTest(ledger=ledger), self.assertRaises(AssertionError):
                self.check(after={**after, "ledger": ledger})

    def test_invalid_system_fault_cannot_pass(self) -> None:
        for key, value in (("init_mountinfo", ""), ("init_mountinfo", self.before["mountinfo"]), ("mountinfo", mount(9, 1, "0:999"))):
            with self.subTest(field=key, value=value), self.assertRaises(AssertionError):
                self.check(fault={**self.fault, key: value})
        with self.assertRaises(AssertionError):
            RUNNER.validate(self.before, self.fault, self.after, [], False)

    def test_single_alias_fault_requires_an_independent_healthy_control(self) -> None:
        alias = "/mnt/user/0/emulated"
        for target, control in ((TARGET, alias), (alias, TARGET)):
            with self.subTest(target=target):
                control_layer = mount(201, 1, "0:38").replace(TARGET, control)
                snapshots = []
                for sample, identifier in ((self.before, 200), (self.fault, 200), (self.after, 3)):
                    snapshots.append({**sample, "mountinfo": sample["mountinfo"].replace(TARGET, target) + control_layer,
                                      "ledger": f"mount={identifier}\t0:38\t/dev/fuse\t{target}\nmount=201\t0:38\t/dev/fuse\t{control}\n"})
                RUNNER.validate(*snapshots, [target], False)
                for index in (1, 2):
                    damaged = list(snapshots)
                    damaged[index] = {**damaged[index], "mountinfo": damaged[index]["mountinfo"].replace(control_layer, "")}
                    with self.assertRaisesRegex(AssertionError, "对照别名"):
                        RUNNER.validate(*damaged, [target], False)

    def test_app_output_and_backend_disagreement_are_failures(self) -> None:
        raw = "PASS [file_write] success\n__SRT_RESULT_END__=1\n__SRT_RESULT_TOKEN__=" + "a" * 32 + "\n"
        record = dict(raw=raw, payload="data", private="data", real="__absent__", metadata="10100:1078:660")
        RUNNER.validate_io(record)
        corrupt = [
            {"raw": raw.replace("PASS", "FAIL")},
            {"raw": "FAIL [file_write] failed\n" + raw},
            {"raw": raw + "PASS [file_write] success\n"},
            {"raw": raw.replace("__SRT_RESULT_END__=1", "__SRT_RESULT_END__=0")},
            {"raw": raw.replace("__SRT_RESULT_TOKEN__=", "__SRT_RESULT_TOKEN__=invalid")},
            {"raw": raw + "__SRT_RESULT_END__=0\n"},
            {"raw": raw + "__SRT_RESULT_TOKEN__=invalid\n"},
            {"raw": raw + "__SRT_RESULT_UNKNOWN__=1\n"},
            {"raw": raw + raw}, {"private": "wrong"}, {"real": "data"}, {"metadata": ""},
        ]
        for change in corrupt:
            with self.subTest(change=change), self.assertRaises(AssertionError):
                RUNNER.validate_io({**record, **change})
        with self.assertRaises(AssertionError):
            RUNNER.validate_io(record, "0:0:777")

    def test_delayed_attachment_requires_system_first_and_same_live_process(self) -> None:
        before = {
            **self.before,
            "pid": "41",
            "start": "19",
            "config": json.dumps({"users": {"0": {"enabled": False}}}),
            "mountinfo": mount(99, 1, "0:95"),
            "ledger": "__absent__",
        }
        after = {
            **self.after,
            "pid": before["pid"],
            "start": before["start"],
            "namespace": before["namespace"],
            "config": json.dumps({"users": {"0": {"enabled": True}}}),
            "ledger": f"mount=3\t0:38\t/dev/fuse\t{TARGET}\n",
        }
        RUNNER.validate_system_first_attachment(before, after, 0)
        mutants = (
            {**before, "mountinfo": self.before["mountinfo"]},
            {**after, "mountinfo": mount(3, 1, "0:95")},
            {**after, "config": before["config"]},
            {**after, "pid": "43"},
            {**after, "start": "21"},
            {**after, "namespace": "mnt:[2]"},
            {**after, "uid": "1010100"},
            {**after, "global": "changed"},
            {**after, "config": json.dumps({"users": {"0": {"enabled": True, "allowed_real_paths": ["*"]}}})},
            {**after, "host": "pid=52\nstart_ticks=11\ndev=0:39\n"},
            {**after, "ledger": ""},
        )
        for mutant in mutants:
            with self.subTest(mutant=mutant), self.assertRaises(AssertionError):
                RUNNER.validate_system_first_attachment(before, mutant, 0)

    def test_delayed_wait_observes_the_existing_process(self) -> None:
        before = {
            **self.before,
            "pid": "41",
            "start": "19",
            "config": json.dumps({"users": {"0": {"enabled": False}}}),
            "mountinfo": mount(99, 1, "0:95"),
            "ledger": "__absent__",
        }
        after = {
            **self.after,
            "pid": before["pid"],
            "start": before["start"],
            "namespace": before["namespace"],
            "config": json.dumps({"users": {"0": {"enabled": True}}}),
            "ledger": f"mount=3\t0:38\t/dev/fuse\t{TARGET}\n",
        }

        class Device:
            def __init__(self):
                self.snapshots = []

            def snapshot(self, pid, name):
                self.snapshots.append((pid, name))
                return after

        device = Device()
        result, observations = RUNNER.wait_for_system_first_attachment(device, before, 0, "delay")
        self.assertEqual(result, after)
        self.assertEqual(observations, 1)
        self.assertEqual(device.snapshots, [(before["pid"], "delay-observed-0")])

    def test_unredirected_control_requires_physical_only_write(self) -> None:
        raw = "PASS [file_write] success\n__SRT_RESULT_END__=1\n__SRT_RESULT_TOKEN__=" + "a" * 32 + "\n"
        record = dict(raw=raw, payload="data", private="__absent__", real="data")
        RUNNER.validate_unredirected_io(record)
        for change in ({"private": "data"}, {"real": "__absent__"}, {"raw": raw.replace("PASS", "FAIL")}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                RUNNER.validate_unredirected_io({**record, **change})

    def test_cleanup_attempts_every_step_after_a_failure(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        device.owned_files = ["/data/media/0/issue18-owned"]
        calls = []

        def stop(*args):
            calls.append("stop")
            raise RuntimeError("expected stop failure")

        device.adb = stop
        device.write = lambda path, content: calls.append(path)
        device.su = lambda command: calls.append(command)
        with self.assertRaisesRegex(RuntimeError, "expected stop failure"):
            device.cleanup(11, {"config": "original", "global": "original"})
        self.assertEqual(calls[:3], ["stop", "config", "global"])
        self.assertIn("rm -f /data/media/0/issue18-owned", calls)
        self.assertIn("rm -rf " + device.remote, calls)
        self.assertIn(RUNNER.LOCK, calls[-1])

    def test_failed_app_launch_keeps_owned_files_in_cleanup(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))

        def failed_launch(*args):
            raise RuntimeError("expected launch failure")

        device.adb = failed_launch
        with self.assertRaisesRegex(RuntimeError, "expected launch failure"):
            device.app_write(11, "owned", "data")
        self.assertEqual(device.owned_files, [
            f"/data/media/11/Android/data/{RUNNER.APP}/sdcard/Download/issue18-owned.txt",
            "/data/media/11/Download/issue18-owned.txt",
        ])

    def test_background_user_start_uses_the_test_service(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        calls = []
        device.adb = lambda *args: calls.append(args) or ("42" if "pidof" in args[-1] else "")
        device.su = lambda command: "Uid:\t1110100\n"
        observations = []

        def snapshot(*args):
            observations.append(args)
            self.assertFalse(any(isinstance(call[0], int) and call[2] == "file_mkdir" for call in calls))
            return {**self.before, "ledger": "current" if len(observations) > 1 else "__absent__"}

        device.snapshot = snapshot
        device.app_case = lambda user, name, case, timeout=45, **extras: calls.append((user, name, case, extras)) or f"PASS [{case}] success\n__SRT_RESULT_END__=1\n__SRT_RESULT_TOKEN__=" + "a" * 32 + "\n"

        def app_write(user, name, payload):
            calls.append((user, name))
            return dict(raw="PASS [file_write] success\n__SRT_RESULT_END__=1\n__SRT_RESULT_TOKEN__=" + "a" * 32 + "\n", payload=payload, private=payload, real="__absent__", metadata="1110100:1078:660")

        device.app_write = app_write
        with patch.object(RUNNER.time, "sleep"):
            self.assertEqual(device.start(11), "42")
        self.assertEqual(calls[0], ("shell", f"am force-stop --user 11 {RUNNER.APP}"))
        self.assertEqual(calls[1][2:], ("file_stat", {"file_path": f"/data/user/11/{RUNNER.APP}"}))
        self.assertEqual(len(observations), 2)
        mkdir = next(call for call in calls if isinstance(call[0], int) and call[2] == "file_mkdir")
        self.assertEqual(mkdir[3], {"file_dir": "/storage/emulated/11/Download"})
        self.assertFalse(any("MainActivity" in str(call) for call in calls))

    def test_failed_startup_stops_before_control_write(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        device.adb = lambda *args: ""
        device.app_case = lambda *args, **extras: "FAIL [file_stat] denied\n"
        device.app_write = lambda *args: self.fail("write ran after failed launch")
        with self.assertRaisesRegex(AssertionError, "单次成功操作"):
            device.start(11)

    def test_system_first_waits_for_the_selected_volume_before_app_launch(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        calls = []
        device.adb = lambda *args: calls.append(args)
        states = iter(("pending", "ready"))
        device.su = lambda command: calls.append(command) or next(states)
        with patch.object(RUNNER.time, "sleep"):
            device.prepare_user(11)
        self.assertEqual(calls, [("shell", "am start-user 11"), calls[1], calls[1]])
        self.assertIn("am get-started-user-state 11", calls[1])
        self.assertIn("^emulated;11 mounted ", calls[1])
        self.assertIn("/proc/1/root/mnt/user/11/emulated/11", calls[1])

    def test_system_volume_timeout_keeps_the_app_stopped(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        device.adb = lambda *args: ""
        device.su = lambda command: "pending"
        device.app_case = lambda *args, **kwargs: self.fail("app launched before the volume was ready")
        with patch.object(RUNNER.time, "monotonic", side_effect=[0, 900]), self.assertRaisesRegex(AssertionError, "用户 11 的存储"):
            device.prepare_user(11)

    def test_result_timeout_keeps_the_stale_raw_record(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            device = RUNNER.Device("emulator-test", Path(directory))
            device.adb = lambda *args: "launched"
            device.read = lambda path: "previous result"
            with patch.object(RUNNER.time, "monotonic", side_effect=[0, 0, 46]), patch.object(RUNNER.time, "sleep"):
                with self.assertRaisesRegex(AssertionError, "previous result"):
                    device.app_case(0, "timeout", "file_stat")
            self.assertEqual(Path(directory, "timeout-result.txt").read_text(), "previous result")

    def test_startup_rejects_a_changed_process_before_baseline_io(self) -> None:
        device = RUNNER.Device("emulator-test", Path("unused"))
        device.adb = lambda *args: "42" if "pidof" in args[-1] else ""
        device.su = lambda command: "Uid:\t1110100\n"
        device.app_case = lambda *args, **extras: "PASS [file_stat] success\n__SRT_RESULT_END__=1\n__SRT_RESULT_TOKEN__=" + "a" * 32 + "\n"
        snapshots = iter(({**self.before, "ledger": "__absent__"}, {**self.before, "start": "21", "ledger": "current"}))
        device.snapshot = lambda *args: next(snapshots)
        device.app_write = lambda *args: self.fail("write ran after process identity changed")
        with patch.object(RUNNER.time, "sleep"), self.assertRaisesRegex(AssertionError, "启动期间应用进程或命名空间发生变化"):
            device.start(11)


class NamespaceViewReportingTest(unittest.TestCase):
    def setUp(self) -> None:
        self.user = 10
        self.before = dict(pid="42", start="20", uid="1010100", namespace="mnt:[1]", config="policy", host="pid=51\nstart_ticks=10\ndev=0:38\n")
        self.before["global"] = "global"
        self.before["init_mountinfo"] = "9 1 0:95 / /mnt/user/10/emulated rw - fuse /dev/fuse rw\n"
        self.before.update(mountinfo="", ledger="", visible={})
        self.fault = copy.deepcopy(self.before)
        self.after = copy.deepcopy(self.before)
        for index, parent in enumerate(RUNNER.storage_parents(self.user)):
            target = f"{parent}/{self.user}"
            base, old, cover, new = (offset + index for offset in (100, 200, 300, 400))
            root = f"/media/{self.user}/Android/data/{RUNNER.APP}/sdcard"
            parent_line = f"{base} 1 0:95 / {parent} rw - fuse /dev/fuse rw\n"
            old_line = f"{old} {base} 254:1 {root} {target} rw - ext4 rootfs rw\n"
            cover_line = f"{cover} {base} 0:95 / {parent} rw - fuse /dev/fuse rw\n"
            new_line = f"{new} {cover} 254:1 {root} {target} rw - ext4 rootfs rw\n"
            self.before["mountinfo"] += parent_line + old_line
            self.before["ledger"] += f"mount={old}\t254:1\trootfs\t{target}\n"
            self.before["visible"].update({parent: base, target: old})
            self.fault["mountinfo"] += parent_line + old_line + cover_line
            self.fault["ledger"] += f"mount={old}\t254:1\trootfs\t{target}\n"
            self.fault["visible"].update({parent: cover, target: cover})
            self.after["mountinfo"] += parent_line + old_line + cover_line + new_line
            self.after["ledger"] += f"mount={new}\t254:1\trootfs\t{target}\n"
            self.after["visible"].update({parent: cover, target: new})

    def test_hidden_children_recover_to_matching_visible_ledger(self) -> None:
        RUNNER.validate_namespace(self.before, self.fault, self.after, self.user, "cover")

    def test_recovery_can_select_the_shared_host_backend(self) -> None:
        after = copy.deepcopy(self.after)
        after.update(mountinfo="", ledger="", visible={})
        for index, parent in enumerate(RUNNER.storage_parents(self.user)):
            identifier = 500 + index
            after["mountinfo"] += f"{identifier} 1 0:38 / {parent} rw - fuse /dev/fuse rw\n"
            after["ledger"] += f"mount={identifier}\t0:38\t/dev/fuse\t{parent}\n"
            after["visible"].update({parent: identifier, f"{parent}/{self.user}": identifier})
        RUNNER.validate_namespace(self.before, self.fault, after, self.user, "cover")
        for key, value in (("mountinfo", after["mountinfo"].replace("0:38", "0:99")), ("ledger", self.before["ledger"])):
            with self.subTest(field=key), self.assertRaises(AssertionError):
                RUNNER.validate_namespace(self.before, self.fault, {**after, key: value}, self.user, "cover")

    def test_false_visibility_identity_and_recovery_are_rejected(self) -> None:
        for stage, key, value in (
            ("fault", "visible", self.before["visible"]),
            ("after", "visible", self.fault["visible"]),
            ("after", "ledger", self.before["ledger"]),
            ("after", "namespace", "changed"),
            ("fault", "config", "changed"),
            ("fault", "init_mountinfo", ""),
            ("before", "visible", {}),
        ):
            inputs = {name: copy.deepcopy(getattr(self, name)) for name in ("before", "fault", "after")}
            inputs[stage][key] = value
            with self.subTest(stage=stage, field=key), self.assertRaises(AssertionError):
                RUNNER.validate_namespace(**inputs, user=self.user, mode="cover")
        with self.assertRaises(AssertionError):
            RUNNER.validate_namespace(self.before, self.fault, self.after, 0, "cover")

    def test_missing_child_requires_actual_removal(self) -> None:
        with self.assertRaisesRegex(AssertionError, "子挂载仍然存在"):
            RUNNER.validate_namespace(self.before, self.fault, self.after, self.user, "missing")
        fault = copy.deepcopy(self.fault)
        old_ids = {str(self.before["visible"][f"{parent}/{self.user}"]) for parent in RUNNER.storage_parents(self.user)}
        fault["mountinfo"] = "\n".join(line for line in fault["mountinfo"].splitlines() if line.split()[0] not in old_ids)
        RUNNER.validate_namespace(self.before, fault, self.after, self.user, "missing")

    def test_foreign_ancestor_requires_full_observation_to_stay_unchanged(self) -> None:
        fault = copy.deepcopy(self.fault)
        fault["mountinfo"] = fault["mountinfo"].replace("0:95 /", "0:99 /").replace("- fuse /dev/fuse", "- tmpfs issue18-foreign")
        for parent in RUNNER.storage_parents(self.user):
            fault["visible"][f"{parent}/{self.user}"] = None
        RUNNER.validate_namespace(self.before, fault, fault, self.user, "foreign")
        for key in ("mountinfo", "ledger"):
            changed = {**fault, key: fault[key] + "unexpected\n"}
            with self.subTest(field=key), self.assertRaises(AssertionError):
                RUNNER.validate_namespace(self.before, fault, changed, self.user, "foreign")


if __name__ == "__main__":
    unittest.main()
