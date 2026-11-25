"""在隔离模拟器上构造系统视图覆盖；观察普通 daemon 恢复，不发送重挂或重启命令。"""

import argparse
import base64
import json
import re
import shlex
import subprocess
import time
import uuid
from pathlib import Path

APP = "me.fakerqu.test.storageredirect"
MODULE = "/data/adb/modules/storage.redirect.x"
CONFIG = f"{MODULE}/config/apps/{APP}.json"
GLOBAL = f"{MODULE}/config/global.json"
LOCK = "/data/local/tmp/srx-test-flow.lock"


def fields(text: str) -> dict[str, str]:
    return dict(line.split("=", 1) for line in text.splitlines() if "=" in line)


def mount_layers(text: str):
    for line in text.splitlines():
        before, separator, after = line.partition(" - ")
        left, right = before.split(), after.split()
        if separator and len(left) >= 5 and len(right) >= 2:
            yield dict(id=int(left[0]), parent=int(left[1]), dev=left[2], root=left[3],
                       target=left[4], fs=right[0], source=right[1])


def mounts(text: str, target: str) -> list[dict]:
    return [layer for layer in mount_layers(text) if layer["target"] == target]


def top(text: str, target: str) -> dict | None:
    layers = mounts(text, target)
    covered = {layer["parent"] for layer in layers}
    return next((layer for layer in reversed(layers) if layer["id"] not in covered), None)


def stable(before: dict, after: dict) -> bool:
    host_before, host_after = fields(before["host"]), fields(after["host"])
    return all(before[key] == after[key] for key in ("pid", "start", "namespace", "uid", "config", "global")) and all(host_before.get(key) == host_after.get(key) for key in ("pid", "start_ticks", "dev"))


def validate_isolation(before: dict, after: dict) -> None:
    if not view_restored(before, "/storage/emulated", fields(before["host"])["dev"]):
        raise AssertionError("隔离对照用户启动时缺少健康的 SRX 视图")
    if not stable(before, after) or any(before[key] != after[key] for key in ("mountinfo", "ledger")):
        raise AssertionError("另一用户的进程、存储视图或账本发生变化")


def view_restored(snapshot: dict, target: str, dev: str) -> bool:
    layers = mounts(snapshot["mountinfo"], target)
    if len(layers) != 1 or layers[0]["dev"] != dev:
        return False
    receipt = f"mount={layers[0]['id']}\t{dev}\t/dev/fuse\t{target}"
    return receipt in snapshot.get("ledger", "").splitlines()


def validate_app_result(raw: str, case: str, *, stat_denied: bool = False) -> None:
    lines = raw.splitlines()
    messages = [line for line in lines if not line.startswith("__SRT_RESULT_")]
    prefix = "FAIL [file_stat] stat failed: EACCES (Permission denied)" if stat_denied else f"PASS [{case}] "
    if len(messages) != 1 or not messages[0].startswith(prefix):
        raise AssertionError(f"应用结果并非单次成功操作：{raw!r}")
    if len(lines) != 3 or lines.count("__SRT_RESULT_END__=1") != 1 or len([line for line in lines if re.fullmatch(r"__SRT_RESULT_TOKEN__=[0-9a-f]{32}", line)]) != 1:
        raise AssertionError("应用结果不完整或含多条回执")


def storage_parents(user: int) -> list[str]:
    return ["/storage/emulated"] + [f"/mnt/{view}/{user}/emulated" for view in ("user", "installer", "androidwritable")]


def visible_layer(snapshot: dict, target: str) -> dict | None:
    identifier = snapshot["visible"].get(target)
    return next((layer for layer in mount_layers(snapshot["mountinfo"])
                 if str(layer["id"]) == str(identifier)), None)


