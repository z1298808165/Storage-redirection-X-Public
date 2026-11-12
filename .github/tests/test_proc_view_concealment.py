"""procfs 视图痕迹的基线与门禁测试。

隐藏能力的验收不能只靠人工翻日志：这里用真机采集的 fixture 把「当前痕迹上限」与
「基线干净」两条事实钉住，并锁死内部直读通道的形态。全部断言不需要设备与 root。

- leak_* 计数是上限：任何改动让痕迹回升立即失败（隐藏不能倒退）。
- parity_* 计数是下限：命名空间形态与原型设备对齐的进度登记，达成后由登记转为门禁。
- fixture 里出现未被登记的痕迹字符串也失败：防止新增痕迹类别绕过登记表。
"""

import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / ".github" / "tests" / "fixtures" / "proc_view"
BOUNDS = FIXTURES / "trace_bounds.json"

# 挂载记录里只要出现这些字符串，就说明这一行把本模块暴露给了应用。
TRACE_TOKENS = ("srx", "storage.redirect", "data/adb")

PROPAGATION_TAGS = ("master:", "shared:", "propagate_from:", "unbindable")


def read_source(path: str) -> str:
    return (ROOT / "src" / path).read_text(encoding="utf-8")


def parse_entry(line: str) -> dict:
    """解析一行 mountinfo。

    格式：`id parent major:minor root mountpoint options... - fstype source super...`，
    分隔符是独立成词的 `-`（挂载选项里不会出现该形态）。
    """
    head, tail = line.split(" - ", 1)
    fields = head.split()
    after = tail.split()
    if len(fields) < 5 or len(after) < 2:
        raise AssertionError(f"无法解析的 mountinfo 行: {line}")
    return {
        "id": fields[0],
        "parent": fields[1],
        "dev": fields[2],
        "root": fields[3],
        "target": fields[4],
        "options": fields[5:],
        "fs_type": after[0],
        "source": after[1],
    }


def load_entries(name: str) -> list[dict]:
    text = (FIXTURES / name).read_text(encoding="utf-8")
    lines = [line for line in text.splitlines() if line.strip()]
    return [parse_entry(line) for line in lines]


def storage_targets(entries: list[dict], current_user: int) -> set[str]:
    """该命名空间里**属于当前用户**的存储挂载点集合。

    别的用户的别名（`/mnt/user/<other>/emulated` 等）也会出现在同一命名空间里，
    它们在两次采集之间是否存在只取决于设备状态，不构成痕迹，必须排除。
    """
    prefixes = ("/storage/", "/mnt/", "/data/media/")
    targets = set()
    for entry in entries:
        target = entry["target"]
        if not target.startswith(prefixes):
            continue
        segments = target.strip("/").split("/")
        other_user = False
        for index, segment in enumerate(segments):
            if segment.isdigit() and int(segment) != current_user:
                # 只把「紧跟 emulated 的段」或「/storage/emulated/<n>」视为用户号；
                # 其余数字段（如 Android/11）与用户无关。
                previous = segments[index - 1] if index else ""
                if previous in ("emulated", "user", "installer", "androidwritable", "pass_through"):
                    other_user = True
                    break
        if other_user:
            continue
        targets.add(target)
    return targets


def serving_entry(entries: list[dict], path: str) -> dict | None:
    """返回该路径当前由哪一层挂载服务。

    与内核路径遍历同口径：取「是目标路径祖先或本身」的挂载点里最深的那个。设备号相同
    等价于同一个超级块，因此这是应用不需要任何参照就能做的自洽性检查——同一个存储树
    经两条别名访问到的必须是同一个文件系统。
    """
    best = None
    for entry in entries:
        target = entry["target"]
        if target != path and not path.startswith(target.rstrip("/") + "/"):
            continue
        if best is None or len(target) > len(best["target"]):
            best = entry
    return best


