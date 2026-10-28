//! Phase timings for `--timing`, reported on stderr.
//!
//! Commands record the phases that run before and after their core operation
//! (index lookup, freshness check, model setup, output) so the report adds up
//! to the real end-to-end time instead of covering only the search itself.

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static PHASES: Mutex<Vec<(&'static str, Duration)>> = Mutex::new(Vec::new());

/// Anchor `total` at process start; call first thing in `main`.
pub fn mark_process_start() {
    PROCESS_START.get_or_init(Instant::now);
}

/// Run `operation` and record its duration under `phase`.
pub fn timed<T>(phase: &'static str, operation: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let value = operation();
    record_stages(&[(phase, Some(start.elapsed()))]);
    value
}

/// Record stages measured elsewhere (such as the search pipeline's own
/// timings), in order; stages that did not run (`None`) are skipped.
pub fn record_stages(stages: &[(&'static str, Option<Duration>)]) {
    let mut phases = PHASES.lock().unwrap_or_else(|err| err.into_inner());
    phases.extend(
        stages
            .iter()
            .filter_map(|(name, duration)| duration.map(|duration| (*name, duration))),
    );
}

/// Print the recorded phases in order, then `total`: the wall time since
/// process start.
pub fn print_timings() {
    let phases = PHASES.lock().unwrap_or_else(|err| err.into_inner()).clone();
    let total = PROCESS_START.get().map(Instant::elapsed);
    let stderr = std::io::stderr();
    let mut err = stderr.lock();
    for line in timing_lines(&phases, total) {
        let _ = writeln!(err, "{line}");
    }
}

fn timing_lines(phases: &[(&'static str, Duration)], total: Option<Duration>) -> Vec<String> {
    let format = |duration: Duration| format!("{:.1}ms", duration.as_micros() as f64 / 1000.0);
    phases
        .iter()
        .map(|(name, duration)| format!("[timing] {name}: {}", format(*duration)))
        .chain(std::iter::once(format!(
            "[timing] total: {}",
            total.map_or_else(|| "n/a".to_string(), format)
        )))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_lines_list_phases_in_order_then_total() {
        let lines = timing_lines(
            &[
                ("freshness", Duration::from_micros(1500)),
                ("bm25", Duration::from_millis(2)),
                ("search", Duration::from_millis(3)),
            ],
            Some(Duration::from_millis(10)),
        );
        assert_eq!(
            lines,
            [
                "[timing] freshness: 1.5ms",
                "[timing] bm25: 2.0ms",
                "[timing] search: 3.0ms",
                "[timing] total: 10.0ms",
            ]
        );
        assert_eq!(timing_lines(&[], None), ["[timing] total: n/a"]);
    }
}