def validate_namespace(before: dict, fault: dict, after: dict, user: int, mode: str) -> None:
    if not stable(before, fault) or not stable(before, after) or before["ledger"] != fault["ledger"]:
        raise AssertionError("命名空间恢复的进程身份或原始账本已改变")
    anchor = top(fault["init_mountinfo"], f"/mnt/user/{user}/emulated")
    if not anchor or (anchor["fs"], anchor["source"], anchor["root"]) != ("fuse", "/dev/fuse", "/"):
        raise AssertionError("缺少可信的系统挂载基准")
    for parent in storage_parents(user):
        target = f"{parent}/{user}"
        original = visible_layer(before, target)
        final = visible_layer(after, target)
        if not original or original["target"] != target or original["dev"] == anchor["dev"] or not original["root"].endswith(f"/{user}/Android/data/{APP}/sdcard"):
            raise AssertionError("缺少命名空间绑定对照：" + target)
        host_dev = fields(before["host"])["dev"]
        for snapshot, layer in ((before, original), (after, final)):
            if mode == "foreign" and snapshot is after:
                continue
            bind = layer and layer["target"] == target and (layer["dev"], layer["root"]) == (original["dev"], original["root"])
            host = layer and (layer["target"], layer["dev"], layer["root"], layer["fs"], layer["source"]) == (parent, host_dev, "/", "fuse", "/dev/fuse")
            if not layer or not (bind or host) or f"mount={layer['id']}\t{layer['dev']}\t{layer['source']}\t{layer['target']}" not in snapshot["ledger"].splitlines():
                raise AssertionError("命名空间中的可达挂载与账本不一致：" + target)
        changed = visible_layer(fault, parent if mode == "foreign" else target)
        if mode == "foreign":
            if not changed or (changed["fs"], changed["source"]) != ("tmpfs", "issue18-foreign") or visible_layer(after, parent) != changed:
                raise AssertionError("未知父挂载已消失或被修改")
        elif not changed or changed["dev"] != anchor["dev"] or changed["target"] != parent:
            raise AssertionError("实际可达的父挂载不是系统视图")
        if mode == "cover" and original["id"] not in {layer["id"] for layer in mounts(fault["mountinfo"], target)}:
            raise AssertionError("被覆盖的子挂载记录已从 mountinfo 消失")
        if mode == "missing" and original["id"] in {layer["id"] for layer in mounts(fault["mountinfo"], target)}:
            raise AssertionError("应已移除的子挂载仍然存在")
    if mode == "foreign" and (after["mountinfo"] != fault["mountinfo"] or after["ledger"] != fault["ledger"]):
        raise AssertionError("观察未知父挂载期间命名空间发生变化")


def validate_io(record: dict, metadata: str | None = None) -> None:
    validate_app_result(record["raw"], "file_write")
    if record["private"] != record["payload"] or record["real"] != "__absent__" or not record["metadata"]:
        raise AssertionError("应用结果与实际文件落点不一致")
    if metadata is not None and record["metadata"] != metadata:
        raise AssertionError("恢复后文件元数据与健康对照不同")


def validate_unredirected_io(record: dict) -> None:
    validate_app_result(record["raw"], "file_write")
    if record["real"] != record["payload"] or record["private"] != "__absent__":
        raise AssertionError("关闭重定向后的写入未仅落到真实存储")


def validate_system_first_attachment(before: dict, after: dict, user: int) -> None:
    anchor = top(before["init_mountinfo"], f"/mnt/user/{user}/emulated")
    system = top(before["mountinfo"], "/storage/emulated")
    host_before = fields(before["host"])
    host_after = fields(after["host"])
    if not anchor or not system or (anchor["fs"], anchor["source"], anchor["root"]) != ("fuse", "/dev/fuse", "/"):
        raise AssertionError("系统先就绪的对照缺少初始系统存储视图")
    if (system["dev"], system["fs"], system["source"], system["root"]) != (anchor["dev"], "fuse", "/dev/fuse", "/") or system["dev"] == host_before.get("dev"):
        raise AssertionError("接入 SRX 前应用视图不是系统 FUSE")
    previous_policy = json.loads(before["config"])
    current_policy = json.loads(after["config"])
    if previous_policy["users"][str(user)]["enabled"] is not False or current_policy["users"][str(user)]["enabled"] is not True:
        raise AssertionError("延迟接入要求策略从停用切换到启用")
    previous_policy["users"][str(user)]["enabled"] = True
    if previous_policy != current_policy or any(
        before[key] != after[key] for key in ("pid", "start", "namespace", "uid", "global")
    ):
        raise AssertionError("切换策略后未在同一应用进程完成 SRX 接入")
    if not host_before.get("dev") or host_before.get("dev") == anchor["dev"] or any(
        host_before.get(key) != host_after.get(key) for key in ("pid", "start_ticks", "dev")
    ):
        raise AssertionError("延迟接入期间宿主发生变化或与系统 FUSE 相同")
    targets = [
        target for target in storage_parents(user)
        if (layer := top(before["mountinfo"], target)) and layer["dev"] == anchor["dev"]
    ]
    if not targets or any(not view_restored(after, target, host_before["dev"]) for target in targets):
        raise AssertionError("仍有继承的系统存储别名未被 SRX 替换")