def alias_dev_mismatch(entries: list[dict], current_user: int = 0) -> int:
    """统计「主视图与别名指向不同超级块」的用户数。

    原型设备上 `/storage/emulated/<u>` 与 `/mnt/user/<u>/emulated` 是同一个 MediaProvider
    FUSE 超级块；一旦主视图被模块替换而别名没跟着换，应用只要 `stat` 两端就能发现
    "同一份存储有两个互不相同的文件系统"，这比任何字符串痕迹都更直接。

    只在两端都由 FUSE 服务时比较：没有存储权限的应用根本看不到 `/storage/emulated` 这一层
    （落回根挂载），那不是差异也不是痕迹，必须跳过，否则基线会长期假阳性。

    **只比较当前用户**：多用户设备上命名空间里同时挂着别的用户的 `/mnt/user/<other>/emulated`
    （各自的 MediaProvider 实例、各自的超级块，例如真机 2026-11-02：user0=0:131、user999=0:168）。
    把它们与 `/storage/emulated/<other>` 相比必然不等，那是原生形态而不是痕迹——早期版本
    在这里产生过假阳性。
    """
    mismatched = 0
    for user in (str(current_user),):
        primary = serving_entry(entries, f"/storage/emulated/{user}")
        alias = serving_entry(entries, f"/mnt/user/{user}/emulated/{user}")
        if not primary or not alias:
            continue
        if not primary["fs_type"].startswith("fuse") or not alias["fs_type"].startswith("fuse"):
            continue
        if primary["dev"] != alias["dev"]:
            mismatched += 1
    return mismatched


def measure(entries: list[dict], current_user: int = 0) -> dict:
    """按登记表的口径统计痕迹。

    口径必须与 trace_bounds.json 的描述一致：只数「应用可见的挂载文本」，不看内部账本。
    """
    counts = {
        "leak_module_fuse_source": 0,
        "leak_module_path": 0,
        "leak_barrier_tmpfs": 0,
        "leak_alias_dev_mismatch": alias_dev_mismatch(entries, current_user),
        "parity_master_tag_lines": 0,
    }
    for entry in entries:
        if entry["source"].startswith("srx_fuse"):
            counts["leak_module_fuse_source"] += 1
        if any(
            "/data/adb/" in field
            for field in (entry["root"], entry["target"], entry["source"])
        ):
            counts["leak_module_path"] += 1
        if entry["fs_type"] == "tmpfs" and entry["source"] in ("none", ""):
            counts["leak_barrier_tmpfs"] += 1
        if any(option.startswith(tag) for option in entry["options"] for tag in PROPAGATION_TAGS):
            counts["parity_master_tag_lines"] += 1
    return counts


def bounds() -> dict:
    return json.loads(BOUNDS.read_text(encoding="utf-8"))


class ProcViewFixtureTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.config = bounds()
        cls.stock = load_entries(cls.config["stock"]["file"])
        cls.redirected = load_entries(cls.config["redirected"]["file"])

    def test_fixtures_are_complete(self) -> None:
        # 采集被截断会让下面的上限断言变成空转，这里先确认两份视图都是完整命名空间。
        self.assertGreater(len(self.stock), 150, "对照基线 fixture 疑似被截断")
        self.assertGreater(len(self.redirected), 150, "被接管进程 fixture 疑似被截断")

    def test_storage_mount_set_delta_is_registered(self) -> None:
        """存储路径上的挂载集合差异必须与登记完全相等。

        这是比"字符串痕迹"更强的判据：应用不需要任何参照就能数自己的挂载集合，多一条
        正常设备不存在（或有、但在这里被吃掉）的存储挂载都是可枚举的差异。

        只统计**当前用户**的路径：多用户设备上 vold 会把别的用户的别名
        （`/mnt/user/999/emulated` 等）也挂进同一个命名空间，它们在两份基线之间是否存在
        取决于采集时另一个用户有没有起来，属于设备状态而不是痕迹。
        """
        stock = load_entries("stock_app_mountinfo.txt")
        redirected = load_entries("redirected_app_mountinfo.txt")
        extra = sorted(storage_targets(redirected, 0) - storage_targets(stock, 0))
        missing = sorted(storage_targets(stock, 0) - storage_targets(redirected, 0))
        self.assertEqual(
            bounds()["mount_set"]["redirected_extra"],
            extra,
            "存储挂载集合出现了未登记的差异（新增即新痕迹）",
        )
        self.assertEqual([], missing, "丢失了原型里存在的存储挂载（重定向出口缺失）")

    def test_stock_baseline_has_no_module_trace(self) -> None:
        counts = measure(self.stock)
        self.assertEqual(0, counts["leak_module_fuse_source"], "对照基线不应出现模块挂载源")
        self.assertEqual(0, counts["leak_module_path"], "对照基线不应出现模块私有路径")
        self.assertEqual(0, counts["leak_barrier_tmpfs"], "对照基线不应出现 barrier tmpfs")
        # 原型设备的命名空间里有大量 slave 传播标签，它是形态对齐的参照值。
        self.assertGreater(
            counts["parity_master_tag_lines"], 100, "对照基线的传播标签样本不足"
        )

    def test_redirected_traces_within_registered_bounds(self) -> None:
        counts = measure(self.redirected)
        for key, limit in self.config["bounds"].items():
            if key.startswith("leak_"):
                self.assertLessEqual(
                    counts[key],
                    limit["max"],
                    f"{key} 回升到 {counts[key]}（登记上限 {limit['max']}）：{limit['desc']}",
                )
            else:
                self.assertGreaterEqual(
                    counts[key],
                    limit["min"],
                    f"{key} 跌到 {counts[key]}（登记下限 {limit['min']}）：{limit['desc']}",
                )

    def test_no_unregistered_trace_line(self) -> None:
        # 每一行含模块标识的记录都必须能被某个已登记的痕迹类别解释，否则说明出现了
        # 登记表之外的新痕迹（例如新增了一层挂载），必须先登记再修。
        registered = 0
        for entry in self.redirected:
            fields = (entry["root"], entry["target"], entry["source"])
            if not any(token in field for field in fields for token in TRACE_TOKENS):
                continue
            explained = entry["source"].startswith("srx_fuse") or any(
                "/data/adb/" in field for field in fields
            )
            self.assertTrue(
                explained,
                f"未登记的痕迹行 target={entry['target']} source={entry['source']}",
            )
            registered += 1
        counts = measure(self.redirected)
        self.assertEqual(
            counts["leak_module_fuse_source"] + counts["leak_module_path"],
            registered,
            "含模块标识的行数与登记口径不一致，说明登记表与实测已漂移",
        )


class DirectReadChannelTest(unittest.TestCase):
    """就绪判定必须读内核事实，而不是读本进程里其它 hook 允许它看到的内容。"""

    def test_app_side_readiness_uses_direct_channel(self) -> None:
        source = read_source("module_mount_source.rs")
        start = source.index("pub fn app_redirect_mounts_in(")
        body = source[start:]
        self.assertIn("proc_direct::read_proc_file(", body)
        # std::fs 会经过 PLT 上的 open/read，任何 hook（我们自己的或第三方的）都能改写。
        self.assertNotIn("fs::read_to_string(", body)

    def test_channel_bypasses_plt_and_does_not_poll(self) -> None:
        channel = read_source("platform/proc_direct.rs")
        for symbol in ("SYS_openat", "SYS_read", "SYS_close"):
            self.assertIn(symbol, channel, f"直读通道必须走裸系统调用 {symbol}")
        # 走 libc 包装函数就等于把自己交回 hook 手里。
        for hookable in ("libc::read(", "libc::open(", "libc::openat(", "std::fs"):
            self.assertNotIn(hookable, channel, f"直读通道不得使用可被 hook 的 {hookable}")
        # 通道是一次性读取：失败即返回，不允许重试等待（那是最难排查的轮询形态）。
        for polling in ("thread::sleep", "usleep", "Instant::now"):
            self.assertNotIn(polling, channel, f"直读通道不得包含等待逻辑 {polling}")

    def test_raw_syscall_reader_has_single_implementation(self) -> None:
        caller = read_source("hook/caller.rs")
        self.assertNotIn("SYS_openat", caller, "调方应复用 platform::proc_direct，不要自带一份")
        for name in ("fn read_single_line_by_syscall", "fn read_file_by_syscall"):
            start = caller.index(name)
            body = caller[start : caller.index("\n}\n", start)]
            self.assertIn("proc_direct::read_proc_file(", body, f"{name} 必须复用直读通道")


