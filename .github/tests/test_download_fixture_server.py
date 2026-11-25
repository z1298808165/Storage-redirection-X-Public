from contextlib import ExitStack
from http.client import HTTPConnection
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


SERVER = Path(__file__).with_name("download-fixture-server.py").resolve()


class DownloadFixtureServerTest(unittest.TestCase):
    def test_concurrent_runs_serve_their_own_bytes(self):
        with ExitStack() as resources:
            servers = []
            for payload in (b"first\x00\xff", b"other\xff\x00"):
                directory = Path(resources.enter_context(tempfile.TemporaryDirectory()))
                (directory / "srt_download.bin").write_bytes(payload)
                port_file = directory / "port"
                with port_file.open("wb") as output:
                    process = subprocess.Popen(
                        [sys.executable, str(SERVER)], cwd=directory,
                        stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.DEVNULL,
                    )
                resources.callback(process.wait, 5)
                resources.callback(process.kill)
                deadline = time.monotonic() + 10
                while not port_file.read_bytes().endswith(b"\n"):
                    self.assertIsNone(process.poll(), "server exited before binding")
                    self.assertLess(time.monotonic(), deadline, "server startup timed out")
                    time.sleep(0.02)
                servers.append((int(port_file.read_bytes()), payload))

            self.assertNotEqual(servers[0][0], servers[1][0])
            for port, payload in servers:
                connection = HTTPConnection("127.0.0.1", port, timeout=5)
                try:
                    connection.request("GET", "/srt_download.bin")
                    response = connection.getresponse()
                    self.assertEqual(response.status, 200)
                    self.assertEqual(response.read(), payload)
                finally:
                    connection.close()
