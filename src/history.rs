//! Local record of how long each workflow usually takes, used to turn a run's
//! elapsed time into a progress percentage.
//!
//! Persisted as JSON at `%APPDATA%\actions-monitor\history.json`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// How many past durations we keep per workflow.
pub const MAX_SAMPLES: usize = 20;

/// Progress bars never claim completion before the API says so.
const MAX_ESTIMATED_FRACTION: f32 = 0.95;

/// What to draw in a run card's progress bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Progress {
    /// No history for this workflow yet - draw a pulsing bar instead.
    Indeterminate,
    /// A fraction in `0.0..=1.0`.
    Fraction(f32),
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct History {
    /// `owner/repo::workflow name` -> durations in seconds, oldest first.
    #[serde(default)]
    workflows: BTreeMap<String, Vec<u64>>,
}

impl History {
    pub fn key(repo: &str, workflow: &str) -> String {
        format!("{repo}::{workflow}")
    }

    /// Read history from disk. A missing file is not an error (first run); a
    /// corrupt one is reported so the caller can log it and start fresh.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        };
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// Write history via a temporary file + rename so an interrupted write can
    /// never leave a truncated JSON file behind.
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn record(&mut self, key: &str, duration: Duration) {
        let secs = duration.as_secs();
        if secs == 0 {
            return;
        }
        let samples = self.workflows.entry(key.to_owned()).or_default();
        samples.push(secs);
        if samples.len() > MAX_SAMPLES {
            let excess = samples.len() - MAX_SAMPLES;
            samples.drain(..excess);
        }
    }

    pub fn median(&self, key: &str) -> Option<Duration> {
        let samples = self.workflows.get(key)?;
        median_secs(samples).map(Duration::from_secs)
    }

    #[cfg(test)]
    pub fn sample_count(&self, key: &str) -> usize {
        self.workflows.get(key).map_or(0, Vec::len)
    }
}

/// Median of an unsorted slice. Even-length inputs average the two middles,
/// rounding to the nearest second.
fn median_secs(samples: &[u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        // Average without overflowing on large values.
        let (a, b) = (sorted[mid - 1], sorted[mid]);
        a + (b - a).div_ceil(2)
    })
}

/// Estimate how far along a run is.
///
/// Without history we cannot say anything, so the bar pulses. With history we
/// report `elapsed / median`, capped at 95% - a run that overruns its estimate
/// should look nearly-done, never done.
pub fn estimate(elapsed: Duration, median: Option<Duration>) -> Progress {
    match median {
        Some(median) if median.as_secs_f32() > 0.0 => {
            let fraction = elapsed.as_secs_f32() / median.as_secs_f32();
            Progress::Fraction(fraction.clamp(0.0, MAX_ESTIMATED_FRACTION))
        }
        _ => Progress::Indeterminate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn median_of_odd_and_even_sample_counts() {
        assert_eq!(median_secs(&[]), None);
        assert_eq!(median_secs(&[30]), Some(30));
        assert_eq!(median_secs(&[30, 10, 20]), Some(20));
        assert_eq!(median_secs(&[10, 20, 30, 40]), Some(25));
        // Odd averages round up rather than truncating toward zero.
        assert_eq!(median_secs(&[10, 11]), Some(11));
    }

    #[test]
    fn history_keeps_only_the_most_recent_samples() {
        let mut history = History::default();
        let key = History::key("me/repo", "CI");
        for i in 1..=(MAX_SAMPLES as u64 + 5) {
            history.record(&key, secs(i));
        }
        assert_eq!(history.sample_count(&key), MAX_SAMPLES);
        // The five oldest (1..=5) were evicted, so the window is 6..=25.
        assert_eq!(history.median(&key), Some(secs(16)));
    }

    #[test]
    fn zero_length_runs_are_not_recorded() {
        let mut history = History::default();
        let key = History::key("me/repo", "CI");
        history.record(&key, Duration::ZERO);
        assert_eq!(history.sample_count(&key), 0);
        assert_eq!(history.median(&key), None);
    }

    #[test]
    fn progress_is_indeterminate_without_history() {
        assert_eq!(estimate(secs(30), None), Progress::Indeterminate);
        assert_eq!(estimate(secs(30), Some(Duration::ZERO)), Progress::Indeterminate);
    }

    #[test]
    fn progress_is_elapsed_over_median() {
        assert_eq!(estimate(secs(0), Some(secs(100))), Progress::Fraction(0.0));
        assert_eq!(estimate(secs(50), Some(secs(100))), Progress::Fraction(0.5));
        assert_eq!(estimate(secs(95), Some(secs(100))), Progress::Fraction(0.95));
    }

    #[test]
    fn progress_caps_at_95_percent_for_overrunning_runs() {
        assert_eq!(estimate(secs(200), Some(secs(100))), Progress::Fraction(0.95));
        assert_eq!(estimate(secs(100_000), Some(secs(1))), Progress::Fraction(0.95));
    }

    #[test]
    fn history_round_trips_through_json() {
        let mut history = History::default();
        let key = History::key("me/repo", "Deploy");
        history.record(&key, secs(120));
        history.record(&key, secs(140));

        let json = serde_json::to_string(&history).expect("serialize");
        let restored: History = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.median(&key), Some(secs(130)));
    }

    #[test]
    fn unknown_and_empty_json_loads_as_empty_history() {
        let restored: History = serde_json::from_str("{}").expect("deserialize");
        assert_eq!(restored.median("anything"), None);
    }
}
