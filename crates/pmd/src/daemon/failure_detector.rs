use std::collections::VecDeque;
use std::time::Instant;

/// Phi Accrual Failure Detector (Hayashibara et al., 2004).
///
/// Maintains a sliding window of heartbeat inter-arrival times and computes
/// a suspicion level φ (phi). A peer is considered unreachable when φ exceeds
/// a configurable threshold.
///
/// The phi value is derived from the cumulative distribution function of a
/// normal distribution fitted to the observed inter-arrival times:
///   φ = -log10(1 - CDF(t_now - t_last))
///
/// Higher φ means higher suspicion that the peer has failed.
pub struct PhiAccrualDetector {
    /// Sliding window of heartbeat inter-arrival times in milliseconds.
    intervals: VecDeque<f64>,
    /// Maximum number of samples to keep.
    max_window_size: usize,
    /// Timestamp of the last heartbeat received.
    last_heartbeat: Option<Instant>,
    /// Minimum standard deviation (ms) to prevent division-by-zero when
    /// inter-arrival times are perfectly uniform.
    min_std_deviation_ms: f64,
}

impl PhiAccrualDetector {
    /// Create a new detector.
    ///
    /// * `max_window_size` — maximum number of inter-arrival samples to retain
    /// * `min_std_deviation_ms` — floor for the standard deviation (ms)
    pub fn new(max_window_size: usize, min_std_deviation_ms: f64) -> Self {
        Self {
            intervals: VecDeque::with_capacity(max_window_size),
            max_window_size,
            last_heartbeat: None,
            min_std_deviation_ms,
        }
    }

    /// Record a heartbeat arrival. Should be called each time a `HeartbeatAck`
    /// is received from the peer.
    pub fn heartbeat(&mut self) {
        self.heartbeat_at(Instant::now());
    }

    /// Record a heartbeat arrival at a specific instant (useful for testing).
    pub fn heartbeat_at(&mut self, now: Instant) {
        if let Some(last) = self.last_heartbeat {
            let interval_ms = now.duration_since(last).as_secs_f64() * 1000.0;
            if self.intervals.len() >= self.max_window_size {
                self.intervals.pop_front();
            }
            self.intervals.push_back(interval_ms);
        }
        self.last_heartbeat = Some(now);
    }

    /// Compute the current φ (phi) value.
    ///
    /// Returns `0.0` if no heartbeat has been received yet or if the window
    /// has fewer than 2 samples (not enough data to form a distribution).
    pub fn phi(&self) -> f64 {
        self.phi_at(Instant::now())
    }

    /// Compute φ at a specific instant (useful for testing).
    pub fn phi_at(&self, now: Instant) -> f64 {
        let last = match self.last_heartbeat {
            Some(t) => t,
            None => return 0.0,
        };

        if self.intervals.len() < 2 {
            return 0.0;
        }

        let elapsed_ms = now.duration_since(last).as_secs_f64() * 1000.0;
        let mean = self.mean();
        let std_dev = self.std_deviation().max(self.min_std_deviation_ms);

        // P(X <= elapsed) using the normal CDF approximation
        let y = (elapsed_ms - mean) / std_dev;
        let p = normal_cdf(y);

        // φ = -log10(1 - P) — the higher the elapsed time relative to
        // the distribution, the higher φ.
        let one_minus_p = (1.0 - p).max(1e-100); // avoid log10(0)
        -one_minus_p.log10()
    }

    /// Check whether the peer should be considered unreachable.
    #[allow(dead_code)] // convenience API, used from tests and future callers
    pub fn is_available(&self, threshold: f64) -> bool {
        self.phi() < threshold
    }

    fn mean(&self) -> f64 {
        if self.intervals.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.intervals.iter().sum();
        sum / self.intervals.len() as f64
    }

    fn std_deviation(&self) -> f64 {
        if self.intervals.len() < 2 {
            return 0.0;
        }
        let mean = self.mean();
        let variance: f64 = self
            .intervals
            .iter()
            .map(|x| {
                let diff = x - mean;
                diff * diff
            })
            .sum::<f64>()
            / self.intervals.len() as f64;
        variance.sqrt()
    }
}

