//! Capture statistics for `camera_probe` (C1c): frames per second, jitter and
//! lost frames, from the frames' host timestamps.
//!
//! Pure numbers in, numbers out, tested on every platform. "Lost" means a gap in
//! the camera's own delivery (an interval of ≥ 1.5 nominal frame periods, counted
//! as the whole frames missing), as opposed to frames dropped by our queue or by
//! the OS, which the capture's own counters report.

/// What the timestamps say about one capture.
#[derive(Clone, Debug, PartialEq)]
pub struct IntervalStats {
    pub frames: u64,
    /// Seconds from the first to the last timestamp.
    pub span_s: f64,
    /// Measured delivery rate: `(frames - 1) / span`.
    pub fps: f64,
    pub mean_ms: f64,
    /// Standard deviation of the frame interval (the jitter).
    pub jitter_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub p99_ms: f64,
    /// Whole frames missing in gaps of ≥ 1.5 nominal periods.
    pub lost: u64,
    /// Timestamps that were not strictly after their predecessor.
    pub non_monotonic: u64,
}

/// Compute [`IntervalStats`] from host timestamps in nanoseconds, in delivery
/// order. `nominal_fps` is the format that was asked for (the yardstick for
/// "lost"). `None` with fewer than two timestamps.
pub fn interval_stats(host_ns: &[u64], nominal_fps: f64) -> Option<IntervalStats> {
    if host_ns.len() < 2 {
        return None;
    }
    let mut non_monotonic = 0u64;
    let mut intervals_ms = Vec::with_capacity(host_ns.len() - 1);
    for w in host_ns.windows(2) {
        if w[1] <= w[0] {
            non_monotonic += 1;
            continue;
        }
        intervals_ms.push((w[1] - w[0]) as f64 / 1e6);
    }
    if intervals_ms.is_empty() {
        return None;
    }
    let n = intervals_ms.len() as f64;
    let mean = intervals_ms.iter().sum::<f64>() / n;
    let var = intervals_ms.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    let mut sorted = intervals_ms.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let p99 = sorted[((sorted.len() as f64 * 0.99).ceil() as usize).clamp(1, sorted.len()) - 1];
    let period_ms = if nominal_fps > 0.0 {
        1000.0 / nominal_fps
    } else {
        mean
    };
    let lost = intervals_ms
        .iter()
        .filter(|&&x| x >= period_ms * 1.5)
        .map(|&x| (x / period_ms).round() as u64 - 1)
        .sum();
    let span_s = (host_ns[host_ns.len() - 1].saturating_sub(host_ns[0])) as f64 / 1e9;
    Some(IntervalStats {
        frames: host_ns.len() as u64,
        span_s,
        fps: if span_s > 0.0 {
            (host_ns.len() - 1) as f64 / span_s
        } else {
            0.0
        },
        mean_ms: mean,
        jitter_ms: var.sqrt(),
        min_ms: sorted[0],
        max_ms: sorted[sorted.len() - 1],
        p99_ms: p99,
        lost,
        non_monotonic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(intervals_ms: &[f64]) -> Vec<u64> {
        let mut t = 1_000_000_000u64;
        let mut out = vec![t];
        for &i in intervals_ms {
            t += (i * 1e6) as u64;
            out.push(t);
        }
        out
    }

    #[test]
    fn a_steady_30_fps_has_no_jitter_and_no_loss() {
        let s = interval_stats(&ts(&[1000.0 / 30.0; 300]), 30.0).unwrap();
        assert_eq!(s.frames, 301);
        assert!((s.fps - 30.0).abs() < 0.01, "{}", s.fps);
        assert!(s.jitter_ms < 0.001);
        assert_eq!(s.lost, 0);
        assert_eq!(s.non_monotonic, 0);
    }

    #[test]
    fn a_gap_counts_the_whole_frames_missing() {
        // One 100 ms gap at 30 fps (33.3 ms) = 3 periods = 2 frames lost.
        let mut iv = vec![1000.0 / 30.0; 20];
        iv[10] = 100.0;
        let s = interval_stats(&ts(&iv), 30.0).unwrap();
        assert_eq!(s.lost, 2);
        assert!((s.max_ms - 100.0).abs() < 0.01);
        assert!(s.jitter_ms > 10.0);
    }

    #[test]
    fn ordinary_jitter_is_not_loss() {
        // 25–42 ms intervals at a nominal 30 fps: wobble, nothing missing.
        let iv: Vec<f64> = (0..100)
            .map(|i| if i % 2 == 0 { 25.0 } else { 41.6 })
            .collect();
        let s = interval_stats(&ts(&iv), 30.0).unwrap();
        assert_eq!(s.lost, 0);
        assert!(s.jitter_ms > 5.0);
    }

    #[test]
    fn a_camera_below_nominal_shows_its_real_rate() {
        // Dim room: 15 fps delivered against 30 requested = each interval is
        // two periods, so one frame "lost" per interval, and the fps says 15.
        let s = interval_stats(&ts(&[1000.0 / 15.0; 150]), 30.0).unwrap();
        assert!((s.fps - 15.0).abs() < 0.01);
        assert_eq!(s.lost, 150);
    }

    #[test]
    fn backwards_timestamps_are_counted_and_skipped() {
        let s = interval_stats(&[100, 200_000_000, 150_000_000, 300_000_000], 30.0).unwrap();
        assert_eq!(s.non_monotonic, 1);
    }

    #[test]
    fn too_few_frames_have_no_statistics() {
        assert!(interval_stats(&[], 30.0).is_none());
        assert!(interval_stats(&[5], 30.0).is_none());
        assert!(interval_stats(&[5, 5], 30.0).is_none());
    }
}
