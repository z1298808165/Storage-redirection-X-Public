"""执行设备端权限准备脚本，验证版本边界与授权失败传播，不连接设备。"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / ".github/tests/grant-storage-test-permissions.sh"
SH = shutil.which("sh")
PERMISSIONS = (
    "READ_EXTERNAL_STORAGE", "WRITE_EXTERNAL_STORAGE", "ACCESS_MEDIA_LOCATION",
    "READ_MEDIA_IMAGES", "READ_MEDIA_VIDEO", "READ_MEDIA_AUDIO",
    "READ_MEDIA_VISUAL_USER_SELECTED", "POST_NOTIFICATIONS", "ACCESS_LOCAL_NETWORK",
)
STUB = r'''
import json, os, sys
from pathlib import Path
path = Path(os.environ["SRT_PERMISSION_STATE"])
state = json.loads(path.read_text())
name, args = Path(sys.argv[0]).name, sys.argv[1:]
if name == "getprop":
    print(state["sdk"])
elif name == "dumpsys":
    print("    requested permissions:")
    for permission in state["declared"]:
        print("    android.permission." + permission)
    print("    runtime permissions:")
    for permission in state["granted"]:
        print("      android.permission." + permission + ": granted=true")
elif name == "pm":
    permission = args[-1].removeprefix("android.permission.")
    if permission == state.get("fail_grant"):
        sys.exit(23)
    state["granted"].append(permission)
elif name == "cmd":
    assert args[0] == "appops", args
    if args[1] == "set":
        scope = "uid" if "--uid" in args else "package"
        state["operations"][scope + "/" + args[-2]] = args[-1]
    else:
        assert args[1] == "get", args
        operation = args[-1]
        for scope in ("uid", "package"):
            mode = state["operations"].get(scope + "/" + operation, "default")
            if operation == state.get("deny_readback") and scope == "uid":
                mode = "ignore"
            print(scope + ": " + mode)
else:
    raise AssertionError(name)
path.write_text(json.dumps(state))
'''


@unittest.skipUnless(SH and os.name == "posix", "需要 POSIX shell 执行设备命令替身")
class StoragePermissionTests(unittest.TestCase):
    def run_helper(self, sdk=37, declared=PERMISSIONS, **overrides):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            state_path = root / "state.json"
            state = dict(sdk=sdk, declared=declared, granted=[], operations={}, **overrides)
            state_path.write_text(json.dumps(state), encoding="utf-8")
            for command in ("getprop", "dumpsys", "pm", "cmd"):
                executable = root / command
                executable.write_text(f"#!{sys.executable}\n" + STUB, encoding="utf-8")
                executable.chmod(0o755)
            result = subprocess.run(
                [SH, str(HELPER), "test.storage"],
                env=dict(os.environ, PATH=directory + os.pathsep + os.environ["PATH"], SRT_PERMISSION_STATE=str(state_path)),
                text=True, capture_output=True, timeout=20,
            )
            return result, json.loads(state_path.read_text(encoding="utf-8"))

    def test_android_17_grants_declared_permissions_and_both_appops_scopes(self):
        result, state = self.run_helper()
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertEqual(set(PERMISSIONS), set(state["granted"]))
        for operation in ("WRITE_MEDIA_IMAGES", "WRITE_MEDIA_VIDEO", "WRITE_MEDIA_AUDIO", "MANAGE_EXTERNAL_STORAGE", "MANAGE_MEDIA"):
            for scope in ("uid", "package"):
                self.assertEqual("allow", state["operations"][scope + "/" + operation])
        self.assertIn("测试存储权限已就绪", result.stdout)

    def test_android_12_skips_newer_runtime_permissions_and_selected_media_op(self):
        result, state = self.run_helper(sdk=31)
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertEqual({"READ_EXTERNAL_STORAGE", "WRITE_EXTERNAL_STORAGE", "ACCESS_MEDIA_LOCATION"}, set(state["granted"]))
        self.assertNotIn("uid/READ_MEDIA_VISUAL_USER_SELECTED", state["operations"])

    def test_media_permission_version_boundaries(self):
        for sdk in (33, 34):
            with self.subTest(sdk=sdk):
                result, state = self.run_helper(sdk=sdk)
                self.assertEqual(0, result.returncode, result.stderr)
                self.assertIn("READ_MEDIA_IMAGES", state["granted"])
                self.assertIn("POST_NOTIFICATIONS", state["granted"])
                self.assertEqual(sdk >= 34, "READ_MEDIA_VISUAL_USER_SELECTED" in state["granted"])
                self.assertNotIn("ACCESS_LOCAL_NETWORK", state["granted"])

    def test_undeclared_permissions_are_not_granted(self):
        result, state = self.run_helper(declared=("READ_MEDIA_IMAGES",))
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertEqual(["READ_MEDIA_IMAGES"], state["granted"])
        self.assertNotIn("uid/ACCESS_MEDIA_LOCATION", state["operations"])

    def test_permission_grant_failure_stops_setup(self):
        result, state = self.run_helper(fail_grant="WRITE_EXTERNAL_STORAGE")
        self.assertEqual(23, result.returncode)
        self.assertFalse(state["operations"])
        self.assertNotIn("测试存储权限已就绪", result.stdout)

    def test_denied_uid_mode_is_not_hidden_by_allowed_package_mode(self):
        result, _ = self.run_helper(deny_readback="WRITE_MEDIA_IMAGES")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("测试 AppOps 未生效：WRITE_MEDIA_IMAGES", result.stderr)
        self.assertNotIn("测试存储权限已就绪", result.stdout)


if __name__ == "__main__":
    unittest.main()