class PropagationParityTest(unittest.TestCase):
    """命名空间形态必须与原型设备同形：整树私有化会把传播标签全部抹平。"""

    def test_no_whole_tree_privatize_on_mount_paths(self) -> None:
        # 整树 `MS_REC|MS_PRIVATE /` 是"0 行 master 标签"的直接原因（真机 111 → 0）。
        # 正常路径只允许最小隔离；整树私有化只能作为复核失败后的兜底，且必须留在
        # mount/propagation.rs 里唯一一处。
        for path in ("mount/planner.rs", "lifecycle/companion_mount.rs"):
            source = read_source(path)
            self.assertNotIn(
                "MS_REC | MS_PRIVATE",
                source,
                f"{path} 不得再整树私有化（会让应用命名空间丢失全部传播标签）",
            )
            self.assertIn(
                "propagation::isolate_storage_mounts(",
                source,
                f"{path} 必须走最小传播隔离",
            )
            self.assertIn(
                "make_tree_private_fallback(",
                source,
                f"{path} 必须保留复核失败时的安全兜底",
            )

    def test_isolation_targets_shared_and_storage_subtrees(self) -> None:
        module = read_source("mount/propagation.rs")
        # 外播只发生在 shared 对等组上，入向传播只压在存储别名子树上：这两条是全部目标。
        self.assertIn('has_propagation_tag(line, "shared:")', module)
        self.assertIn("storage_alias_roots_for_user(", module)
        # 复核必须存在，否则"最小隔离"退化成没有保证的乐观假设。
        self.assertIn("fn verify_isolation(", module)
        self.assertIn('has_propagation_tag(line, "master:")', module)
        # 复核读的必须是自己刚改完的真实状态，因此要重新读一次 mountinfo。
        self.assertGreaterEqual(module.count("read_mountinfo_lines()"), 2)

    def test_isolation_self_heals_before_giving_up(self) -> None:
        # 复核失败不能立刻退回整树私有化：MediaProvider 换代/切用户会在第一轮私有化之后
        # 把新的挂载换到同一路径上（真机 2026-11-02 实测：重启 MP 后 `/storage/emulated`
        # 换成带 master: 的新层，我们从没碰过它），一次竞态就把 111 条传播标签抹平。
        module = read_source("mount/propagation.rs")
        self.assertIn("fn isolation_violations(", module)
        self.assertIn("verify self-heal", module)
        # 复核 → 自愈 → 再复核：循环体必须是两次尝试，且自愈调用的私有化是非递归的。
        body = module[module.index("fn verify_isolation(") :]
        self.assertIn("for attempt in 0..2 {", body)
        self.assertIn("make_private(target, false)", body)
        self.assertIn("if healed == 0 {", body)

    def test_isolation_uses_direct_channel(self) -> None:
        # 这里的调用发生在应用进程内（companion 的 fork 之后），读挂载表同样不能走
        # 可被 hook 的 libc 路径，否则隔离判据本身可能被改写。
        module = read_source("mount/propagation.rs")
        self.assertIn("proc_direct::read_proc_file(", module)
        self.assertNotIn("std::fs", module)

    def test_isolation_never_privatizes_generic_ancestors(self) -> None:
        # 别名清单里有些路径在本命名空间并不存在（`/mnt/runtime/*/emulated/<u>`），
        # 它们的最长前缀会退到 `/mnt`、`/storage`。递归私有化这类通用祖先会把整片无关
        # 子树的 `master:` 标签一并抹掉（真机实测 13 条 `/mnt/vendor/*` 受害）。
        module = read_source("mount/propagation.rs")
        self.assertIn("fn is_storage_view_mount(", module)
        self.assertIn("if !is_storage_view_mount(&serving)", module)
        # 私有化必须非递归：只需要断开这一层自身的入向传播。
        self.assertIn("make_private(&serving, false)", module)
        self.assertNotIn("make_private(&serving, true)", module)


