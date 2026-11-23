//! Opt-in instrumentation for the phase 1 release benchmark.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("DESHCMD_PERF_STAGES").is_some())
}

pub(crate) fn timestamp() -> Option<Instant> {
    enabled().then(Instant::now)
}

pub(crate) fn elapsed(name: &str, start: Option<Instant>) {
    if let Some(start) = start {
        report(name, start.elapsed());
    }
}

fn report(name: &str, duration: Duration) {
    eprintln!("stage,{name},{:.6}", duration.as_secs_f64() * 1000.);
}

#[derive(Default)]
pub(crate) struct ScanTiming {
    start: Option<Instant>,
    metadata: Duration,
    first: Option<Duration>,
    attempts: usize,
    samples: usize,
    comment_reads: usize,
    dedup_paths: usize,
    rng: u32,
}

impl ScanTiming {
    pub(crate) fn new() -> Self {
        Self {
            start: timestamp(),
            rng: 1,
            ..Self::default()
        }
    }

    pub(crate) fn entry<T, E>(&mut self, load: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        let Some(start) = self.start else {
            return load();
        };
        self.attempts += 1;
        // Sample about 1/128 of the rows, avoiding a clock read per entry.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        let sample = (self.rng & 127 == 0).then(Instant::now);
        let result = load();
        if let Some(sample) = sample {
            self.samples += 1;
            self.metadata += sample.elapsed();
        }
        if self.first.is_none() && result.is_ok() {
            self.first = Some(start.elapsed());
        }
        result
    }

    pub(crate) fn comment_read(&mut self) {
        self.comment_reads += 1;
    }

    pub(crate) fn dedup_path(&mut self) {
        self.dedup_paths += 1;
    }

    pub(crate) fn finish(self, capacity: usize) {
        let Some(start) = self.start else {
            return;
        };
        let total = start.elapsed();
        report("scan", total);
        if let Some(first) = self.first {
            report("first_materialized", first);
        }
        if self.samples > 0 {
            let metadata = self
                .metadata
                .mul_f64(self.attempts as f64 / self.samples as f64);
            report("metadata_and_row_estimate", metadata);
            report(
                "enumeration_and_bookkeeping_estimate",
                total.saturating_sub(metadata),
            );
        }
        eprintln!(
            "counts,metadata_attempts={},metadata_samples={},comment_reads={},dedup_path_clones={},batch_inline_bytes={}",
            self.attempts,
            self.samples,
            self.comment_reads,
            self.dedup_paths,
            capacity * std::mem::size_of::<super::Entry>()
        );
    }
}