def wait_for_system_first_attachment(device, before: dict, user: int, name: str, timeout: int = 90) -> tuple[dict, int]:
    deadline = time.monotonic() + timeout
    observations = 0
    while True:
        after = device.snapshot(before["pid"], f"{name}-observed-{observations}")
        observations += 1
        if any(before[key] != after[key] for key in ("pid", "start", "namespace")):
            raise AssertionError("延迟接入期间应用进程或命名空间发生变化")
        try:
            validate_system_first_attachment(before, after, user)
            return after, observations
        except AssertionError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(1)


def validate(before: dict, fault: dict, after: dict, targets: list[str], foreign: bool, missing: bool = False) -> None:
    if not targets:
        raise AssertionError("未验证任何存储入口")
    if not stable(before, fault) or not stable(before, after):
        raise AssertionError("观察期间进程、命名空间、策略或宿主发生变化")
    dev = fields(before["host"])["dev"]
    user = int(before["uid"]) // 100000
    system = top(fault["init_mountinfo"], f"/mnt/user/{user}/emulated")
    if not system or system["dev"] == dev or system["fs"] != "fuse" or system["source"] != "/dev/fuse" or system["root"] != "/":
        raise AssertionError("缺少可信的系统存储挂载基准")
    if fault.get("ledger") != before.get("ledger"):
        raise AssertionError("记录故障前账本已经改变")
    for target in storage_parents(user):
        if target in targets or not top(before["mountinfo"], target):
            continue
        if (not view_restored(before, target, dev) or mounts(before["mountinfo"], target) != mounts(fault["mountinfo"], target)
                or not view_restored(after, target, dev)):
            raise AssertionError(f"对照别名未保持健康视图：{target}")
    for target in targets:
        original, changed, final = (top(s["mountinfo"], target) for s in (before, fault, after))
        if not original or original["dev"] != dev or (not changed and not missing) or changed and changed["dev"] == dev:
            raise AssertionError(f"未构造预期故障：{target}")
        if foreign:
            if not changed or changed["fs"] != "tmpfs" or changed["source"] != "issue18-foreign":
                raise AssertionError(f"缺少未知接管者的挂载身份：{target}")
        elif changed and (changed["dev"], changed["fs"], changed["source"], changed["root"]) != (system["dev"], "fuse", "/dev/fuse", "/"):
            raise AssertionError(f"故障入口不是系统存储视图：{target}")
        if foreign:
            if final != changed:
                raise AssertionError(f"未知挂载已被修改：{target}")
        elif not final or final["dev"] != dev or len(mounts(after["mountinfo"], target)) != 1:
            raise AssertionError(f"恢复后不是单层 SRX 视图：{target}")
        elif "ledger" in after and f"mount={final['id']}\t{dev}\t/dev/fuse\t{target}" not in after["ledger"].splitlines():
            raise AssertionError(f"恢复账本与实际挂载不一致：{target}")


