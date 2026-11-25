#![allow(dead_code, unused_variables)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
static NOW: AtomicI64 = AtomicI64::new(0);
static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
static LOADED: AtomicBool = AtomicBool::new(false);
mod platform { pub mod paths { pub fn monotonic_ms() -> i64 { crate::NOW.load(crate::Ordering::Relaxed) } } }
mod context { pub struct ReentryGuard; impl ReentryGuard { pub fn enter() -> Self { Self } } }
mod scheduler {
    use super::*;
    static QUEUE: Mutex<Vec<Box<dyn FnOnce() + Send>>> = Mutex::new(Vec::new());
    pub static FAIL: AtomicBool = AtomicBool::new(false);
    pub struct Builder;
    impl Builder {
        pub fn new() -> Self { Self }
        pub fn name(self, _: String) -> Self { self }
        pub fn spawn(self, f: impl FnOnce() + Send + 'static) -> Result<(), ()> {
            if FAIL.load(Ordering::Relaxed) { return Err(()); }
            QUEUE.lock().unwrap().push(Box::new(f));
            Ok(())
        }
    }
    pub fn sleep(_: std::time::Duration) {}
    pub fn drain() {
        let jobs = std::mem::take(&mut *QUEUE.lock().unwrap());
        assert_eq!(jobs.len(), 1, "一批晚加载只能排一次任务");
        for job in jobs { job(); }
    }
}
mod fuse_fix {
    use super::*;
    pub static RETRY_COUNT: AtomicU32 = AtomicU32::new(16);
    pub static LONG_TAIL_RETRY_MS: AtomicI64 = AtomicI64::new(0);
    pub static TARGET_ENABLED: AtomicBool = AtomicBool::new(true);
    pub static INSTALLED: AtomicBool = AtomicBool::new(false);
    fn install_target_if_enabled() {
        // 内核安装边界的桩：必须先解除旧预算，不能让已加载库继续等长尾间隔。
        assert_eq!(RETRY_COUNT.load(Ordering::Relaxed), 0);
        assert!(LONG_TAIL_RETRY_MS.load(Ordering::Relaxed) < 0);
        INSTALLED.store(LOADED.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    // __NATIVE_EVENTS__
}
// __DEFINITIONS__
// __STATE__
// __EVENTS__
fn ensure_fuse_engine_read_patch() {
    for i in 0..3 {
        assert!(PATCH_NEXT_ATTEMPT_MS[i].load(Ordering::Relaxed) < NOW.load(Ordering::Relaxed));
        assert_eq!(PATCH_FAIL_COUNT[i].load(Ordering::Relaxed), 0);
    }
    ATTEMPTS.fetch_add(1, Ordering::Relaxed);
}
fn main() {
    for profile in [HookProfile::AppWrite, HookProfile::FuseFix, HookProfile::SystemWriter, HookProfile::SystemWriterBootLite] {
        assert!(PROFILE_LATE_LOAD_REFRESH.intersects(HookProfileSet::from_profile(profile)));
    }
    for _ in 0..20 { patch_backoff(0, 0); }
    assert_eq!(PATCH_NEXT_ATTEMPT_MS[0].load(Ordering::Relaxed), PATCH_RETRY_MAX_MS);
    retry_after_late_load();
    assert_eq!(ATTEMPTS.load(Ordering::Relaxed), 1);
    assert!(!fuse_fix::INSTALLED.load(Ordering::Relaxed));
    NOW.store(100, Ordering::Relaxed);
    LOADED.store(true, Ordering::Relaxed);
    fuse_fix::RETRY_COUNT.store(16, Ordering::Relaxed);
    fuse_fix::LONG_TAIL_RETRY_MS.store(100, Ordering::Relaxed);
    for _ in 0..20 { retry_after_late_load(); }
    assert_eq!(ATTEMPTS.load(Ordering::Relaxed), 1);
    NOW.store(2000, Ordering::Relaxed);
    scheduler::drain();
    assert!(fuse_fix::INSTALLED.load(Ordering::Relaxed));
    assert_eq!(ATTEMPTS.load(Ordering::Relaxed), 2);
    assert!(!LATE_LOAD_RETRY_PENDING.load(Ordering::Relaxed));
    NOW.store(2100, Ordering::Relaxed);
    scheduler::FAIL.store(true, Ordering::Relaxed);
    retry_after_late_load();
    assert_eq!(ATTEMPTS.load(Ordering::Relaxed), 3);
    assert!(!LATE_LOAD_RETRY_PENDING.load(Ordering::Relaxed));
    NOW.store(1000000, Ordering::Relaxed);
    retry_after_late_load();
    assert_eq!(ATTEMPTS.load(Ordering::Relaxed), 4);
    println!("late-load event matrix passed");
}