class BarrierLifecycleTest(unittest.TestCase):
    """fail-closed 屏障必须在视图接入前摘除，且不能留在挂载表里。"""

    def setUp(self) -> None:
        self.companion = read_source("lifecycle/companion_mount.rs")

    def test_barrier_is_released_before_view_attach(self) -> None:
        # 视图根一旦落到屏障路径上方，屏障就既不能按路径、也不能按 fd 摘除——内核解析
        # 最后一段时会重新爬到该路径的栈顶（真机探针实测：卸掉的是视图根本身）。
        # 所以顺序是硬约束：先摘屏障，再接入视图。
        detach_at = self.companion.index("detach_inherited_fail_closed_barriers(")
        preattach_at = self.companion.index("preattach_shared_host(request, scoped_fuse_roots)")
        self.assertLess(detach_at, preattach_at, "摘屏障必须排在视图接入之前")

    def test_barrier_fds_are_opened_while_barrier_is_topmost(self) -> None:
        # fd 必须在屏障刚压上、仍是栈顶时抓取，且要先核一遍服务挂载确实是屏障
        # （热重载时同一路径上可能已是上一轮的视图根 FUSE）。
        self.assertIn("fn open_fail_closed_barrier_fds(", self.companion)
        self.assertIn("barrier_fds = open_fail_closed_barrier_fds(request, &barrier_roots)", self.companion)
        body = self.companion[
            self.companion.index("fn open_fail_closed_barrier_fds(") : self.companion.index(
                "fn detach_inherited_fail_closed_barriers("
            )
        ]
        self.assertIn("is_barrier_entry", body)
        self.assertIn('libc::O_PATH', body)

    def test_failed_mount_reinstalls_barrier(self) -> None:
        # fail-closed 不能降级：摘了屏障而接管失败，必须重新压回去。
        self.assertIn("if !is_success && !barrier_roots.is_empty()", self.companion)
        reinstall_start = self.companion.index("if !is_success && !barrier_roots.is_empty()")
        self.assertIn(
            "mount_storage_view_barriers(request, &barrier_roots)",
            self.companion[reinstall_start : reinstall_start + 400],
        )


class FuseSessionIdentityTest(unittest.TestCase):
    """挂载源原生化之后，会话身份只能按超级块判定。"""

    def test_module_fuse_identity_is_superblock_based(self) -> None:
        companion = read_source("lifecycle/companion_mount.rs")
        # 按源前缀判定在源原生化之后恒为假，会把每次正常挂载误判成"屏障裸奔"并返回失败
        # （真机实测 6 次误报，其中 4 次是被接管的微信）。
        self.assertNotIn("is_srx_fuse_source", companion)
        self.assertNotIn("starts_with(\"srx_fuse\")", companion)
        self.assertIn("fn is_module_fuse_entry(", companion)
        self.assertIn("fn session_fuse_devs(", companion)
        self.assertIn("devs.contains(&entry.dev)", companion)

    def test_fuse_mount_options_match_mediator(self) -> None:
        # 与系统 MediaProvider 的 FUSE 挂载逐项对齐：`nosuid,nodev,noexec,noatime`
        # 加超级选项 `lazytime`。少任何一项本身就是"这层不是系统建的"的判据。
        config = read_source("fuse_redirect/config.rs")
        for option in (
            "MountOption::NoSuid",
            "MountOption::NoDev",
            "MountOption::NoExec",
            "MountOption::NoAtime",
            "MountOption::LazyTime",
        ):
            self.assertIn(option, config, f"宿主 FUSE 挂载选项缺少 {option}")


