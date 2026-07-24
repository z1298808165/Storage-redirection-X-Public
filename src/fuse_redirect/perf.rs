use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub(super) struct FusePerfStats {
    pub(super) package_name: String,
    pub(super) calls: AtomicU64,
    pub(super) lookup_calls: AtomicU64,
    pub(super) metadata_calls: AtomicU64,
    pub(super) open_calls: AtomicU64,
    pub(super) read_calls: AtomicU64,
    pub(super) write_calls: AtomicU64,
    pub(super) mutation_calls: AtomicU64,
    pub(super) sampled_calls: AtomicU64,
    pub(super) sampled_ns: AtomicU64,
    pub(super) slow_samples: AtomicU64,
}

pub(super) struct FusePerfSample<'a> {
    pub(super) stats: Option<&'a FusePerfStats>,
    pub(super) started: Option<Instant>,
    pub(super) snapshot: bool,
}

impl FusePerfStats {
    pub(super) fn new(package_name: String) -> Self {
        Self {
            package_name,
            calls: AtomicU64::new(0),
            lookup_calls: AtomicU64::new(0),
            metadata_calls: AtomicU64::new(0),
            open_calls: AtomicU64::new(0),
            read_calls: AtomicU64::new(0),
            write_calls: AtomicU64::new(0),
            mutation_calls: AtomicU64::new(0),
            sampled_calls: AtomicU64::new(0),
            sampled_ns: AtomicU64::new(0),
            slow_samples: AtomicU64::new(0),
        }
    }

    pub(super) fn observe<'a>(&'a self, counter: &AtomicU64) -> FusePerfSample<'a> {
        if !crate::logging::is_debug_logging_enabled() {
            return FusePerfSample {
                stats: None,
                started: None,
                snapshot: false,
            };
        }
        counter.fetch_add(1, Ordering::Relaxed);
        let calls = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        FusePerfSample {
            stats: Some(self),
            started: calls.is_multiple_of(256).then(Instant::now),
            snapshot: calls.is_multiple_of(4096),
        }
    }

    pub(super) fn log_snapshot(&self) {
        let sampled = self.sampled_calls.load(Ordering::Relaxed);
        let sampled_ns = self.sampled_ns.load(Ordering::Relaxed);
        log::debug!(
            "perf_snapshot component=fuse pkg={} calls={} lookup={} metadata={} open={} read={} write={} mutation={} samples={} avg_sample_us={} slow_samples={}",
            self.package_name,
            self.calls.load(Ordering::Relaxed),
            self.lookup_calls.load(Ordering::Relaxed),
            self.metadata_calls.load(Ordering::Relaxed),
            self.open_calls.load(Ordering::Relaxed),
            self.read_calls.load(Ordering::Relaxed),
            self.write_calls.load(Ordering::Relaxed),
            self.mutation_calls.load(Ordering::Relaxed),
            sampled,
            sampled_ns.checked_div(sampled.max(1)).unwrap_or(0) / 1000,
            self.slow_samples.load(Ordering::Relaxed),
        );
    }
}

impl Drop for FusePerfSample<'_> {
    fn drop(&mut self) {
        let Some(stats) = self.stats else {
            return;
        };
        if let Some(started) = self.started {
            let elapsed_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            stats.sampled_calls.fetch_add(1, Ordering::Relaxed);
            stats.sampled_ns.fetch_add(elapsed_ns, Ordering::Relaxed);
            if elapsed_ns >= 5_000_000 {
                stats.slow_samples.fetch_add(1, Ordering::Relaxed);
            }
        }
        if self.snapshot {
            stats.log_snapshot();
        }
    }
}