/// Approximation of the standard normal CDF using the logistic function.
/// Accurate to ~0.01, which is sufficient for failure detection.
fn normal_cdf(x: f64) -> f64 {
    1.0 / (1.0 + (-1.5976 * x).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    #[test]
    fn test_phi_zero_without_heartbeats() {
        let detector = PhiAccrualDetector::new(100, 500.0);
        assert_eq!(detector.phi(), 0.0);
    }

    #[test]
    fn test_phi_zero_with_single_heartbeat() {
        let mut detector = PhiAccrualDetector::new(100, 500.0);
        detector.heartbeat();
        // Only one heartbeat, no intervals → phi = 0
        assert_eq!(detector.phi(), 0.0);
    }

    #[test]
    fn test_phi_zero_with_two_heartbeats_no_delay() {
        let mut detector = PhiAccrualDetector::new(100, 500.0);
        let now = Instant::now();
        detector.heartbeat_at(now);
        detector.heartbeat_at(now + Duration::from_secs(1));
        // 2 heartbeats → 1 interval sample, need >=2
        assert_eq!(detector.phi_at(now + Duration::from_secs(1)), 0.0);
    }

    #[test]
    fn test_phi_low_when_on_schedule() {
        let mut detector = PhiAccrualDetector::new(100, 500.0);
        let start = Instant::now();
        // Simulate regular heartbeats every 1s
        for i in 0..10 {
            detector.heartbeat_at(start + Duration::from_secs(i));
        }
        // Check phi right after the last heartbeat (0ms elapsed)
        let phi = detector.phi_at(start + Duration::from_secs(9));
        assert!(phi < 1.0, "phi should be low when on schedule, got {phi}");
    }

    #[test]
    fn test_phi_increases_with_delay() {
        let mut detector = PhiAccrualDetector::new(100, 500.0);
        let start = Instant::now();
        // Regular heartbeats every 1s
        for i in 0..10 {
            detector.heartbeat_at(start + Duration::from_secs(i));
        }
        // Wait 5s after last heartbeat (5x the normal interval)
        let phi_5s = detector.phi_at(start + Duration::from_secs(14));
        // Wait 10s after last heartbeat
        let phi_10s = detector.phi_at(start + Duration::from_secs(19));

        assert!(
            phi_10s > phi_5s,
            "phi should increase with more delay: {phi_5s} vs {phi_10s}"
        );
    }

    #[test]
    fn test_phi_exceeds_threshold_on_long_delay() {
        let mut detector = PhiAccrualDetector::new(100, 500.0);
        let start = Instant::now();
        // Regular heartbeats every 1s
        for i in 0..20 {
            detector.heartbeat_at(start + Duration::from_secs(i));
        }
        // Very long delay: 30s after last heartbeat with 1s interval
        let phi = detector.phi_at(start + Duration::from_secs(49));
        assert!(
            phi > 8.0,
            "phi should exceed default threshold 8.0 on 30s delay, got {phi}"
        );
    }

    #[test]
    fn test_is_available() {
        let mut detector = PhiAccrualDetector::new(100, 500.0);
        let start = Instant::now();
        for i in 0..10 {
            detector.heartbeat_at(start + Duration::from_secs(i));
        }
        // Right after last heartbeat → available
        assert!(detector.is_available(8.0));
    }

    #[test]
    fn test_window_size_limit() {
        let mut detector = PhiAccrualDetector::new(5, 500.0);
        let start = Instant::now();
        // Add 10 heartbeats → only 5 intervals retained
        for i in 0..10 {
            detector.heartbeat_at(start + Duration::from_millis(i * 1000));
        }
        assert_eq!(detector.intervals.len(), 5);
    }

    #[test]
    fn test_normal_cdf_symmetry() {
        // CDF(0) ≈ 0.5
        let mid = normal_cdf(0.0);
        assert!((mid - 0.5).abs() < 0.01, "CDF(0) should be ~0.5, got {mid}");

        // CDF(x) + CDF(-x) ≈ 1
        let pos = normal_cdf(2.0);
        let neg = normal_cdf(-2.0);
        assert!(
            (pos + neg - 1.0).abs() < 0.02,
            "CDF(x)+CDF(-x) ≈ 1, got {pos}+{neg}"
        );
    }

    #[test]
    fn test_min_std_deviation_prevents_extreme_phi() {
        let mut detector = PhiAccrualDetector::new(100, 500.0);
        let start = Instant::now();
        // Perfectly uniform heartbeats → std_dev = 0, but min kicks in
        for i in 0..10 {
            detector.heartbeat_at(start + Duration::from_millis(i * 1000));
        }
        // 2s after last heartbeat — with min_std_deviation, phi stays bounded
        let phi = detector.phi_at(start + Duration::from_millis(11000));
        assert!(phi.is_finite(), "phi should be finite, got {phi}");
        assert!(phi > 0.0, "phi should be positive, got {phi}");
    }
}