class MultiUserParityTest(unittest.TestCase):
    """隐蔽性必须与用户无关：第二用户（user 999）的痕迹上限与主用户相同。"""

    def test_stock_baseline_user999_is_clean(self) -> None:
        counts = measure(load_entries("stock_app_mountinfo_user999.txt"), 999)
        for key in ("leak_module_fuse_source", "leak_module_path", "leak_barrier_tmpfs"):
            self.assertEqual(0, counts[key], f"第二用户原型基线出现 {key}")
        self.assertEqual(0, counts["leak_alias_dev_mismatch"], "第二用户原型基线别名不一致")

    def test_redirected_user999_within_same_bounds(self) -> None:
        counts = measure(load_entries("redirected_app_mountinfo_user999.txt"), 999)
        for key in (
            "leak_module_fuse_source",
            "leak_module_path",
            "leak_barrier_tmpfs",
            "leak_alias_dev_mismatch",
        ):
            self.assertEqual(0, counts[key], f"第二用户被接管进程出现 {key}")
        self.assertGreaterEqual(
            counts["parity_master_tag_lines"],
            bounds()["bounds"]["parity_master_tag_lines"]["min"],
            "第二用户的传播标签数低于主用户的下限",
        )

    def test_redirected_user999_mount_delta_is_registered(self) -> None:
        stock = storage_targets(load_entries("stock_app_mountinfo_user999.txt"), 999)
        redirected = storage_targets(load_entries("redirected_app_mountinfo_user999.txt"), 999)
        self.assertEqual(
            bounds()["mount_set"]["user999_extra"],
            sorted(redirected - stock),
            "第二用户的存储挂载集合出现未登记差异",
        )
        self.assertEqual([], sorted(stock - redirected), "第二用户丢失了原型里存在的存储挂载")


class NamespaceBackendConcealmentTest(unittest.TestCase):
    """namespace 兜底后端同样不得留下模块路径或屏障。"""

    def test_namespace_backend_has_no_module_trace(self) -> None:
        entries = load_entries("namespace_app_mountinfo.txt")
        counts = measure(entries)
        self.assertEqual(0, counts["leak_module_path"], "namespace 后端出现了模块私有路径挂载")
        self.assertEqual(0, counts["leak_module_fuse_source"], "namespace 后端出现了模块挂载源")
        self.assertEqual(0, counts["leak_barrier_tmpfs"], "namespace 后端残留 fail-closed 屏障")
        # 该形态必须有重定向本身（存储根是沙箱 bind），否则说明捕获到的不是被接管进程。
        self.assertTrue(
            any(
                entry["target"] == "/storage/emulated/0"
                and entry["fs_type"] in ("f2fs", "ext4", "fuse")
                for entry in entries
            ),
            "fixture 未体现存储根重定向层",
        )


class SystemViewCloneTest(unittest.TestCase):
    """自有私有目录的恢复源必须取"挂载之前"的系统视图快照。

    坍缩形态下别名挂载 `.../emulated/<user>` 一旦落地，经任何路径解析 `<...>/emulated/<user>/...`
    都会命中我们自己的宿主会话；root 请求没有会话策略（`fallback_deny`）→ ENOENT →
    恢复 bind 失败（真机 2026-11-02：`own private system view restored count=0`）。
    因此源必须是**挂载之前**的游离克隆，且只能用 `move_mount` 附着（按路径 bind 游离挂载
    会被内核判 `EINVAL`：`do_loopback` 要求源属于当前命名空间）。
    """

    def setUp(self) -> None:
        self.mount = read_source("mount.rs")
        self.apply = read_source("mount/apply.rs")
        self.companion = read_source("lifecycle/companion_mount.rs")

    def test_clone_uses_open_tree_and_move_mount(self) -> None:
        self.assertIn("pub struct SystemViewClone", self.mount)
        self.assertIn("libc::SYS_open_tree", self.mount)
        self.assertIn("libc::SYS_move_mount", self.mount)
        # 不允许把克隆当成普通路径 bind 源（内核 EINVAL）。
        self.assertNotIn('"/proc/self/fd/{}/{}"', self.mount)

    def test_clone_is_captured_before_any_module_mount(self) -> None:
        capture_at = self.companion.index("SystemViewClone::capture(")
        preattach_at = self.companion.index("preattach_shared_host(")
        self.assertLess(
            capture_at,
            preattach_at,
            "系统视图克隆必须在 preattach_shared_host 之前取，否则快照已被本模块覆盖",
        )

    def test_daemon_path_also_captures_before_mount(self) -> None:
        # daemon 路径同样会挂本模块视图（开机窗口的请求常走这条），因此也必须"先快照后挂载"。
        daemon = read_source("daemon_mount.rs")
        capture_at = daemon.index("SystemViewClone::capture(")
        preattach_at = daemon.index("preattach_shared_host(")
        self.assertLess(capture_at, preattach_at, "daemon 路径也必须先捕获系统视图再挂载")
        self.assertIn("planner.set_system_view_clone(clone)", daemon)

    def test_daemon_cleanup_uses_ledger_dev_ownership(self) -> None:
        # 源原生化后 source/root 形态不再可辨，归属必须回到账本的超级块身份。
        daemon = read_source("daemon_mount.rs")
        self.assertIn("is_owned_by_dev", daemon)
        self.assertIn("mount.dev == live.dev", daemon)

    def test_anchor_prefers_clone_over_live_bind(self) -> None:
        start = self.apply.index("fn create_transient_system_view_anchor(")
        # 函数体到下一个顶层结构体为止（锚点结构体紧跟 impl 块之后）。
        body = self.apply[start : self.apply.index("struct SystemViewAnchor", start)]
        self.assertIn("system_view_clone.borrow_mut().take()", body)
        self.assertIn("clone.attach_at(&anchor_root)", body)
        # 克隆优先：其分支必须早于回退源。
        #
        # 回退源必须是**系统视图**：借 `/proc/1/root` 在 init 命名空间里解析同一条路径
        # （模块从不挂载 init 命名空间，那里的 `/mnt/user/<user>/emulated` 恒为
        # MediaProvider 的 FUSE）。不得退回 `/mnt/pass_through/...` 这类 f2fs 直连后端——
        # 那样自有私有目录里的 SQLite 边车会按调用方 umask 落 0600，MP 用自身身份打不开，
        # 跨进程读同一份库的一方即 `SQLITE_CANTOPEN`（真机 2026-11-12 duchamp 实测）。
        self.assertNotIn('format!("/mnt/pass_through/', body)
        self.assertLess(
            body.index("system_view_clone.borrow_mut().take()"),
            body.index('format!("/proc/1/root/mnt/user/{}/emulated"'),
            "克隆锚点必须优先于回退的系统视图源",
        )