class Device:
    def __init__(self, serial: str, output: Path):
        self.serial, self.output = serial, output
        self.remote = "/data/local/tmp/srx-issue18-" + uuid.uuid4().hex
        self.owned_files = []
        self._root_shell = None

    def adb(self, *args: str, timeout: int = 45) -> str:
        run = subprocess.run(["adb", "-s", self.serial, *args], capture_output=True, text=True, encoding="utf8", errors="replace", timeout=timeout)
        if run.returncode:
            raise RuntimeError(run.stdout + run.stderr)
        return run.stdout.replace("\r", "")

    def su(self, command: str, timeout: int = 45) -> str:
        if self._root_shell is None:
            self._root_shell = self.adb("shell", "id -u").strip() == "0"
            if not self._root_shell and self.adb("shell", "su 0 id -u").strip() != "0":
                raise RuntimeError("需要 root ADB 或 KernelSU su 0")
        if self._root_shell:
            return self.adb("shell", command, timeout=timeout)
        return self.adb("shell", "su 0 sh -c " + shlex.quote(command), timeout=timeout)

    def read(self, path: str) -> str:
        return self.su(f"if test -f {shlex.quote(path)}; then cat {shlex.quote(path)}; else printf __absent__; fi")

    def write(self, path: str, content: str) -> None:
        if content == "__absent__":
            self.su("rm -f " + shlex.quote(path))
            return
        encoded = base64.b64encode(content.encode()).decode()
        self.su(f"printf %s {encoded} | base64 -d > {shlex.quote(path)}.issue18.tmp && chmod 644 {shlex.quote(path)}.issue18.tmp && mv {shlex.quote(path)}.issue18.tmp {shlex.quote(path)}")

    def snapshot(self, pid: str, name: str, *, user: int | None = None, inject: tuple[str, ...] = ()) -> dict:
        ledger_path = f"{MODULE}/tmp/mount_state/identity/{APP}_{pid}.identity"
        # 故障注入与取证共用一次设备调用，避免宿主往返期间 daemon 已完成恢复。
        batch = [*inject, "printf '\\000'", f"cat /proc/{pid}/mountinfo", "printf '\\000'",
                 f"if test -f {ledger_path}; then cat {ledger_path}; else printf __absent__; fi"]
        if user is not None:
            targets = [path for parent in storage_parents(user) for path in (parent, f"{parent}/{user}")]
            # 用 shell 内建读取自身 fdinfo，避免 exec 外部命令后关闭探测描述符。
            batch.extend(["printf '\\000'", f'''for target in {shlex.join(targets)}; do
id=$( (exec 9<"/proc/{pid}/root$target" && while read -r key value; do
if [ "$key" = mnt_id: ]; then printf '%s\\n' "$value"; fi
done </proc/self/fdinfo/9) 2>/dev/null) || id=missing
printf '%s=%s\\n' "$target" "$id"
done'''])
        receipts, mountinfo, ledger, *visible = self.su(" && ".join(batch)).split("\0")
        if inject:
            self.output.joinpath(name + "-injection.txt").write_text(receipts, encoding="utf8")
        stat = self.su(f"cat /proc/{pid}/stat")
        status = self.su(f"cat /proc/{pid}/status")
        result = dict(
            pid=pid, start=stat[stat.rindex(")") + 2 :].split()[19],
            namespace=self.su(f"readlink /proc/{pid}/ns/mnt").strip(),
            uid=re.search(r"^Uid:\s+(\d+)", status, re.M)[1],
            mountinfo=mountinfo,
            init_mountinfo=self.su("cat /proc/1/mountinfo"),
            config=self.read(CONFIG), global_=self.read(GLOBAL),
            host=self.read(f"{MODULE}/tmp/fuse_host.snapshot"),
            ledger=ledger,
            monotonic=time.monotonic(),
        )
        result["global"] = result.pop("global_")
        if user is not None:
            raw, = visible
            self.output.joinpath(name + "-visible.txt").write_text(raw, encoding="utf8")
            result["visible"] = {path: int(value) if value.isdigit() else None for path, value in fields(raw).items()}
        self.output.joinpath(name + ".json").write_text(json.dumps(result, indent=2), encoding="utf8")
        return result

    def fault_command(self, before: dict, user: int, target: str, mode: str, dev: str) -> str:
        return shlex.join([self.remote + "/fixture", before["pid"], before["uid"], str(user), target, mode, dev])

    def app_case(self, user: int, name: str, case: str, timeout: int = 45, **extras: str) -> str:
        token = uuid.uuid4().hex
        args = ["am", "start-foreground-service", "--user", str(user), "-n", f"{APP}/.TestService", "-a", "me.fakerqu.test.storageredirection.TEST_CASE"]
        for key, value in dict(test_case=case, result_token=token, **extras).items():
            args += ["--es", key, value]
        launch = self.adb("shell", shlex.join(args))
        self.output.joinpath(name + "-launch.txt").write_text(launch, encoding="utf8")
        result = f"/data/user/{user}/{APP}/files/test_case_result/result_current.txt"
        deadline = time.monotonic() + timeout
        text = ""
        while time.monotonic() < deadline:
            text = self.read(result)
            if f"__SRT_RESULT_TOKEN__={token}\n" in text:
                break
            time.sleep(0.2)
        else:
            self.output.joinpath(name + "-result.txt").write_text(text, encoding="utf8")
            raise AssertionError(f"测试应用回执缺失或过期：{text!r}")
        self.output.joinpath(name + "-result.txt").write_text(text, encoding="utf8")
        return text

    def app_write(self, user: int, name: str, payload: str, path_name: str | None = None, *, parent: str = "/storage/emulated") -> dict:
        file_name = path_name or name
        path = f"{parent}/{user}/Download/issue18-{file_name}.txt"
        private = f"/data/media/{user}/Android/data/{APP}/sdcard/Download/issue18-{file_name}.txt"
        real = f"/data/media/{user}/Download/issue18-{file_name}.txt"
        self.owned_files.extend((private, real))
        text = self.app_case(user, name, "file_write", file_path=path, payload=payload, expected_payload=payload)
        metadata = self.su(f"if test -f {shlex.quote(private)}; then stat -c '%u:%g:%a' {shlex.quote(private)}; fi").strip()
        record = dict(raw=text, payload=payload, private=self.read(private), real=self.read(real), metadata=metadata)
        self.output.joinpath(name + "-io.json").write_text(json.dumps(record, indent=2), encoding="utf8")
        return record

    def check_alias_io(self, user: int, name: str, parent: str) -> bool:
        raw = self.app_case(user, name + "-stat", "file_stat", file_path=f"{parent}/{user}/Download")
        denied = raw.startswith("FAIL [file_stat]")
        validate_app_result(raw, "file_stat", stat_denied=denied)
        if not denied:
            validate_io(self.app_write(user, name, "issue18-alias", parent=parent))
        return not denied

    def prepare_user(self, user: int) -> None:
        self.adb("shell", f"am start-user {user}")
        deadline = time.monotonic() + 900
        probe = (
            f"am get-started-user-state {user} | grep -qx RUNNING_UNLOCKED && "
            f"sm list-volumes all | grep -q '^emulated;{user} mounted ' && "
            f"timeout 3 stat /proc/1/root/mnt/user/{user}/emulated/{user} >/dev/null 2>&1 "
            "&& echo ready || echo pending"
        )
        while self.su(probe).strip() != "ready":
            if time.monotonic() >= deadline:
                raise AssertionError(f"启动应用前用户 {user} 的存储未就绪")
            time.sleep(2)

    def start(self, user: int, namespace_bind: bool = False, system_only: bool = False) -> str:
        if namespace_bind and system_only:
            raise ValueError("仅检查系统视图时不能使用 namespace-bind 就绪判据")
        self.adb("shell", f"am force-stop --user {user} {APP}")
        # 后台用户的 Activity 未必启动进程；服务由 shell 显式指定用户发起。
        # 先用内部目录启动服务；存储故障注入必须从就绪视图开始，不能拿接入中的 I/O 当基线。
        setup_name = f"setup-{user}-{uuid.uuid4().hex[:8]}"
        deadline = time.monotonic() + 360
        raw = self.app_case(user, setup_name + "-launch", "file_stat", timeout=360, file_path=f"/data/user/{user}/{APP}")
        validate_app_result(raw, "file_stat")
        identity = None
        observations = 0
        while time.monotonic() < deadline:
            for pid in self.adb("shell", f"pidof {APP} || true").split():
                status = self.su(f"cat /proc/{pid}/status")
                uid = int(re.search(r"^Uid:\s+(\d+)", status, re.M)[1])
                if uid // 100000 == user:
                    state = self.snapshot(pid, f"{setup_name}-observed-{observations}")
                    observations += 1
                    current_identity = tuple(state[key] for key in ("pid", "start", "namespace"))
                    if identity is not None and current_identity != identity:
                        raise AssertionError("启动期间应用进程或命名空间发生变化")
                    identity = current_identity
                    host = fields(state["host"])
                    current = top(state["mountinfo"], f"/storage/emulated/{user}" if namespace_bind else "/storage/emulated")
                    if system_only:
                        system = top(state["init_mountinfo"], f"/mnt/user/{user}/emulated")
                        ready = current and system and current["dev"] == system["dev"] and current["root"] == "/"
                    else:
                        ready = current and (current["root"].endswith(f"/{user}/Android/data/{APP}/sdcard") if namespace_bind else current["dev"] == host.get("dev"))
                    if (system_only or state["ledger"] != "__absent__") and ready:
                        # file_write 不创建父目录；必须在最终可见视图中由应用自身创建。
                        raw = self.app_case(user, setup_name + "-mkdir", "file_mkdir", file_dir=f"/storage/emulated/{user}/Download")
                        validate_app_result(raw, "file_mkdir")
                        record = self.app_write(user, setup_name + "-io", "issue18-setup")
                        (validate_unredirected_io if system_only else validate_io)(record)
                        return pid
            time.sleep(1)
        raise AssertionError("SRX 视图和账本未就绪")

    def cleanup(self, user: int, original: dict[str, str]) -> None:
        actions = [("stop", lambda: self.adb("shell", f"am force-stop --user {user} {APP}"))]
        actions += [(path, lambda path=path, content=content: self.write(path, content)) for path, content in original.items()]
        if self.owned_files:
            actions.append(("files", lambda: self.su("rm -f " + shlex.join(self.owned_files))))
        actions.append(("fixture", lambda: self.su("rm -rf " + self.remote)))
        actions.append(("lock", lambda: self.su(f'test "$(cat {LOCK}/owner)" = {self.remote.rsplit("/", 1)[1]} && rm {LOCK}/owner && rmdir {LOCK}')))
        failures = []
        for name, action in actions:
            try:
                action()
            except Exception as error:
                failures.append(f"{name}: {error}")
        if failures:
            raise RuntimeError("清理未完成：" + "; ".join(failures))


