import os
import tempfile
from pathlib import Path
import runpy
import shutil
import subprocess
import unittest


HELPER = Path(__file__).with_name("media-provider-pid.sh")


@unittest.skipUnless(shutil.which("sh"), "需要设备 shell 语法")
class TestAppProcessSelection(unittest.TestCase):
    def test_only_one_exact_user_zero_process_is_selectable(self):
        selector = HELPER.with_name("test-app-pid.sh").read_text()
        command = 'package=test.app\nps() { cat; }\n' + selector
        owner = "10192 92 test.app"
        other = "1010192 410 test.app"
        remote = "10192 365 test.app:worker"
        for rows, expected in (([owner, other], "92"), ([other, owner], "92"), ([remote, owner], "92"),
                               ([other, remote], ""), ([owner, "10192 350 test.app"], ""), ([], "")):
            with self.subTest(rows=rows):
                run = subprocess.run(["sh", "-c", command], input="\n".join(rows), text=True,
                                     capture_output=True, check=True)
                self.assertEqual(expected, run.stdout.strip())


class MountConfirmationTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("sh") and shutil.which("bash") and shutil.which("pwsh"), "需要三个 shell 验证实际挂载确认命令")
    def test_only_the_selected_pid_can_confirm_a_mount(self):
        tests = HELPER.parent
        selector = HELPER.with_name("test-app-pid.sh").read_text()
        with tempfile.TemporaryDirectory() as directory:
            probe = Path(directory) / "probe"
            logfile = Path(directory) / "log"
            logfile.touch()
            env = dict(os.environ, SELECTOR=selector, PROBE=str(probe), LOGFILE=str(logfile))
            for host, suffix, first, last, driver in (
                ("bash", "sh", "wait_app_mount_confirmed() {", "\nscenario_from_label() {", '''
APP_ID=test.app LOG_PATH="$LOGFILE" SRT_TEST_APP_PID_COMMAND="$SELECTOR" SRT_MOUNT_CONFIRM_TIMEOUT_MS=1000
adb_su_timeout() { printf '%s' "$2" > "$PROBE"; return 1; }
wait_app_mount_confirmed scenario-2 || :
'''),
                ("pwsh", "ps1", "function Wait-AppMountConfirmed {", "\nfunction Get-ScenarioFromLabel {", '''
$AppId="test.app"; $LogPath=$env:LOGFILE; $script:TestAppPidCommand=$env:SELECTOR
$script:MountConfirmTimeoutMilliseconds=1000
function Invoke-Su($cmd) { [IO.File]::WriteAllText($env:PROBE,$cmd); $global:LASTEXITCODE=1 }
Wait-AppMountConfirmed scenario-2 | Out-Null
'''),
            ):
                source = (tests / ("run-storage-redirect-scenarios." + suffix)).read_text(encoding="utf-8-sig")
                start = source.index(first)
                body = source[start:source.index(last, start)]
                probe.unlink(missing_ok=True)
                args = [host, "-c"] if host == "bash" else [host, "-NoProfile", "-Command"]
                subprocess.run([*args, body + driver], env=env, check=True, capture_output=True, timeout=12)
                stubs = """ps() { printf '%s\\n' '1010192 659 test.app' '10192 123 test.app' '10192 981 test.app:worker'; }
logcat() { echo "$LINE"; }
"""
                for fmt in ("app mount confirmed pid={} mounts=1 observed_ms=3 pkg=test.app",
                            "daemon mount pkg=test.app pid={} op=Reload ok=true",
                            "perf companion mount pkg=test.app pid={} uid=10192 ok=true"):
                    for pid in (123, 1234, 659):
                        with self.subTest(host=host, fmt=fmt, pid=pid):
                            run = subprocess.run(["sh", "-c", stubs + probe.read_text()], env=dict(env, LINE=fmt.format(pid)),
                                                 capture_output=True, text=True, timeout=6)
                            self.assertEqual(0 if pid == 123 else 1, run.returncode, run.stderr)
                            self.assertEqual("confirmed_pid=123" if pid == 123 else "pid=123", run.stdout.strip())


class CollectionDelegationTest(unittest.TestCase):
    def test_requires_serving_process_matching_caller_thread_and_request_order(self):
        delegated = runpy.run_path(str(HELPER.with_name("verify-collection-delegation.py")))["delegated"]
        result = "PASS [mediastore_open_typed_collection] delegated caller_uid=10192 caller_pid=3201"
        prefix = "11-19 12:01:00.000  9049  108 I SRX     : "
        request = prefix + "java open target=MediaProvider#openTypedAssetFile(Uri) caller_uid=10192 caller_pid=3201 redirect_enabled=true args=[uri=content://media/external/file, ...]"
        marker = prefix + "java open delegate reason=collection_uri_passthrough from=content://media/external/file to= n=3"
        self.assertTrue(delegated(result, request + "\n" + marker, 9049))
        for boundary in (
            marker.replace("collection_uri_passthrough", "mapped_query_miss"),
            prefix + "java open result source=backup class=android.content.res.AssetFileDescriptor",
            prefix.replace(" I SRX", " W SRX") + "java open backup failure caller_uid=10192",
        ):
            with self.subTest(boundary=boundary):
                self.assertFalse(delegated(result, "\n".join((request, boundary, marker)), 9049))
                self.assertTrue(delegated(result, "\n".join((request, boundary.replace(" 108 ", " 109 "), marker)), 9049))
        for lines in ((marker,), (request,), (marker, request), (request, marker.replace("108 I", "109 I")),
                      (request.replace("3201", "3202"), marker), (request.replace("10192", "1010192"), marker),
                      (request.replace("true", "false"), marker), (request, request.replace("3201", "3202"), marker),
                      (request, marker.replace("9049", "7800")), (request, marker.replace("/file to=", "/file/1 to="))):
            with self.subTest(lines=lines):
                self.assertFalse(delegated(result, "\n".join(lines), 9049))
        self.assertFalse(delegated(result, request + "\n" + marker, 7800))
        self.assertFalse(delegated("", request + "\n" + marker, 9049))


def provider(user, pid, authority="media"):
    proc = f"ProcessRecord{{a1 {pid}:org.example.provider/u{user}a179}}" if pid else "null"
    return (f"  * ContentProviderRecord{{a2 u{user} org.example.provider/.Provider}}\n"
            f"    proc={proc}\n    uid={user * 100000 + 10179} provider=proxy\n"
            f"    authority={authority}\n")


@unittest.skipUnless(shutil.which("sh") and shutil.which("awk"), "sh and awk required")
class MediaProviderPidTest(unittest.TestCase):
    def select(self, records):
        command = 'dumpsys() { test "$*" = "activity providers" && cat; }; . "$1"'
        return subprocess.run(["sh", "-c", command, "sh", str(HELPER)], input=records,
                              text=True, capture_output=True, check=True).stdout

    def test_published_owner_independent_of_process_order_and_name(self):
        owner = provider(0, 9049, "downloads;media")
        other = provider(10, 7800) + provider(0, 590, "media.documents")
        for records in (owner + other, other + owner):
            with self.subTest(records=records):
                self.assertEqual(self.select(records), "9049\n")

    def test_missing_stopped_malformed_or_ambiguous_owner(self):
        for records in ("", provider(10, 7800), provider(0, None), provider(0, "invalid"),
                        provider(0, 590, "media.documents") + provider(0, None),
                        provider(0, 9049) + provider(0, 9050)):
            with self.subTest(records=records):
                self.assertEqual(self.select(records), "")