class AndroidMediaVisibilityTest(unittest.TestCase):
    """`Android/media` 必须按原生共享语义列举，`data`/`obb` 仍只列自有。

    原生：`/storage/emulated/<u>/Android` 列 `data|media|obb`；`Android/media` 列**所有**
    包目录（跨应用共享）；`Android/data|obb` 只列自有包名（scoped storage）。
    真机 2026-11-02 应用域实测（微信进程内 readdir）：修复前 `Android/media` 只返回
    `["com.tencent.mm"]`（原型 8~9 个包），既是功能缺陷，也是一行 readdir 就能看出的差异。
    """

    def setUp(self) -> None:
        self.policy = read_source("fuse_redirect/policy.rs")

    def test_shared_media_subtree_is_decided_real(self) -> None:
        self.assertIn("fn is_shared_media_subtree_path(", self.policy)
        # 必须挂在决策链的 Real 分支上（与 allowed_real_paths 同一支），且位于
        # excluded_real 之后——用户显式排除仍要能把它推回 redirect。
        body = self.policy[
            self.policy.index("fn backend_decision(") : self.policy.index(
                "fn matches_any("
            )
        ]
        self.assertIn("|| self.is_shared_media_subtree_path(storage_path)", body)
        excluded_at = body.index("self.matches_any(&self.excluded_real_index, storage_path)")
        shared_at = body.index("self.is_shared_media_subtree_path(storage_path)")
        self.assertLess(excluded_at, shared_at, "excluded_real_paths 必须优先于共享媒体判定")

    def test_private_categories_stay_hidden(self) -> None:
        # `Android/data`、`Android/obb` 只列自有是**原生**语义，不能被一起放宽。
        body = self.policy[
            self.policy.index("fn is_shared_media_subtree_path(") : self.policy.index(
                "fn has_real_child_rule("
            )
        ]
        self.assertIn('paths::eq_ignore_case(category, "media")', body)
        self.assertNotIn('"data"', body)
        self.assertNotIn('"obb"', body)


