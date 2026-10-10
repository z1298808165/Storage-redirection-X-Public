import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MONITOR = ROOT / "src" / "daemon_monitor.rs"


class MonitorResourceBoundariesTest(unittest.TestCase):
    def test_production_repair_flags_execute(self) -> None:
        """执行生产源码中的首次建树标记与修复条件，覆盖连续重建和溢出补偿。"""
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest("环境缺少 rustc，首次建树条件的执行验证已跳过")
        source = MONITOR.read_text(encoding="utf-8")
        start = source.index("pub fn reconfigure(")
        end = source.index("fn retry_missing_watch_roots(", start)
        rebuild = source[start:end]
        first_build = re.search(r"let first_build = [^;]+;", rebuild)
        repair = re.search(r"let repair_existing_files = [^;]+;", rebuild)
        self.assertIsNotNone(first_build, "未找到生产首次建树标记")
        self.assertIsNotNone(repair, "未找到生产既有文件修复条件")
        harness = """
struct WatchWatcher { source: &'static str }
struct WatchNode { watchers: Vec<WatchWatcher> }
struct Monitor { first_build_done: bool }
impl Monitor {
    fn repair_flags(&mut self, overflow_resync: bool) -> [bool; 3] {
        __FIRST_BUILD__
        std::array::from_fn(|index| {
            let node = WatchNode {
                watchers: vec![WatchWatcher {
                    source: ["private_owner", "path_mapping", "public_owner"][index],
                }],
            };
            __REPAIR_FLAGS__
            repair_existing_files
        })
    }
}
fn main() {
    let mut monitor = Monitor { first_build_done: false };
    assert_eq!(monitor.repair_flags(false), [true, false, false], "首次建树必须修复私有文件");
    assert!(monitor.first_build_done, "首次建树完成后应登记状态");
    for _ in 0..12 {
        assert_eq!(monitor.repair_flags(false), [false; 3], "连续重建应跳过既有文件修复");
    }
    assert_eq!(monitor.repair_flags(true), [true; 3], "溢出补偿必须修复全部来源");
    assert_eq!(monitor.repair_flags(false), [false; 3], "溢出补偿后应恢复轻量重建");
    let mut restarted = Monitor { first_build_done: false };
    assert_eq!(restarted.repair_flags(false), [true, false, false], "新实例必须重新执行首次修复");
}
"""
        harness = harness.replace("__FIRST_BUILD__", first_build.group(0)).replace(
            "__REPAIR_FLAGS__", repair.group(0)
        )
        temp_root = ROOT / "temp"
        temp_root.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(
            prefix="monitor-first-build-", dir=temp_root
        ) as tmp:
            harness_path = Path(tmp) / "harness.rs"
            binary = Path(tmp) / (
                "monitor-first-build.exe" if os.name == "nt" else "monitor-first-build"
            )
            harness_path.write_text(harness, encoding="utf-8")
            compiled = subprocess.run(
                [rustc, "--edition=2024", str(harness_path), "-o", str(binary)],
                capture_output=True,
                text=True,
                encoding="utf-8",
            )
            self.assertEqual(compiled.returncode, 0, compiled.stdout + compiled.stderr)
            executed = subprocess.run(
                [str(binary)], capture_output=True, text=True, encoding="utf-8"
            )
            self.assertEqual(executed.returncode, 0, executed.stdout + executed.stderr)

    def test_write_event_repair_flags_execute(self) -> None:
        """执行生产事件判定，验证内容写入不重复修复且其它事件仍保持修复。"""
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest("环境缺少 rustc，写入事件判定的执行验证已跳过")
        source = (ROOT / "src" / "daemon_monitor" / "inotify.rs").read_text(
            encoding="utf-8"
        )
        functions = []
        for name in ("is_relevant_event", "is_owner_repair_event"):
            match = re.search(
                rf"pub\(super\) fn {name}\(mask: u32\) -> bool \{{[^{{}}]+\}}", source
            )
            self.assertIsNotNone(match, f"未找到生产事件判定 {name}")
            functions.append(match.group(0).replace("pub(super)", ""))
        harness = """
const IN_MODIFY: u32 = 0x2;
const IN_ATTRIB: u32 = 0x4;
const IN_CLOSE_WRITE: u32 = 0x8;
const IN_MOVED_FROM: u32 = 0x40;
const IN_MOVED_TO: u32 = 0x80;
const IN_CREATE: u32 = 0x100;
const IN_DELETE: u32 = 0x200;
__FUNCTIONS__
fn main() {
    let repair_events = [IN_ATTRIB, IN_CLOSE_WRITE, IN_MOVED_FROM, IN_MOVED_TO, IN_CREATE, IN_DELETE];
    assert!(is_relevant_event(IN_MODIFY), "内容写入仍应进入文件监视记录");
    assert!(!is_owner_repair_event(IN_MODIFY), "纯内容写入应跳过元数据修复");
    assert!(!is_owner_repair_event(IN_MODIFY | 0x40000000), "目录标志不应恢复重复修复");
    for event in repair_events {
        assert!(is_owner_repair_event(event), "非内容写入事件必须保留修复");
        assert!(is_owner_repair_event(event | IN_MODIFY), "混合事件必须保留修复");
        assert!(is_owner_repair_event(event | 0x40000000), "目录事件必须保留修复");
    }
    for event in [0, 0x400, 0x800, 0x4000, 0x8000] {
        assert!(!is_owner_repair_event(event), "生命周期事件由外层先处理");
    }
    let repairs = std::iter::repeat_n(IN_MODIFY, 10000)
        .chain(std::iter::repeat_n(IN_CLOSE_WRITE, 38))
        .filter(|mask| is_owner_repair_event(*mask)).count();
    assert_eq!(repairs, 38, "持续写入只在关闭时执行补偿检查");
}
""".replace("__FUNCTIONS__", "\n".join(functions))
        temp_root = ROOT / "temp"
        temp_root.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(
            prefix="monitor-event-repair-", dir=temp_root
        ) as tmp:
            path = Path(tmp) / "harness.rs"
            binary = Path(tmp) / (
                "event-repair.exe" if os.name == "nt" else "event-repair"
            )
            path.write_text(harness, encoding="utf-8")
            compiled = subprocess.run(
                [rustc, "--edition=2024", str(path), "-o", str(binary)],
                capture_output=True,
                text=True,
                encoding="utf-8",
            )
            self.assertEqual(compiled.returncode, 0, compiled.stdout + compiled.stderr)
            executed = subprocess.run(
                [str(binary)], capture_output=True, text=True, encoding="utf-8"
            )
            self.assertEqual(executed.returncode, 0, executed.stdout + executed.stderr)

    def test_event_recording_is_outside_owner_repair_gate(self) -> None:
        """修复节流只包住 owner 检查，不吞掉写入事件或新目录的监视登记。"""
        source = MONITOR.read_text(encoding="utf-8")
        start = source.index("if inotify::is_owner_repair_event(mask) {")
        brace = source.index("{", start)
        depth = 1
        end = brace + 1
        while depth:
            if source[end] == "{":
                depth += 1
            elif source[end] == "}":
                depth -= 1
            end += 1
        gated = source[start:end]
        self.assertIn("repair_monitored_backend_owner(", gated)
        self.assertNotIn("emit_monitor_event(", gated)
        self.assertNotIn("expand_watch_tree_from(", gated)
        rest = source[end : source.index("fn should_skip_duplicate(", end)]
        self.assertIn("emit_monitor_event(", rest)
        self.assertIn("expand_watch_tree_from(", rest)


if __name__ == "__main__":
    unittest.main()
