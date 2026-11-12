import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(shutil.which("rustc"), "需要 Rust 验证查询计划缓存")
class CursorRowPlanTest(unittest.TestCase):
    def test_plan_cache_tracks_config_and_caller(self):
        source = (ROOT / "src/hook/jni_query/rewrite.rs").read_text(encoding="utf-8")
        cache = source[source.index("#[derive(Clone)]\nenum CallerRowPlan"):source.index("fn compute_caller_row_plan(")]
        script = r'''
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
static VERSION: AtomicU64 = AtomicU64::new(1);
static COMPUTATIONS: AtomicUsize = AtomicUsize::new(0);
struct SettingsHub;
impl SettingsHub {
    fn instance() -> &'static Self { &Self }
    fn config_version(&self) -> u64 { VERSION.load(Ordering::SeqCst) }
}
fn compute_caller_row_plan(package: &str, uid: i32) -> CallerRowPlan {
    COMPUTATIONS.fetch_add(1, Ordering::SeqCst);
    match VERSION.load(Ordering::SeqCst) {
        2 => CallerRowPlan::PerRow,
        3 => CallerRowPlan::Pass,
        version => CallerRowPlan::Redirect { target: format!("{package}/{uid}/{version}") },
    }
}
fn plan(package: &str, uid: i32) -> String {
    match caller_row_plan(package, uid) {
        CallerRowPlan::PerRow => "rules".into(),
        CallerRowPlan::Pass => "pass".into(),
        CallerRowPlan::Redirect { target } => target,
    }
}
#[test]
fn config_changes_invalidate_each_thread() {
    let (request, requests) = std::sync::mpsc::channel();
    let (reply, replies) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        for _ in requests {
            reply.send(plan("app", 10123)).unwrap();
        }
    });
    for (version, expected) in [(1, "app/10123/1"), (2, "rules"), (3, "pass"), (4, "app/10123/4")] {
        VERSION.store(version, Ordering::SeqCst);
        for _ in 0..2 {
            assert_eq!(plan("app", 10123), expected);
            request.send(()).unwrap();
            assert_eq!(replies.recv().unwrap(), expected);
        }
        assert_eq!(COMPUTATIONS.load(Ordering::SeqCst), version as usize * 2);
    }
    assert_eq!(plan("other", 10123), "other/10123/4");
    assert_eq!(plan("other", 10124), "other/10124/4");
    assert_eq!(COMPUTATIONS.load(Ordering::SeqCst), 10);
    drop(request);
    worker.join().unwrap();
}
'''
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            path = base / "cursor_plan.rs"
            path.write_text(cache + script, encoding="utf-8")
            binary = base / ("cursor_plan.exe" if os.name == "nt" else "cursor_plan")
            build = subprocess.run(["rustc", "--edition=2024", "--test", str(path), "-o", str(binary)],
                                   text=True, capture_output=True, timeout=60)
            self.assertEqual(0, build.returncode, build.stdout + build.stderr)
            result = subprocess.run([str(binary)], text=True, capture_output=True, timeout=15)
            self.assertEqual(0, result.returncode, result.stdout + result.stderr)
