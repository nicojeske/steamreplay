//! Finding loud parts (laughs, shouts) in the level-0 RMS envelope.

use serde::{Deserialize, Serialize};
use specta::Type;

use super::audio::{BIN_MS, Bin};

/// Floor for the dB conversion (−100 dB).
const DB_FLOOR_RMS: f32 = 1e-5;
/// Bins quieter than this (−80 dB) are digital silence and do not count towards the baseline.
const SILENCE_RMS: f32 = 1e-4;
/// Histogram range and resolution of the sliding median.
const HIST_MIN_DB: f32 = -100.0;
const HIST_STEP_DB: f32 = 0.5;
const HIST_BUCKETS: usize = 200;

/// All tuning constants of the detector. The defaults are the spec's starting values
/// (not yet tuned on real sessions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SpikeParams {
    /// Window of the short-term loudness mean.
    pub short_ms: u32,
    /// Window of the rolling-median baseline.
    #[specta(type = specta_typescript::Number)]
    pub baseline_s: f32,
    /// How far above the baseline the short-term loudness must be.
    #[specta(type = specta_typescript::Number)]
    pub threshold_db: f32,
    pub min_len_ms: u32,
    #[specta(type = specta_typescript::Number)]
    pub merge_gap_s: f32,
    #[specta(type = specta_typescript::Number)]
    pub pad_s: f32,
    pub max_results: u32,
}

