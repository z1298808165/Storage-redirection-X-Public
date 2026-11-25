import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(shutil.which("sh"), "sh unavailable")
class DownloadTestNetworkTest(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.state = self.root / "download-network"
        source = (ROOT / ".github/tests/prepare-download-test-network.sh").read_text()
        self.script = self.root / "prepare.sh"
        self.script.write_text(source.replace(
            "/data/local/tmp/srx-test-flow.lock/download-network", str(self.state)
        ))
        self.mock_state = self.root / "network.json"
        self.network = {
            "qemu": "1", "sdk": "37", "validated": False, "connect": True,
            "settings": {"mobile_data": "1", "wifi_on": "0"}, "calls": [],
            "connections": {"data": True, "wifi": False},
            "disconnect_pending": {}, "disconnect_stuck": False, "dumpsys_fails": False,
        }
        mock = self.root / "mock"
        mock.write_text(textwrap.dedent("""\
            #!/usr/bin/env python3
            import json
            import os
            from pathlib import Path
            import sys

            path = Path(os.environ["SRT_MOCK_NETWORK"])
            data = json.loads(path.read_text())
            command = Path(sys.argv[0]).name
            args = sys.argv[1:]
            data["calls"].append([command, *args])
            status = 0
            if command == "getprop":
                print(data["qemu"] if args[0] == "ro.kernel.qemu" else data["sdk"])
            elif command == "settings":
                action, _, key, *values = args
                if action == "get":
                    print(data["settings"].get(key, "null"))
                elif action == "put":
                    data["settings"][key] = values[0]
                else:
                    data["settings"].pop(key, None)
            elif command == "svc":
                transport = args[0]
                key = "mobile_data" if transport == "data" else "wifi_on"
                data["settings"][key] = "1" if args[1] == "enable" else "0"
                if args[1] == "disable":
                    data["disconnect_pending"][transport] = 2
                    data["validated"] = False
                else:
                    reconnected = not data["connections"][transport] and data["connect"]
                    data["connections"][transport] |= data["connect"]
                    data["disconnect_pending"].pop(transport, None)
                    data["validated"] |= (reconnected
                        and data["settings"].get("captive_portal_mode") == "0")
            elif command == "dumpsys":
                status = int(data["dumpsys_fails"])
                for transport in list(data["disconnect_pending"]):
                    if not data["disconnect_stuck"]:
                        data["disconnect_pending"][transport] -= 1
                        if data["disconnect_pending"][transport] == 0:
                            data["connections"][transport] = False
                            del data["disconnect_pending"][transport]
                print("Active default network:", data.get("default_id", 100))
                if data.get("other_validated"):
                    print("  NetworkAgentInfo{network{101} nc{[ Transports: VPN Capabilities: INTERNET&VALIDATED ]}}")
                capability = "VALIDATED" if data["validated"] else "PARTIAL_CONNECTIVITY"
                for transport, connected in data["connections"].items():
                    if connected:
                        kind = "CELLULAR" if transport == "data" else "WIFI"
                        print("  NetworkAgentInfo{network{100} nc{[ Transports: " + kind
                            + " Capabilities: INTERNET&TRUSTED&" + capability + "&NOT_ROAMING ]}}")
            path.write_text(json.dumps(data))
            sys.exit(status)
            """))
        mock.chmod(0o755)
        for command in ("getprop", "settings", "svc", "dumpsys", "sleep"):
            (self.root / command).symlink_to(mock)

    def invoke(self, action: str) -> subprocess.CompletedProcess:
        self.mock_state.write_text(json.dumps(self.network))
        result = subprocess.run(
            ["sh", str(self.script), action], text=True, capture_output=True,
            env={**os.environ, "PATH": f"{self.root}{os.pathsep}{os.environ['PATH']}",
                 "SRT_MOCK_NETWORK": str(self.mock_state)}, timeout=15,
        )
        self.network = json.loads(self.mock_state.read_text())
        return result

    def test_reconnect_validates_network_and_restores_absent_setting(self) -> None:
        result = self.invoke("prepare")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("download_network_validated", result.stdout)
        self.assertEqual(self.state.read_text().splitlines(), ["null", "1", "0"])
        self.assertEqual(self.network["settings"]["captive_portal_mode"], "0")
        self.assertTrue(self.network["validated"])
        calls = self.network["calls"]
        disabled = calls.index(["svc", "data", "disable"])
        enabled = calls.index(["svc", "data", "enable"])
        self.assertEqual(calls[disabled + 1:enabled].count(["dumpsys", "connectivity"]), 2)
        result = self.invoke("restore")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("captive_portal_mode", self.network["settings"])
        self.assertEqual(self.network["settings"]["mobile_data"], "1")
        self.assertEqual(self.network["settings"]["wifi_on"], "0")
        self.assertFalse(self.state.exists())

    def test_wifi_only_network_restores_explicit_setting(self) -> None:
        self.network["settings"].update(
            captive_portal_mode="2", mobile_data="0", wifi_on="1"
        )
        self.network["connections"] = {"data": False, "wifi": True}
        self.assertEqual(self.invoke("prepare").returncode, 0)
        self.assertEqual(self.invoke("restore").returncode, 0)
        self.assertEqual(self.network["settings"], {
            "captive_portal_mode": "2", "mobile_data": "0", "wifi_on": "1",
        })
        self.assertNotIn(["svc", "data", "enable"], self.network["calls"])

    def test_disconnect_timeout_reenables_network_and_preserves_backup(self) -> None:
        self.network["disconnect_stuck"] = True
        result = self.invoke("prepare")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("下载测试网络在 15 秒内未断开", result.stderr)
        self.assertTrue(self.state.exists())
        self.assertEqual(self.network["settings"]["mobile_data"], "1")
        self.assertTrue(self.network["connections"]["data"])
        self.network["disconnect_stuck"] = False
        self.assertEqual(self.invoke("restore").returncode, 0)
        self.assertNotIn("captive_portal_mode", self.network["settings"])
        self.assertFalse(self.state.exists())

    def test_failed_network_query_does_not_confirm_disconnect(self) -> None:
        self.network["dumpsys_fails"] = True
        result = self.invoke("prepare")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("下载测试网络在 15 秒内未断开", result.stderr)
        self.assertEqual(self.network["settings"]["mobile_data"], "1")
        self.assertTrue(self.state.exists())
        self.network["dumpsys_fails"] = False
        self.assertEqual(self.invoke("restore").returncode, 0)

    def test_unvalidated_network_fails_and_remains_restorable(self) -> None:
        self.network["connect"] = False
        result = self.invoke("prepare")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("did not become VALIDATED", result.stderr)
        self.assertTrue(self.state.exists())
        self.assertEqual(self.invoke("restore").returncode, 0)
        self.assertNotIn("captive_portal_mode", self.network["settings"])

    def test_verified_default_network_needs_no_changes(self) -> None:
        for sdk in ("35", "36", "37"):
            with self.subTest(sdk=sdk):
                self.network.update(sdk=sdk, validated=True, calls=[])
                self.assertEqual(self.invoke("prepare").returncode, 0)
                self.assertFalse(self.state.exists())
                self.assertFalse(any(c[0] in ("settings", "svc") for c in self.network["calls"]))

    def test_validated_secondary_network_does_not_mask_default(self) -> None:
        for sdk in ("35", "36", "37"):
            with self.subTest(sdk=sdk):
                self.network.update(sdk=sdk, other_validated=True, validated=False)
                self.assertEqual(self.invoke("prepare").returncode, 0)
                self.assertTrue(self.state.exists())
                self.assertEqual(self.invoke("restore").returncode, 0)

    def test_existing_backup_is_not_overwritten(self) -> None:
        self.assertEqual(self.invoke("prepare").returncode, 0)
        original = self.state.read_bytes()
        result = self.invoke("prepare")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.state.read_bytes(), original)
        self.assertEqual(self.invoke("restore").returncode, 0)

    def test_physical_devices_and_older_emulators_are_untouched(self) -> None:
        for qemu, sdk in (("0", "37"), ("1", "34")):
            with self.subTest(qemu=qemu, sdk=sdk):
                self.network.update(qemu=qemu, sdk=sdk, calls=[])
                self.assertEqual(self.invoke("prepare").returncode, 0)
                self.assertFalse(self.state.exists())
                self.assertFalse(any(call[0] in ("settings", "svc")
                                     for call in self.network["calls"]))
                self.assertEqual(self.invoke("restore").returncode, 0)


if __name__ == "__main__":
    unittest.main()
