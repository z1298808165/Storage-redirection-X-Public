"""核对测试应用发起的集合请求与同一媒体提供器线程的直通记录。"""

import re
import sys
from pathlib import Path


def delegated(result, log, provider):
    callers = re.findall(r"^PASS \[mediastore_open_typed_collection\][^\n]* caller_uid=(\d+) caller_pid=(\d+)\b", result, re.M)
    if len(callers) != 1:
        return False
    uid, pid = callers[0]
    request = re.compile(r"^java open target=\S+#openTypedAssetFile\([^)]*\) "
                         rf"caller_uid={uid} caller_pid={pid} redirect_enabled=true "
                         r"args=\[uri=content://media/external/file,")
    marker = "java open delegate reason=collection_uri_passthrough from=content://media/external/file "
    pending = set()
    for thread, message in re.findall(rf"^\S+\s+\S+\s+{int(provider)}\s+(\d+)\s+[IW]\s+SRX\s*: (.*)$", log, re.M):
        if message.startswith("java open target="):
            pending.discard(thread)
            if request.match(message):
                pending.add(thread)
        elif message.startswith(("java open delegate ", "java open result ", "java open backup failure ")):
            if thread in pending and message.startswith(marker):
                return True
            pending.discard(thread)
    return False


if __name__ == "__main__":
    result_path, log_path, provider_pid = sys.argv[1:]
    sys.exit(0 if delegated(Path(result_path).read_text(encoding="utf-8-sig"),
                            Path(log_path).read_text(encoding="utf-8-sig"), provider_pid)
             else "Missing collection delegation for the fixture caller on the serving provider thread")