def run_namespace_matrix(device: Device, user: int, timeout: int, reports: list[dict]) -> None:
    device.write(CONFIG, json.dumps({"users": {str(user): {"enabled": True}}}))
    for index, mode in enumerate(("cover", "missing", "cover", "foreign")):
        name = f"namespace-{index}-{mode}"
        try:
            # 恢复可改用共享宿主；每个 bind 故障都需重新建立可核对的子挂载基线。
            pid = device.start(user, namespace_bind=True)
            before = device.snapshot(pid, name + "-before", user=user)
            control = device.app_write(user, name + "-control", "issue18-control")
            validate_io(control)
            commands = []
            for parent in storage_parents(user):
                target = f"{parent}/{user}" if mode == "missing" else parent
                layer = visible_layer(before, target)
                if not layer:
                    raise AssertionError("缺少故障前基线：" + target)
                commands.append(device.fault_command(before, user, target, mode, layer["dev"]))
            fault = device.snapshot(pid, name + "-fault", user=user, inject=tuple(commands))
            deadline = time.monotonic() + timeout
            observations = 0
            while True:
                after = device.snapshot(pid, f"{name}-observed-{observations}", user=user)
                observations += 1
                try:
                    validate_namespace(before, fault, after, user, mode)
                    restored = True
                except AssertionError:
                    restored = False
                if not stable(before, after):
                    raise AssertionError("观察期间进程、策略或宿主发生变化")
                if mode == "foreign" and (after["mountinfo"] != fault["mountinfo"] or after["ledger"] != fault["ledger"]):
                    raise AssertionError("观察期间未知命名空间发生变化")
                if restored and mode != "foreign" or time.monotonic() >= deadline:
                    break
                if observations % 6 == 1:
                    print(f"WAIT {name}: 等待守护进程的周期检查", flush=True)
                time.sleep(10)
            validate_namespace(before, fault, after, user, mode)
            if mode != "foreign":
                validate_io(device.app_write(user, name + "-recovered", "issue18-recovered"))
                path = f"/storage/emulated/{user}/Download/issue18-{name}-control.txt"
                validate_app_result(device.app_case(user, name + "-existing-read", "file_read", file_path=path, expected_payload=control["payload"]), "file_read")
                # 新文件的创建后端可变；既有文件跨恢复读写后必须保留原始元数据。
                validate_io(device.app_write(user, name + "-existing-write", "issue18-existing", path_name=name + "-control"), control["metadata"])
            reports.append(dict(case=name, passed=True, mode=mode, observations=observations))
            print("PASS " + name, flush=True)
        except Exception as error:
            reports.append(dict(case=name, passed=False, error=str(error)))
            print(f"FAIL {name}: {error}", flush=True)
            try:
                device.adb("shell", f"am force-stop --user {user} {APP}")
            except Exception as cleanup_error:
                reports.append(dict(case=name + "-cleanup", passed=False, error=str(cleanup_error)))