impl Default for SpikeParams {
    fn default() -> Self {
        Self {
            short_ms: 250,
            baseline_s: 30.0,
            threshold_db: 10.0,
            min_len_ms: 400,
            merge_gap_s: 5.0,
            pad_s: 1.5,
            max_results: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Spike {
    #[specta(type = specta_typescript::Number)]
    pub start_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub end_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub peak_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub peak_excess_db: f32,
    #[specta(type = specta_typescript::Number)]
    pub score: f32,
}

fn bins_for_ms(ms: f64) -> usize {
    (ms / f64::from(BIN_MS)).round().max(1.0) as usize
}

/// Mean of `v` over a centred window of `w` values (shorter at the edges).
fn centred_mean(v: &[f32], w: usize) -> Vec<f32> {
    let mut prefix = Vec::with_capacity(v.len() + 1);
    prefix.push(0.0f64);
    for x in v {
        prefix.push(prefix.last().unwrap() + f64::from(*x));
    }
    let before = w / 2;
    let after = w - 1 - before;
    (0..v.len())
        .map(|i| {
            let lo = i.saturating_sub(before);
            let hi = (i + after + 1).min(v.len());
            ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32
        })
        .collect()
}

fn bucket(db: f32) -> usize {
    (((db - HIST_MIN_DB) / HIST_STEP_DB).floor().max(0.0) as usize).min(HIST_BUCKETS - 1)
}

/// Median of a 0.5 dB histogram, as the bucket centre.
fn hist_median(hist: &[u32; HIST_BUCKETS], count: u32) -> f32 {
    let target = count.div_ceil(2);
    let mut seen = 0;
    for (i, c) in hist.iter().enumerate() {
        seen += c;
        if seen >= target {
            return HIST_MIN_DB + (i as f32 + 0.5) * HIST_STEP_DB;
        }
    }
    HIST_MIN_DB
}

/// Rolling median of `v` over a centred window of `w` values, skipping values where
/// `include` is false. O(n · buckets). `None` where the window has no included values.
pub fn sliding_median(v: &[f32], include: &[bool], w: usize) -> Vec<Option<f32>> {
    let mut hist = [0u32; HIST_BUCKETS];
    let mut count = 0u32;
    let before = w / 2;
    let after = w - 1 - before;
    let mut lo = 0; // window is [lo, hi)
    let mut hi = 0;
    let mut out = Vec::with_capacity(v.len());
    for i in 0..v.len() {
        let want_hi = (i + after + 1).min(v.len());
        while hi < want_hi {
            if include[hi] {
                hist[bucket(v[hi])] += 1;
                count += 1;
            }
            hi += 1;
        }
        let want_lo = i.saturating_sub(before);
        while lo < want_lo {
            if include[lo] {
                hist[bucket(v[lo])] -= 1;
                count -= 1;
            }
            lo += 1;
        }
        out.push((count > 0).then(|| hist_median(&hist, count)));
    }
    out
}

struct Region {
    start: usize,
    end: usize, // exclusive
    peak: usize,
    peak_excess: f32,
    active_bins: usize,
}

pub fn detect(level0: &[Bin], p: &SpikeParams) -> Vec<Spike> {
    let n = level0.len();
    if n == 0 {
        return Vec::new();
    }
    let bin_s = f64::from(BIN_MS) / 1000.0;
    let db: Vec<f32> = level0.iter().map(|b| 20.0 * b[2].max(DB_FLOOR_RMS).log10()).collect();
    let include: Vec<bool> = level0.iter().map(|b| b[2] >= SILENCE_RMS).collect();
    let st = centred_mean(&db, bins_for_ms(f64::from(p.short_ms)));
    let base = sliding_median(&st, &include, bins_for_ms(f64::from(p.baseline_s) * 1000.0));

    // Runs above the threshold.
    let min_len = bins_for_ms(f64::from(p.min_len_ms));
    let mut regions: Vec<Region> = Vec::new();
    let mut i = 0;
    while i < n {
        let excess = |j: usize| base[j].filter(|_| include[j]).map(|b| st[j] - b);
        if excess(i).is_some_and(|e| e > p.threshold_db) {
            let start = i;
            let (mut peak, mut peak_excess) = (i, f32::NEG_INFINITY);
            while i < n
                && let Some(e) = excess(i).filter(|e| *e > p.threshold_db)
            {
                if e > peak_excess {
                    (peak, peak_excess) = (i, e);
                }
                i += 1;
            }
            if i - start >= min_len {
                regions.push(Region { start, end: i, peak, peak_excess, active_bins: i - start });
            }
        } else {
            i += 1;
        }
    }

    // Merge regions that are close together.
    let merge_gap = bins_for_ms(f64::from(p.merge_gap_s) * 1000.0);
    let mut merged: Vec<Region> = Vec::new();
    for r in regions {
        match merged.last_mut() {
            Some(m) if r.start - m.end < merge_gap => {
                m.end = r.end;
                m.active_bins += r.active_bins;
                if r.peak_excess > m.peak_excess {
                    (m.peak, m.peak_excess) = (r.peak, r.peak_excess);
                }
            }
            _ => merged.push(r),
        }
    }

    let duration_s = n as f64 * bin_s;
    let pad = f64::from(p.pad_s);
    let mut spikes: Vec<Spike> = merged
        .into_iter()
        .map(|r| {
            let active_s = r.active_bins as f32 * bin_s as f32;
            Spike {
                start_s: (r.start as f64 * bin_s - pad).max(0.0),
                end_s: (r.end as f64 * bin_s + pad).min(duration_s),
                peak_s: (r.peak as f64 + 0.5) * bin_s,
                peak_excess_db: r.peak_excess,
                score: r.peak_excess + 3.0 * (1.0 + active_s).log2(),
            }
        })
        .collect();
    spikes.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.start_s.total_cmp(&b.start_s)));
    spikes.truncate(p.max_results as usize);
    spikes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny deterministic PRNG (xorshift), so tests need no extra crate.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    fn db_to_rms(db: f32) -> f32 {
        10f32.powf(db / 20.0)
    }

    /// −40 dB noise (±1 dB jitter) with bursts at −20 dB: `(start_s, len_s)`.
    fn signal(duration_s: f64, bursts: &[(f64, f64)]) -> Vec<Bin> {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let n = (duration_s * 100.0) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / 100.0;
                let loud = bursts.iter().any(|&(s, l)| t >= s && t < s + l);
                let db = if loud { -20.0 } else { -40.0 } + (rng.next() - 0.5) * 2.0;
                let r = db_to_rms(db);
                [-r * 1.4, r * 1.4, r]
            })
            .collect()
    }

    #[test]
    fn bursts_found_at_their_times() {
        let p = SpikeParams::default();
        let spikes = detect(&signal(300.0, &[(60.0, 1.0), (150.0, 1.0), (240.0, 1.0)]), &p);
        assert_eq!(spikes.len(), 3);
        let mut starts: Vec<_> = spikes.iter().map(|s| s.start_s).collect();
        starts.sort_by(f64::total_cmp);
        for (s, expected) in starts.iter().zip([60.0, 150.0, 240.0]) {
            assert!((s - (expected - 1.5)).abs() < 0.2, "start {s} vs {expected} − pad");
        }
        for s in &spikes {
            assert!((s.end_s - s.start_s - (1.0 + 3.0)).abs() < 0.3, "{s:?}");
            assert!(s.peak_excess_db > 15.0 && s.peak_excess_db < 22.0, "{s:?}");
        }
    }

    #[test]
    fn close_bursts_merge_short_burst_ignored() {
        let p = SpikeParams::default();
        let spikes = detect(&signal(300.0, &[(100.0, 1.0), (104.0, 1.0), (200.0, 0.2)]), &p);
        assert_eq!(spikes.len(), 1, "{spikes:?}");
        let s = &spikes[0];
        assert!((s.start_s - 98.5).abs() < 0.2 && (s.end_s - 106.5).abs() < 0.2, "{s:?}");
        // Merged length counts only the active parts.
        let single = detect(&signal(300.0, &[(100.0, 1.0)]), &p);
        assert!(s.score > single[0].score);
    }

    #[test]
    fn pad_clamped_and_sorted() {
        let p = SpikeParams { max_results: 2, ..SpikeParams::default() };
        let mut sig = signal(100.0, &[(0.5, 1.0), (50.0, 3.0), (98.5, 1.5)]);
        // Make the middle burst louder so it ranks first.
        for b in &mut sig[5000..5300] {
            *b = [-0.5, 0.5, db_to_rms(-10.0)];
        }
        let spikes = detect(&sig, &p);
        assert_eq!(spikes.len(), 2);
        assert!((spikes[0].start_s - 48.5).abs() < 0.2);
        assert!(spikes[0].score >= spikes[1].score);
        let all = detect(&sig, &SpikeParams::default());
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|s| s.start_s >= 0.0 && s.end_s <= 100.0));
    }

    #[test]
    fn digital_silence_gives_nothing() {
        assert!(detect(&vec![[0.0; 3]; 10_000], &SpikeParams::default()).is_empty());
        assert!(detect(&[], &SpikeParams::default()).is_empty());
        // Speech after a long silent stretch is compared with the speech-only baseline.
        let mut sig = vec![[0.0f32; 3]; 6000];
        sig.extend(signal(60.0, &[]));
        assert!(detect(&sig, &SpikeParams::default()).is_empty());
    }

    #[test]
    fn sliding_median_matches_naive() {
        let mut rng = Rng(12345);
        let v: Vec<f32> = (0..3000).map(|_| -100.0 + rng.next() * 100.0).collect();
        let include: Vec<bool> = (0..3000).map(|_| rng.next() > 0.2).collect();
        for w in [1, 2, 25, 301] {
            let fast = sliding_median(&v, &include, w);
            let before = w / 2;
            let after = w - 1 - before;
            for (i, got) in fast.iter().enumerate() {
                let lo = i.saturating_sub(before);
                let hi = (i + after + 1).min(v.len());
                let mut win: Vec<f32> = (lo..hi).filter(|&j| include[j]).map(|j| v[j]).collect();
                win.sort_by(f32::total_cmp);
                match *got {
                    None => assert!(win.is_empty()),
                    Some(m) => {
                        // Lower median, within one 0.5 dB bucket.
                        let naive = win[win.len().div_ceil(2) - 1];
                        assert!((m - naive).abs() <= HIST_STEP_DB, "w={w} i={i}: {m} vs {naive}");
                    }
                }
            }
        }
    }

    #[test]
    fn centred_mean_edges() {
        assert_eq!(centred_mean(&[1.0, 2.0, 3.0, 4.0], 3), vec![1.5, 2.0, 3.0, 3.5]);
        assert_eq!(centred_mean(&[1.0, 2.0], 1), vec![1.0, 2.0]);
    }
}
