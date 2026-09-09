//! Approximate RSSI ranging using a fixed -59 dBm at one metre and n = 2.
//!
//! These assumptions are not calibration. Obstructions and antenna orientation
//! can introduce systematic errors that filtering cannot remove.

const STALE_MS: u64 = 2_000;

/// Allocation-free median-of-five and time-based RSSI smoothing state.
#[derive(Clone, Copy, Debug)]
pub struct DistanceEstimator {
    samples: [i8; 5],
    count: usize,
    next: usize,
    filtered: f32,
    last_ms: Option<u64>,
}

impl Default for DistanceEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl DistanceEstimator {
    /// Creates an estimator that needs five valid samples before reporting.
    pub const fn new() -> Self {
        Self {
            samples: [0; 5],
            count: 0,
            next: 0,
            filtered: 0.0,
            last_ms: None,
        }
    }

    /// Records an advertising RSSI in dBm at a monotonic millisecond timestamp.
    /// Invalid HCI RSSI values do not update the estimate or its freshness.
    /// A gap over two seconds resets the filter and its warm-up state.
    pub fn record(&mut self, rssi: i8, now_ms: u64) {
        if !(-127..=20).contains(&rssi) {
            return;
        }
        if self
            .last_ms
            .is_some_and(|last| now_ms < last || now_ms - last > STALE_MS)
        {
            *self = Self::new();
        }
        self.samples[self.next] = rssi;
        self.next = (self.next + 1) % self.samples.len();
        self.count = (self.count + 1).min(self.samples.len());
        let mut sorted = self.samples;
        sorted[..self.count].sort_unstable();
        let median = f32::from(sorted[self.count / 2]);
        if self.count < 5 || self.last_ms.is_none() {
            self.filtered = median;
        } else {
            let elapsed = now_ms.saturating_sub(self.last_ms.unwrap_or(now_ms)) as f32;
            let alpha = elapsed / (1_000.0 + elapsed);
            self.filtered += alpha * (median - self.filtered);
        }
        self.last_ms = Some(now_ms);
    }

    /// Returns estimated metres, or `None` during warm-up or after two seconds
    /// without valid samples. The result is not an accuracy guarantee.
    pub fn meters(&self, now_ms: u64) -> Option<f32> {
        let last = self.last_ms?;
        if self.count < 5 || now_ms < last || now_ms - last > STALE_MS {
            return None;
        }
        Some(libm::powf(10.0, (-59.0 - self.filtered) / 20.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled(rssi: i8) -> DistanceEstimator {
        let mut estimator = DistanceEstimator::new();
        for t in 0..100 {
            estimator.record(rssi, t * 20);
        }
        estimator
    }

    #[test]
    fn reference_and_distance_decades() {
        for (rssi, expected) in [(-39, 0.1), (-59, 1.0), (-79, 10.0), (-99, 100.0)] {
            assert!((settled(rssi).meters(2_000).unwrap() - expected).abs() < 0.001);
        }
    }

    #[test]
    fn warmup_expiry_and_reacquisition() {
        let mut estimator = DistanceEstimator::new();
        for t in 0..4 {
            estimator.record(-59, t * 20);
            assert_eq!(estimator.meters(t * 20), None);
        }
        estimator.record(-59, 80);
        assert_eq!(estimator.meters(80), Some(1.0));
        assert_eq!(estimator.meters(2_081), None);
        estimator.record(-79, 3_000);
        assert_eq!(estimator.meters(3_000), None);
        for t in 1..5 {
            estimator.record(-79, 3_000 + t * 20);
        }
        assert_eq!(estimator.meters(3_080), Some(10.0));
    }

    #[test]
    fn invalid_samples_do_not_refresh_and_spikes_are_rejected() {
        let mut estimator = settled(-59);
        estimator.record(-100, 2_000);
        assert_eq!(estimator.meters(2_000), Some(1.0));
        estimator.record(127, 3_000);
        estimator.record(-128, 3_001);
        estimator.record(21, 3_002);
        assert_eq!(estimator.meters(4_001), None);
    }

    #[test]
    fn sustained_movement_converges() {
        let mut estimator = settled(-59);
        for t in 100..600 {
            estimator.record(-79, t * 20);
        }
        assert!((estimator.meters(12_000).unwrap() - 10.0).abs() < 0.02);
    }
}