def run_system_first_attachment(
    device: Device, user: int, enabled_config: dict, delay_seconds: int, reports: list[dict]
) -> None:
    name = f"system-first-delay-{delay_seconds}s"
    config = json.loads(json.dumps(enabled_config))
    config["users"][str(user)]["enabled"] = False
    try:
        device.write(CONFIG, json.dumps(config))
        before_pid = device.start(user, system_only=True)
        before = device.snapshot(before_pid, name + "-system-only")
        if delay_seconds:
            time.sleep(delay_seconds)
        config["users"][str(user)]["enabled"] = True
        device.write(CONFIG, json.dumps(config))
        after, observations = wait_for_system_first_attachment(device, before, user, name)
        validate_io(device.app_write(user, name + "-io", "issue18-delayed-attachment"))
        reports.append(dict(
            case=name,
            passed=True,
            delay_seconds=delay_seconds,
            observations=observations,
            pid=before_pid,
            system_dev=top(before["mountinfo"], "/storage/emulated")["dev"],
            srx_dev=fields(after["host"])["dev"],
        ))
        print("PASS " + name, flush=True)
    except Exception as error:
        reports.append(dict(case=name, passed=False, error=str(error)))
        print(f"FAIL {name}: {error}", flush=True)
    finally:
        try:
            device.adb("shell", f"am force-stop --user {user} {APP}")
            device.write(CONFIG, json.dumps(enabled_config))
        except Exception as error:
            reports.append(dict(case=name + "-restore", passed=False, error=str(error)))
            print(f"FAIL {name}-restore: {error}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--user", type=int, default=0)
    parser.add_argument("--isolation-user", type=int)
    parser.add_argument("--timeout", type=int, default=360)
    parser.add_argument("--namespace-bind", action="store_true", help="验证无附加规则的 namespace 回退与父入口覆盖")
    args = parser.parse_args()
    if args.user < 0 or args.timeout < 300:
        parser.error("用户号必须非负，观察预算至少覆盖 300 秒的周期检查")
    if args.isolation_user is not None and (args.isolation_user < 0 or args.isolation_user == args.user):
        parser.error("隔离对照需要另一个非负用户号")
    if args.namespace_bind and args.isolation_user is not None:
        parser.error("namespace-bind 回归不支持同时修改另一用户的策略")
    args.output.mkdir(parents=True, exist_ok=True)
    device = Device(args.serial, args.output)
    if device.adb("shell", "getprop ro.kernel.qemu").strip() != "1":
        raise RuntimeError("命名空间故障回归仅支持模拟器")
    device.su(f"mkdir {LOCK} && printf %s {device.remote.rsplit('/', 1)[1]} > {LOCK}/owner")
    original = {}
    reports = []
    try:
        original = {path: device.read(path) for path in (CONFIG, GLOBAL)}
        args.output.joinpath("original-config.json").write_text(json.dumps(original, indent=2), encoding="utf8")
        device.adb("shell", "mkdir " + device.remote)
        device.adb("push", str(args.fixture), device.remote + "/fixture")
        helper = Path(__file__).with_name("grant-storage-test-permissions.sh")
        device.adb("push", str(helper), device.remote + "/permissions.sh")
        device.su(f"chmod 755 {device.remote}/fixture")
        users = [args.user] + ([] if args.isolation_user is None else [args.isolation_user])
        for user in users:
            device.prepare_user(user)
            permissions = device.su(f"sh {device.remote}/permissions.sh {APP} {user}", timeout=180)
            args.output.joinpath(f"permissions-{user}.txt").write_text(permissions, encoding="utf8")
        device.write(GLOBAL, json.dumps(dict(file_monitor_enabled=False, fuse_fix_enabled=True, storage_backend_mode="auto", verbose_logging_enabled=True, auto_enable_redirect_for_new_apps=False)))
        if args.namespace_bind:
            run_namespace_matrix(device, args.user, args.timeout, reports)
        else:
            enabled_config = {"users": {str(user): {"enabled": True, "allowed_real_paths": ["Pictures/issue18-control"]} for user in users}}
            device.write(CONFIG, json.dumps(enabled_config))
            for delay_seconds in (0, 5, 30):
                run_system_first_attachment(device, args.user, enabled_config, delay_seconds, reports)
        isolation_pid = None
        pid = None
        cases = (("cover", None), ("replace", None), ("cover", None), ("missing", None),
                 ("cover", "/storage/emulated"), ("cover", f"/mnt/user/{args.user}/emulated"), ("foreign", None))
        for index, (mode, single_target) in enumerate(() if args.namespace_bind else cases):
            scope = "" if single_target is None else "-primary" if single_target == "/storage/emulated" else "-user-alias"
            name = f"{index}-{mode}{scope}-{uuid.uuid4().hex[:8]}"
            try:
                if args.isolation_user is not None and isolation_pid is None:
                    isolation_pid = device.start(args.isolation_user)
                if pid is None:
                    pid = device.start(args.user)
                before = device.snapshot(pid, name + "-before")
                isolation_before = None
                if isolation_pid is not None:
                    isolation_before = device.snapshot(isolation_pid, name + "-isolation-before")
                    validate_isolation(isolation_before, isolation_before)
                    validate_io(device.app_write(args.isolation_user, name + "-isolation-control", "issue18-other-user"))
                dev = fields(before["host"])["dev"]
                targets = [target for target in storage_parents(args.user) if top(before["mountinfo"], target)]
                if single_target is not None:
                    if single_target not in targets or len(targets) < 2:
                        raise AssertionError(f"缺少独立于 {single_target} 的存储别名")
                    targets = [single_target]
                control = device.app_write(args.user, name + "-control", "issue18-control")
                validate_io(control)
                alias = single_target if single_target and single_target != "/storage/emulated" else None
                if alias:
                    alias_accessible = device.check_alias_io(args.user, name + "-alias-control", alias)
                commands = tuple(device.fault_command(before, args.user, target, mode, dev) for target in targets)
                fault = device.snapshot(pid, name + "-fault", inject=commands)
                for target in targets:
                    current = top(fault["mountinfo"], target)
                    if (not current and mode != "missing") or current and current["dev"] == dev:
                        raise AssertionError(f"未观测到预期故障：{target}")
                fault_io = device.app_write(args.user, name + "-fault", "issue18-fault")
                # Provider 可能继续保护文件落点；故障和恢复只能由挂载身份判定。
                print(f"FAULT_IO {name}: redirected={fault_io['private'] == fault_io['payload']}", flush=True)
                deadline = time.monotonic() + args.timeout
                observations = 0
                while True:
                    after = device.snapshot(pid, f"{name}-observed-{observations}")
                    observations += 1
                    if not stable(before, after):
                        raise AssertionError("观察对象的身份已改变")
                    if isolation_before is not None:
                        isolation_after = device.snapshot(isolation_pid, f"{name}-isolation-observed-{observations}")
                        validate_isolation(isolation_before, isolation_after)
                    if mode == "foreign" and any(top(after["mountinfo"], target) != top(fault["mountinfo"], target) for target in targets):
                        raise AssertionError("观察期间未知接管者的挂载发生变化")
                    restored = all(view_restored(after, target, dev) for target in targets)
                    if restored and mode != "foreign" or time.monotonic() >= deadline:
                        break
                    if observations % 6 == 1:
                        print(f"WAIT {name}: 等待守护进程的周期检查", flush=True)
                    time.sleep(10)
                validate(before, fault, after, targets, mode == "foreign", mode == "missing")
                if isolation_before is not None:
                    validate_io(device.app_write(args.isolation_user, name + "-isolation-recovered", "issue18-other-user"))
                if mode != "foreign":
                    io = device.app_write(args.user, name + "-recovered", "issue18-recovered")
                    validate_io(io, control["metadata"])
                if alias and device.check_alias_io(args.user, name + "-alias-recovered", alias) != alias_accessible:
                    raise AssertionError(f"应用通过恢复后的别名访问时结果发生变化：{alias}")
                reports.append(dict(case=name, passed=True, targets=targets, mode=mode, observations=observations))
                print("PASS " + name, flush=True)
            except Exception as error:
                reports.append(dict(case=name, passed=False, error=str(error)))
                print(f"FAIL {name}: {error}", flush=True)
                device.adb("shell", f"am force-stop --user {args.user} {APP}")
                pid = None
                if args.isolation_user is not None:
                    device.adb("shell", f"am force-stop --user {args.isolation_user} {APP}")
                    isolation_pid = None
    except Exception as error:
        reports.append(dict(case="runner", passed=False, error=str(error)))
        raise
    finally:
        cleanup_errors = []
        try:
            if args.isolation_user is not None:
                device.adb("shell", f"am force-stop --user {args.isolation_user} {APP}")
        except Exception as error:
            cleanup_errors.append(str(error))
        try:
            device.cleanup(args.user, original)
        except Exception as error:
            cleanup_errors.append(str(error))
        if cleanup_errors:
            reports.append(dict(case="cleanup", passed=False, error="; ".join(cleanup_errors)))
            print("FAIL cleanup: " + "; ".join(cleanup_errors), flush=True)
        args.output.joinpath("results.json").write_text(json.dumps(reports, indent=2), encoding="utf8")
    if not reports or any(not report["passed"] for report in reports):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
