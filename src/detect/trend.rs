//! Steady growth of a metric across a window, judged by a least-squares line
//! through the process's history. Shared by the leak detectors.

use crate::tracker::{Sample, TrackedProcess};

/// Too few points make any line look straight.
const MIN_POINTS: usize = 5;

#[derive(Debug, Clone, Copy)]
pub struct Trend {
    /// Growth across the whole window, according to the fitted line.
    pub growth: f64,
    /// Coefficient of determination: 1.0 means the points sit on the line.
    pub r2: f64,
}

impl Trend {
    /// Fits `metric` over the last `window_ms` of history. `None` until the
    /// history covers the whole window, or while the metric doesn't change.
    pub fn of(
        process: &TrackedProcess,
        window_ms: u64,
        metric: impl Fn(&Sample) -> f64 + Copy,
    ) -> Option<Self> {
        let cutoff = process.latest.at_ms.checked_sub(window_ms)?;
        if process.history().next()?.at_ms > cutoff {
            return None;
        }
        let points = process
            .history()
            .filter(move |s| s.at_ms >= cutoff)
            .map(move |s| (s.at_ms as f64 / 1000.0, metric(s)));
        let fit = LinearFit::of(points)?;
        Some(Self {
            growth: fit.slope * (window_ms as f64 / 1000.0),
            r2: fit.r2,
        })
    }

    /// Grew by at least `min_growth`, and steadily rather than in swings.
    pub fn is_steady_rise(&self, min_growth: f64, min_r2: f64) -> bool {
        self.growth >= min_growth && self.r2 >= min_r2
    }
}

/// Least-squares line through a set of points.
#[derive(Debug, Clone, Copy)]
struct LinearFit {
    slope: f64,
    r2: f64,
}

impl LinearFit {
    /// `None` with fewer than [`MIN_POINTS`] points, or if x or y never change.
    fn of<I>(points: I) -> Option<Self>
    where
        I: Iterator<Item = (f64, f64)> + Clone,
    {
        let (mut n, mut sum_x, mut sum_y) = (0usize, 0.0, 0.0);
        for (x, y) in points.clone() {
            n += 1;
            sum_x += x;
            sum_y += y;
        }
        if n < MIN_POINTS {
            return None;
        }
        let (mean_x, mean_y) = (sum_x / n as f64, sum_y / n as f64);
        // Second pass on centred values: numerically stable even for byte counts.
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for (x, y) in points {
            let (dx, dy) = (x - mean_x, y - mean_y);
            sxx += dx * dx;
            syy += dy * dy;
            sxy += dx * dy;
        }
        if sxx == 0.0 || syy == 0.0 {
            return None;
        }
        Some(Self {
            slope: sxy / sxx,
            r2: sxy * sxy / (sxx * syy),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_of_exact_line() {
        let fit = LinearFit::of((0..10).map(|x| (f64::from(x), 3.0 * f64::from(x) + 7.0))).unwrap();
        assert!((fit.slope - 3.0).abs() < 1e-9);
        assert!((fit.r2 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn fit_rejects_flat_or_tiny_input() {
        assert!(LinearFit::of((0..10).map(|x| (f64::from(x), 5.0))).is_none());
        assert!(LinearFit::of((0..3).map(|x| (f64::from(x), f64::from(x)))).is_none());
    }
}