class AttributePresentationTest(unittest.TestCase):
    """属性面（stat 可见）不得出现"内容改写"式痕迹。

    应用不需要参照物就能 stat 自己的存储树；把 mode/nlink/size 这类后端事实改写成
    模块内部常量，会让**已存在的系统目录**（`DCIM` 原生 02773、`Android` 原生 02771）
    统一变成 02770，成为一条可直接枚举的差异（真机 2026-11-02 应用域实测）。
    """

    def setUp(self) -> None:
        self.module = read_source("fuse_redirect/mod.rs")

    def test_dir_mode_is_passed_through_not_rewritten(self) -> None:
        body = self.module[
            self.module.index("fn attr_for_backend(") : self.module.index(
                "fn visible_attr_for_backend("
            )
        ]
        self.assertNotIn("attr.perm =", body, "attr_for_backend 不得改写目录 mode")
        # 属主归一仍保留（落盘统一 1023，见 create_metadata），但只能动 uid/gid。
        self.assertIn("attr.uid = policy.uid as u32", body)
        self.assertIn("attr.gid = MEDIA_RW_GID", body)

    def test_synthetic_virtual_dirs_keep_native_dir_mode(self) -> None:
        # 虚拟目录没有后端实体，唯一能给出的值必须与原生新建目录一致（02773）。
        attrs = read_source("fuse_redirect/attrs.rs")
        self.assertIn("perm: 0o2773", attrs)

    def test_create_path_still_owns_public_entries_as_media_rw(self) -> None:
        # 落盘属主模型目前统一 1023:1023 + 02770/0660（与原生"创建者属主"不同，
        # 见方案文档 §7.10 的待办）；这条断言锁住现状，避免两处漂移。
        metadata = read_source("fuse_redirect/metadata.rs")
        self.assertIn("uid: MEDIA_RW_UID", metadata)
        self.assertIn("gid: MEDIA_RW_GID", metadata)


class RealStorageAnchorTest(unittest.TestCase):
    """namespace 后端的真实存储锚点不得留在应用挂载表里。"""

    def setUp(self) -> None:
        self.apply = read_source("mount/apply.rs")
        self.targets = read_source("mount/apply_targets.rs")

    def test_anchor_source_is_emulated_root_not_redirected_view(self) -> None:
        # 源必须是 emulated 根（本模块从不覆盖它），且非递归——否则锚点会指向自己的
        # 重定向沙箱，映射目标静默落进沙箱（历史故障）。
        body = self.apply[
            self.apply.index("fn prepare_real_storage_anchor(") : self.apply.index(
                "fn real_storage_anchor_is_usable("
            )
        ]
        self.assertIn('format!("/mnt/user/{}/emulated", self.user_id)', body)
        self.assertIn('"/data/media".to_string()', body)
        self.assertIn("bind_mount(&source, &anchor_mount, false)", body)
        # 旧的"摘视图根→重建锚点→立刻绑回"三重系统调用窗口必须彻底消失：
        # 它意味着 fail-open 窗口，而新源不可能被自身重定向污染，不该再需要它。
        self.assertNotIn("detach_mount_if_present(storage_path)", body)
        self.assertNotIn("bind_mount(storage_path", body)

    def test_anchor_usable_root_adds_user_segment(self) -> None:
        # emulated 根下第一段才是用户存储根，可用根必须是 `<anchor>/<user>`。
        self.assertIn("fn real_storage_anchor_mount_path(", self.targets)
        self.assertIn("fn real_storage_anchor_usable_path(", self.targets)
        start = self.targets.index("fn real_storage_anchor_usable_path(")
        body = self.targets[start:].split("\n}\n", 1)[0]
        self.assertIn("real_storage_anchor_mount_path(user_id)", body)

    def test_anchor_is_detached_after_each_mount(self) -> None:
        # 用完即摘：否则应用挂载表里会留下 `tmp/real_storage/<user>` 这条模块私有路径。
        self.assertIn("pub fn detach_real_storage_anchor(", self.apply)
        for path, var in (
            ("lifecycle/companion_mount.rs", "mount_mgr"),
            ("daemon_mount.rs", "planner"),
        ):
            source = read_source(path)
            self.assertIn(
                f"{var}.detach_real_storage_anchor();",
                source,
                f"{path} 必须在挂载收尾时摘除真实存储锚点",
            )


if __name__ == "__main__":
    unittest.main()
