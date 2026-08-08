//! Audio analysis — the frame level, pure (P2a).
//!
//! Ported from the Electron `src/main/audio-analysis.ts`. It classifies every
//! 100 ms frame of decoded mono-16 kHz PCM as speech/music/silence/mixed/unknown
//! using a feature-based heuristic, smooths the type stream, groups runs into
//! segments and merges sub-5 s segments.
//!
//! Everything here is a deterministic function of an in-memory PCM buffer (or
//! pre-extracted frames), so the whole classifier is unit-testable without
//! ffmpeg. The `src-tauri` shell decodes the file to f32 PCM and feeds frames
//! in; this module owns the maths.
//!
//! It does NOT own the numbers. Every threshold this module compares against
//! lives in [`crate::tuning`], with what moving it does to a real recording;
//! this module re-exports the ones callers already reach for by their old path.
//! A frame-level threshold changed here moves every segment boundary in every
//! recording, so read that file's header before touching one.
//!
//! This module stops at classified [`AnalysisSegment`]s. Picking a sermon out of
//! them — and everything above it — is [`crate::detect`]. It used to be BOTH,
//! with a second, drifted copy of the pick in the review path; see that module's
//! header.
//!
//! ## The seam
//!
//! [`FrameScorer`] is the boundary a voice-activity-detection model replaces.
//! [`HeuristicScorer`] is the hand-rolled score this app has always shipped;
//! [`crate::shadow::VadScorer`] is the model. A scorer is handed a
//! [`ScoringInput`] — the PCM *and* the derived frames — because a model needs
//! samples and the four features here cannot be inverted back into them.

// ── Tuning constants ────────────────────────────────────────────────────────
//
// DEFINED IN [`crate::tuning`], not here. Re-exported so every path that ever
// worked (`audio_analysis::SILENCE_DB`, and the dozens of
// `use ...audio_analysis::{FRAME_MS, SAMPLE_RATE}` across the workspace) still
// resolves, while there is exactly ONE definition of each value — and one place
// that documents what moving it does to a real recording. Add a threshold there,
// never here.
pub use crate::tuning::{
    FFT_SIZE, FRAME_MS, FRAME_SAMPLES, MIN_SEGMENT_SEC, SAMPLE_RATE, SILENCE_DB, SMOOTH_HALF_WIN,
};

// Not re-exported: these are used only inside this module's classifier, and
// their one public home is `crate::tuning`.
use crate::tuning::{
    CONF_MIXED, CONF_MUSIC_ALL_THREE, CONF_MUSIC_SOLID, CONF_SPEECH_ALL_FOUR, CONF_SPEECH_SOLID,
    CONF_UNKNOWN, MERGE_MAX_PASSES, MUSIC_ENERGY_MIN_DB, MUSIC_FLUX_MAX, MUSIC_ZCR_MAX_PER_SEC,
    SILENCE_CONFIDENCE_FLOOR, SILENCE_CONFIDENCE_FULL_MARGIN_DB, SILENCE_CONFIDENCE_SPAN,
    SPEECH_CENTROID_MAX_HZ, SPEECH_CENTROID_MIN_HZ, SPEECH_ENERGY_MAX_DB, SPEECH_ENERGY_MIN_DB,
    SPEECH_FLUX_MIN, SPEECH_ZCR_MAX_PER_SEC, SPEECH_ZCR_MIN_PER_SEC,
};

/// The five content classes. Mirrors `SegmentType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentType {
    Silence,
    Speech,
    Music,
    Mixed,
    Unknown,
}

impl SegmentType {
    /// The default Norwegian label, mirroring the `LABELS` table.
    pub fn label(self) -> &'static str {
        match self {
            SegmentType::Speech => "Tale",
            SegmentType::Music => "Musikk",
            SegmentType::Silence => "Stillhet",
            SegmentType::Mixed => "Blandet",
            SegmentType::Unknown => "—",
        }
    }
}

/// Per-frame features. Mirrors `AnalysisFrame`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalysisFrame {
    pub start_sec: f64,
    pub rms_db: f64,
    pub zcr_per_sec: f64,
    pub spectral_centroid: f64,
    pub spectral_flux: f64,
}

/// A grouped, classified segment. Mirrors `AnalysisSegment`.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisSegment {
    pub start_sec: f64,
    pub end_sec: f64,
    pub duration_sec: f64,
    pub seg_type: SegmentType,
    pub confidence: f64,
    pub avg_rms_db: f64,
    pub label: String,
}

// ── FFT ─────────────────────────────────────────────────────────────────────

/// Iterative in-place Cooley-Tukey radix-2 FFT over parallel re/im buffers of
/// length N (power of two). Ports the JS `fft` exactly.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert_eq!(n, im.len(), "fft: re/im length mismatch");
    assert!(n >= 2 && (n & (n - 1)) == 0, "fft: size must be power of 2");

    // bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // butterflies
    let mut size = 2;
    while size <= n {
        let half = size >> 1;
        let table_step = -2.0 * std::f64::consts::PI / size as f64;
        let mut i = 0;
        while i < n {
            for k in 0..half {
                let angle = table_step * k as f64;
                let wr = angle.cos();
                let wi = angle.sin();
                let a_re = re[i + k];
                let a_im = im[i + k];
                let b_re = re[i + k + half];
                let b_im = im[i + k + half];
                let t_re = wr * b_re - wi * b_im;
                let t_im = wr * b_im + wi * b_re;
                re[i + k] = a_re + t_re;
                im[i + k] = a_im + t_im;
                re[i + k + half] = a_re - t_re;
                im[i + k + half] = a_im - t_im;
            }
            i += size;
        }
        size <<= 1;
    }
}

/// Hann window of `size` samples (`0.5*(1-cos(2πi/(size-1)))`). Ports `hannWindow`.
/// `size <= 1` would divide by `size-1 == 0` → NaN; a 1-sample window is just 1.0
/// (the conventional degenerate case), so guard it.
pub fn hann_window(size: usize) -> Vec<f64> {
    if size <= 1 {
        return vec![1.0; size];
    }
    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (size as f64 - 1.0)).cos()))
        .collect()
}

// ── Per-frame features ─────────────────────────────────────────────────────────

/// RMS energy in dBFS. Returns `-inf` for true silence. Ports `rmsDb`.
pub fn rms_db(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return f64::NEG_INFINITY;
    }
    let mut sum_sq = 0.0_f64;
    for &s in samples {
        sum_sq += (s as f64) * (s as f64);
    }
    let rms = (sum_sq / samples.len() as f64).sqrt();
    if rms <= 1e-12 {
        f64::NEG_INFINITY
    } else {
        20.0 * rms.log10()
    }
}

/// Zero-crossing rate per second. Ports `zcrPerSecond`.
pub fn zcr_per_second(samples: &[f32], sample_rate: u32) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let mut crossings = 0u64;
    let mut prev = samples[0];
    for &cur in &samples[1..] {
        if (prev >= 0.0 && cur < 0.0) || (prev < 0.0 && cur >= 0.0) {
            crossings += 1;
        }
        prev = cur;
    }
    let duration_sec = samples.len() as f64 / sample_rate as f64;
    crossings as f64 / duration_sec
}

/// Spectral centroid (Hz) + magnitude spectrum (len N/2+1) of a frame. Ports
/// `spectrum`: Hann-windowed, zero-padded to `FFT_SIZE`.
pub fn spectrum(samples: &[f32], sample_rate: u32) -> (f64, Vec<f64>) {
    let n = FFT_SIZE;
    let mut re = vec![0.0_f64; n];
    let mut im = vec![0.0_f64; n];
    let win = hann_window(samples.len());

    let cap = samples.len().min(n);
    for i in 0..cap {
        re[i] = samples[i] as f64 * win[i];
    }

    fft(&mut re, &mut im);

    let half = n >> 1;
    let mut mag = vec![0.0_f64; half + 1];
    let mut weighted_sum = 0.0;
    let mut total_mag = 0.0;
    let bin_hz = sample_rate as f64 / n as f64;
    for (k, m) in mag.iter_mut().enumerate() {
        *m = (re[k] * re[k] + im[k] * im[k]).sqrt();
        weighted_sum += k as f64 * bin_hz * *m;
        total_mag += *m;
    }
    let centroid = if total_mag > 1e-12 {
        weighted_sum / total_mag
    } else {
        0.0
    };
    (centroid, mag)
}

/// Normalised spectral flux: the L2 norm of the bin-wise difference between the
/// current and previous magnitude spectra, each first scaled to unit L2 norm.
///
/// This measures how much the spectral SHAPE changed between frames, and
/// nothing else. Because both spectra are unit-normalised before differencing,
/// a scalar gain applied to the signal cancels exactly — the same recording
/// 10 dB hotter produces the same flux, bit-for-bit up to float rounding. That
/// is the standard remedy for flux's level dependence in the literature: the
/// original speech/music discriminator this classifier descends from
/// (Scheirer & Slaney 1997) computes its spectral features on normalised
/// spectra, and Lerch (*An Introduction to Audio Content Analysis*, §5.3)
/// gives normalisation of the magnitude spectrum as the way to decouple flux
/// from level. The alternative — dividing raw flux by the current frame's
/// magnitude — is also level-invariant but asymmetric in time; the symmetric,
/// unit-spectrum form is used here.
///
/// Magnitudes are non-negative, so the value lives in `[0, √2]`: 0 for an
/// unchanged shape, √2 for two spectra with no overlapping energy at all.
///
/// **This is a deliberate departure from the Electron port.** The original
/// `spectralFlux` differenced raw magnitudes, which grow with level, so the
/// same sermon recorded ~10 dB hotter had ~3× the flux and the classifier's
/// flux thresholds meant a different thing in every differently-gained room.
/// [`crate::tuning::SPEECH_FLUX_MIN`] and [`crate::tuning::MUSIC_FLUX_MAX`]
/// are expressed on THIS scale; their provenance entries record the change and
/// the derivation of the new values.
///
/// Returns 0 when there is no previous frame (the file's first frame, as the
/// Electron port did) and when either spectrum is numerically zero — a unit
/// vector cannot be made from true digital silence, and the frame below the
/// silence cut never reaches the flux feature anyway.
pub fn spectral_flux(curr: &[f64], prev: Option<&[f64]>) -> f64 {
    let Some(prev) = prev else { return 0.0 };
    let l2 = |v: &[f64]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let (curr_norm, prev_norm) = (l2(curr), l2(prev));
    if curr_norm <= 1e-12 || prev_norm <= 1e-12 {
        return 0.0;
    }
    let n = curr.len().min(prev.len());
    let mut sum = 0.0;
    for i in 0..n {
        let d = curr[i] / curr_norm - prev[i] / prev_norm;
        sum += d * d;
    }
    sum.sqrt()
}

/// Extract features for every `frame_ms` frame of `pcm`. Pure. Ports
/// `extractFeatures` (no overlap; trailing partial frame is dropped).
pub fn extract_features(pcm: &[f32], sample_rate: u32, frame_ms: u32) -> Vec<AnalysisFrame> {
    if pcm.is_empty() || sample_rate == 0 || frame_ms == 0 {
        return Vec::new();
    }
    let samples_per_frame = (sample_rate as usize * frame_ms as usize) / 1000;
    if samples_per_frame == 0 {
        return Vec::new();
    }
    let total = pcm.len() / samples_per_frame;
    let mut frames = Vec::with_capacity(total);
    let mut prev_mag: Option<Vec<f64>> = None;
    for f in 0..total {
        let offset = f * samples_per_frame;
        let slice = &pcm[offset..offset + samples_per_frame];
        let start_sec = offset as f64 / sample_rate as f64;
        let r = rms_db(slice);
        let z = zcr_per_second(slice, sample_rate);
        let (centroid, magnitude) = spectrum(slice, sample_rate);
        let flux = spectral_flux(&magnitude, prev_mag.as_deref());
        prev_mag = Some(magnitude);
        frames.push(AnalysisFrame {
            start_sec,
            rms_db: r,
            zcr_per_sec: z,
            spectral_centroid: centroid,
            spectral_flux: flux,
        });
    }
    frames
}

// ── Classifier ────────────────────────────────────────────────────────────────

/// Classify a single frame. Ports `classifyFrame` exactly — the same thresholds
/// and score table, returning the type + a 0..1 confidence.
pub fn classify_frame(frame: &AnalysisFrame) -> (SegmentType, f64) {
    let r = frame.rms_db;
    let z = frame.zcr_per_sec;
    let c = frame.spectral_centroid;
    let fx = frame.spectral_flux;

    // silence: hard energy threshold
    if r < SILENCE_DB {
        let margin = ((SILENCE_DB - r) / SILENCE_CONFIDENCE_FULL_MARGIN_DB).min(1.0);
        return (
            SegmentType::Silence,
            SILENCE_CONFIDENCE_FLOOR + SILENCE_CONFIDENCE_SPAN * margin,
        );
    }

    let speech_zcr = (SPEECH_ZCR_MIN_PER_SEC..=SPEECH_ZCR_MAX_PER_SEC).contains(&z);
    let speech_centroid = (SPEECH_CENTROID_MIN_HZ..=SPEECH_CENTROID_MAX_HZ).contains(&c);
    let speech_flux = fx > SPEECH_FLUX_MIN;
    let speech_energy = (SPEECH_ENERGY_MIN_DB..=SPEECH_ENERGY_MAX_DB).contains(&r);

    let music_zcr = z < MUSIC_ZCR_MAX_PER_SEC;
    let music_flux = fx < MUSIC_FLUX_MAX;
    let music_energy = r >= MUSIC_ENERGY_MIN_DB;

    let mut speech_score = 0;
    if speech_energy {
        speech_score += 1;
    }
    if speech_zcr {
        speech_score += 1;
    }
    if speech_centroid {
        speech_score += 1;
    }
    if speech_flux {
        speech_score += 1;
    }

    let mut music_score = 0;
    if music_energy {
        music_score += 1;
    }
    if music_zcr {
        music_score += 1;
    }
    if music_flux {
        music_score += 1;
    }

    if speech_score == 4 && music_score < 3 {
        return (SegmentType::Speech, CONF_SPEECH_ALL_FOUR);
    }
    if music_score == 3 && speech_score <= 2 {
        return (SegmentType::Music, CONF_MUSIC_ALL_THREE);
    }
    if speech_score >= 3 && speech_score > music_score {
        return (SegmentType::Speech, CONF_SPEECH_SOLID);
    }
    if music_score >= 2 && music_score >= speech_score {
        return (SegmentType::Music, CONF_MUSIC_SOLID);
    }
    if speech_score >= 2 && music_score >= 2 {
        return (SegmentType::Mixed, CONF_MIXED);
    }
    (SegmentType::Unknown, CONF_UNKNOWN)
}

// ── The model seam ────────────────────────────────────────────────────────────

/// Everything [`crate::detect::analyse_pcm`] has, handed to the scorer whole.
///
/// The seam used to pass `frames` alone, and that was the one shape a
/// voice-activity model could not wear: a VAD needs SAMPLES, and the four
/// derived features in [`AnalysisFrame`] are not invertible. The E9 spike proved
/// the mismatch by having to smuggle the PCM into an adapter's field — a
/// workaround, not a design. So the seam carries what the caller already holds.
///
/// `frames` MUST be `extract_features(pcm, sample_rate, frame_ms)` for the same
/// three values carried here. Nothing enforces that, because enforcing it would
/// mean extracting the features twice; `analyse_pcm` is the only place in the
/// app that builds one of these, and it builds both halves in the same two
/// lines.
#[derive(Debug, Clone, Copy)]
pub struct ScoringInput<'a> {
    /// The decoded mono PCM the frames were derived from. A feature-based scorer
    /// ignores it; a model cannot work without it.
    pub pcm: &'a [f32],
    pub sample_rate: u32,
    pub frame_ms: u32,
    /// One entry per complete frame of `pcm`, in order.
    pub frames: &'a [AnalysisFrame],
}

/// How a frame is judged — the ONE thing a voice-activity-detection model
/// replaces.
///
/// Everything downstream (smoothing, grouping, short-segment merging, sermon
/// selection, the attention reasons) consumes only this trait's output, so
/// swapping the implementation is a one-argument change at
/// [`crate::detect::analyse_pcm`] and nothing else moves. That is the entire
/// point of the trait; it has two implementations —
/// [`HeuristicScorer`] here and [`crate::shadow::VadScorer`].
///
/// Whole-slice rather than per-frame on purpose: a model sees a window, not an
/// isolated frame, and a per-frame signature would forbid batching a single
/// forward pass over the file.
pub trait FrameScorer {
    /// Classify every frame of `input`, in order. The returned vector MUST be
    /// the same length as `input.frames`; grouping indexes the two in lockstep.
    fn classify_frames(&self, input: ScoringInput<'_>) -> Vec<(SegmentType, f64)>;
}

/// The feature-based heuristic this app has shipped since the Electron port —
/// [`classify_frame`] over each frame independently.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicScorer;

impl FrameScorer for HeuristicScorer {
    /// Reads only `input.frames`. The PCM beside them is what a model needs and
    /// this scorer does not — carrying it costs a pointer and a length, and
    /// keeping the two scorers on one signature is what makes them substitutable
    /// at all.
    fn classify_frames(&self, input: ScoringInput<'_>) -> Vec<(SegmentType, f64)> {
        input.frames.iter().map(classify_frame).collect()
    }
}

/// Median (mode) filter over a type sequence — for each index, the most frequent
/// type in `±half_win`. Ports `medianSmooth`.
pub fn median_smooth(types: &[SegmentType], half_win: usize) -> Vec<SegmentType> {
    let mut out = Vec::with_capacity(types.len());
    for i in 0..types.len() {
        let lo = i.saturating_sub(half_win);
        let hi = (i + half_win).min(types.len() - 1);
        let mut counts: [u32; 5] = [0; 5];
        let mut best = types[i];
        let mut best_count = 0;
        for t in &types[lo..=hi] {
            let idx = type_index(*t);
            counts[idx] += 1;
            if counts[idx] > best_count {
                best_count = counts[idx];
                best = *t;
            }
        }
        out.push(best);
    }
    out
}

fn type_index(t: SegmentType) -> usize {
    match t {
        SegmentType::Silence => 0,
        SegmentType::Speech => 1,
        SegmentType::Music => 2,
        SegmentType::Mixed => 3,
        SegmentType::Unknown => 4,
    }
}

/// Group consecutive same-type frames into segments. Ports `groupSegments`:
/// `endSec` is the next frame's start (or last frame start + frame duration at
/// EOF); `avgRmsDb` skips `-inf`; `confidence` is the mean.
fn group_segments(
    frames: &[AnalysisFrame],
    types: &[SegmentType],
    confidences: &[f64],
) -> Vec<AnalysisSegment> {
    if frames.is_empty() {
        return Vec::new();
    }
    let mut segments = Vec::new();
    let mut seg_start = 0usize;
    let mut seg_type = types[0];

    let close = |segments: &mut Vec<AnalysisSegment>,
                 start_frame: usize,
                 end_frame: usize,
                 seg_type: SegmentType| {
        let start_sec = frames[start_frame].start_sec;
        let end_sec = if end_frame < frames.len() {
            frames[end_frame].start_sec
        } else {
            frames[end_frame - 1].start_sec + FRAME_MS as f64 / 1000.0
        };
        let mut rms_sum = 0.0;
        let mut rms_count = 0;
        let mut conf_sum = 0.0;
        for i in start_frame..end_frame {
            let r = frames[i].rms_db;
            if r.is_finite() {
                rms_sum += r;
                rms_count += 1;
            }
            conf_sum += confidences[i];
        }
        let avg_rms_db = if rms_count > 0 {
            rms_sum / rms_count as f64
        } else {
            f64::NEG_INFINITY
        };
        let confidence = conf_sum / (end_frame - start_frame) as f64;
        segments.push(AnalysisSegment {
            start_sec,
            end_sec,
            duration_sec: end_sec - start_sec,
            seg_type,
            confidence,
            avg_rms_db,
            label: seg_type.label().to_string(),
        });
    };

    // `i` is used as a segment boundary value (not just an index), so the
    // range loop is the clearest form here.
    #[allow(clippy::needless_range_loop)]
    for i in 1..frames.len() {
        if types[i] != seg_type {
            close(&mut segments, seg_start, i, seg_type);
            seg_start = i;
            seg_type = types[i];
        }
    }
    close(&mut segments, seg_start, frames.len(), seg_type);
    segments
}

/// Extend `target` to swallow `victim` on the given side. Ports `extendInto`.
fn extend_into(target: &AnalysisSegment, victim: &AnalysisSegment, right: bool) -> AnalysisSegment {
    if right {
        AnalysisSegment {
            end_sec: victim.end_sec,
            duration_sec: victim.end_sec - target.start_sec,
            ..target.clone()
        }
    } else {
        AnalysisSegment {
            start_sec: victim.start_sec,
            duration_sec: target.end_sec - victim.start_sec,
            ..target.clone()
        }
    }
}

/// Merge consecutive same-type segments. Ports `collapseAdjacent`.
fn collapse_adjacent(segments: Vec<AnalysisSegment>) -> Vec<AnalysisSegment> {
    if segments.len() <= 1 {
        return segments;
    }
    let mut out: Vec<AnalysisSegment> = vec![segments[0].clone()];
    for cur in &segments[1..] {
        let last = out.last_mut().unwrap();
        if cur.seg_type == last.seg_type {
            last.end_sec = cur.end_sec;
            last.duration_sec = cur.end_sec - last.start_sec;
            last.confidence = (last.confidence + cur.confidence) / 2.0;
            last.avg_rms_db = if last.avg_rms_db.is_finite() && cur.avg_rms_db.is_finite() {
                (last.avg_rms_db + cur.avg_rms_db) / 2.0
            } else if last.avg_rms_db.is_finite() {
                last.avg_rms_db
            } else {
                cur.avg_rms_db
            };
        } else {
            out.push(cur.clone());
        }
    }
    out
}

/// Merge segments shorter than [`MIN_SEGMENT_SEC`] into the longer neighbour.
/// Ports `mergeShortSegments` (≤[`MERGE_MAX_PASSES`] convergence passes, then
/// `collapseAdjacent`).
pub fn merge_short_segments(segments: &[AnalysisSegment]) -> Vec<AnalysisSegment> {
    if segments.len() <= 1 {
        return segments.to_vec();
    }
    let mut work = segments.to_vec();
    let mut changed = true;
    let mut iterations = 0;
    while changed && iterations < MERGE_MAX_PASSES {
        changed = false;
        iterations += 1;
        let mut next: Vec<AnalysisSegment> = Vec::new();
        let mut i = 0;
        while i < work.len() {
            let seg = work[i].clone();
            if seg.duration_sec >= MIN_SEGMENT_SEC || work.len() == 1 {
                next.push(seg);
                i += 1;
                continue;
            }
            let prev = next.last().cloned();
            let nxt = work.get(i + 1).cloned();
            match (prev, nxt) {
                (None, None) => {
                    next.push(seg);
                }
                (None, Some(nxt)) => {
                    work[i + 1] = extend_into(&nxt, &seg, false);
                    changed = true;
                }
                (Some(prev), None) => {
                    *next.last_mut().unwrap() = extend_into(&prev, &seg, true);
                    changed = true;
                }
                (Some(prev), Some(nxt)) => {
                    if prev.duration_sec >= nxt.duration_sec {
                        *next.last_mut().unwrap() = extend_into(&prev, &seg, true);
                    } else {
                        work[i + 1] = extend_into(&nxt, &seg, false);
                    }
                    changed = true;
                }
            }
            i += 1;
        }
        work = next;
    }
    collapse_adjacent(work)
}

/// Classify → smooth → group → merge, with the shipped heuristic. Ports
/// `classifyAndGroup`.
pub fn classify_and_group(input: ScoringInput<'_>) -> Vec<AnalysisSegment> {
    classify_and_group_with(input, &HeuristicScorer)
}

/// [`classify_and_group`] with the frame scorer chosen by the caller — the seam
/// in use. Everything after the scorer is fixed.
pub fn classify_and_group_with(
    input: ScoringInput<'_>,
    scorer: &dyn FrameScorer,
) -> Vec<AnalysisSegment> {
    if input.frames.is_empty() {
        return Vec::new();
    }
    let scored = scorer.classify_frames(input);
    assert_eq!(
        scored.len(),
        input.frames.len(),
        "FrameScorer returned {} scores for {} frames",
        scored.len(),
        input.frames.len()
    );
    let raw_types: Vec<SegmentType> = scored.iter().map(|(t, _)| *t).collect();
    let confidences: Vec<f64> = scored.iter().map(|(_, c)| *c).collect();
    let smoothed = median_smooth(&raw_types, SMOOTH_HALF_WIN);
    let grouped = group_segments(input.frames, &smoothed, &confidences);
    merge_short_segments(&grouped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_window_handles_degenerate_sizes_without_nan() {
        assert_eq!(hann_window(0).len(), 0);
        // size 1 would divide by (size-1)=0 → NaN; must be a finite 1.0.
        let w1 = hann_window(1);
        assert_eq!(w1.len(), 1);
        assert!(w1[0].is_finite());
        // Normal sizes are unchanged: endpoints ~0, centre ~1.
        let w = hann_window(8);
        assert!(w.iter().all(|x| x.is_finite()));
        assert!(w[0].abs() < 1e-9);
    }

    // ── FFT ──────────────────────────────────────────────────────────────────

    #[test]
    fn fft_of_dc_signal_puts_all_energy_in_bin_zero() {
        let mut re = vec![1.0_f64; 8];
        let mut im = vec![0.0_f64; 8];
        fft(&mut re, &mut im);
        assert!((re[0] - 8.0).abs() < 1e-9);
        for (k, &v) in re.iter().enumerate().skip(1) {
            assert!(v.abs() < 1e-9, "bin {k} re = {v}");
        }
    }

    #[test]
    fn fft_of_single_cycle_sine_peaks_at_bin_one() {
        let n = 16;
        let mut re: Vec<f64> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * i as f64 / n as f64).sin())
            .collect();
        let mut im = vec![0.0_f64; n];
        fft(&mut re, &mut im);
        let mag = |k: usize| (re[k] * re[k] + im[k] * im[k]).sqrt();
        // Energy concentrated at bin 1 (and its mirror n-1).
        assert!(mag(1) > 1.0);
        assert!(mag(2) < 1e-6);
    }

    #[test]
    #[should_panic(expected = "power of 2")]
    fn fft_rejects_non_power_of_two() {
        let mut re = vec![0.0; 3];
        let mut im = vec![0.0; 3];
        fft(&mut re, &mut im);
    }

    // ── features ────────────────────────────────────────────────────────────────

    #[test]
    fn rms_db_of_silence_is_neg_inf() {
        assert_eq!(rms_db(&[0.0; 100]), f64::NEG_INFINITY);
        assert_eq!(rms_db(&[]), f64::NEG_INFINITY);
    }

    #[test]
    fn rms_db_of_full_scale_is_zero() {
        let s = [1.0_f32; 100];
        assert!((rms_db(&s) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn zcr_counts_sign_flips_per_second() {
        // Alternating ±1 at 16 kHz over 1600 samples (0.1 s) → 1599 crossings
        // in 0.1 s → ~15990 /sec.
        let s: Vec<f32> = (0..1600)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let z = zcr_per_second(&s, 16000);
        assert!((z - 15990.0).abs() < 1.0);
    }

    #[test]
    fn extract_features_drops_partial_trailing_frame() {
        // 1.5 frames worth of samples → only 1 full frame extracted.
        let pcm = vec![0.1_f32; FRAME_SAMPLES + FRAME_SAMPLES / 2];
        let frames = extract_features(&pcm, SAMPLE_RATE, FRAME_MS);
        assert_eq!(frames.len(), 1);
    }

    // ── spectral flux ─────────────────────────────────────────────────────────

    #[test]
    fn spectral_flux_is_invariant_under_gain() {
        // The whole point of the normalisation: scaling both spectra by any
        // gain must not move the flux. 20 dB here; the property holds for any.
        let curr = vec![1.0, 3.0, 0.5, 2.0];
        let prev = vec![0.5, 1.0, 2.0, 0.1];
        let g = 10.0_f64; // +20 dB
        let scaled_curr: Vec<f64> = curr.iter().map(|x| x * g).collect();
        let scaled_prev: Vec<f64> = prev.iter().map(|x| x * g).collect();
        let base = spectral_flux(&curr, Some(&prev));
        let hot = spectral_flux(&scaled_curr, Some(&scaled_prev));
        assert!(
            (base - hot).abs() < 1e-12,
            "flux moved with level: {base} vs {hot}"
        );
        assert!(base > 0.0);
    }

    #[test]
    fn spectral_flux_of_unchanged_shape_is_zero_regardless_of_level() {
        // A pure level change between frames — same shape, 6 dB apart — is
        // exactly what the old raw-magnitude flux misread as spectral change.
        let curr = vec![2.0, 6.0, 1.0, 4.0];
        let prev: Vec<f64> = curr.iter().map(|x| x * 0.5).collect();
        assert!(spectral_flux(&curr, Some(&prev)).abs() < 1e-12);
    }

    #[test]
    fn spectral_flux_is_bounded_by_sqrt_two() {
        // Disjoint spectra are the worst case: two non-negative unit vectors
        // can be at most √2 apart.
        let curr = vec![1.0, 0.0, 0.0, 0.0];
        let prev = vec![0.0, 0.0, 0.0, 1.0];
        let flux = spectral_flux(&curr, Some(&prev));
        assert!((flux - std::f64::consts::SQRT_2).abs() < 1e-12);
    }

    #[test]
    fn spectral_flux_guards_degenerate_inputs() {
        // No previous frame (the file's first) and a numerically zero spectrum
        // both answer 0 — a unit vector cannot be made from digital silence.
        let curr = vec![1.0, 2.0];
        assert_eq!(spectral_flux(&curr, None), 0.0);
        let zero = vec![0.0, 0.0];
        assert_eq!(spectral_flux(&curr, Some(&zero)), 0.0);
        assert_eq!(spectral_flux(&zero, Some(&curr)), 0.0);
    }

    // ── classifier ──────────────────────────────────────────────────────────────

    #[test]
    fn classifies_low_energy_as_silence() {
        let frame = AnalysisFrame {
            start_sec: 0.0,
            rms_db: -60.0,
            zcr_per_sec: 1000.0,
            spectral_centroid: 1000.0,
            spectral_flux: 0.7,
        };
        let (t, conf) = classify_frame(&frame);
        assert_eq!(t, SegmentType::Silence);
        assert!(conf > 0.6);
    }

    #[test]
    fn classifies_full_speech_signature() {
        let frame = AnalysisFrame {
            start_sec: 0.0,
            rms_db: -20.0,
            zcr_per_sec: 2000.0,
            spectral_centroid: 1500.0,
            spectral_flux: 0.6,
        };
        assert_eq!(classify_frame(&frame).0, SegmentType::Speech);
    }

    #[test]
    fn classifies_sustained_tone_as_music() {
        let frame = AnalysisFrame {
            start_sec: 0.0,
            rms_db: -15.0,
            zcr_per_sec: 800.0,
            spectral_centroid: 4000.0,
            spectral_flux: 0.05,
        };
        assert_eq!(classify_frame(&frame).0, SegmentType::Music);
    }

    // ── smoothing ─────────────────────────────────────────────────────────────────

    #[test]
    fn median_smooth_removes_single_outlier() {
        use SegmentType::*;
        let types = vec![Speech, Speech, Music, Speech, Speech];
        let out = median_smooth(&types, 2);
        // The lone Music frame is outvoted by surrounding Speech.
        assert_eq!(out[2], Speech);
    }

    // ── grouping + merge ───────────────────────────────────────────────────────────

    fn seg(start: f64, end: f64, t: SegmentType) -> AnalysisSegment {
        AnalysisSegment {
            start_sec: start,
            end_sec: end,
            duration_sec: end - start,
            seg_type: t,
            confidence: 0.8,
            avg_rms_db: -20.0,
            label: t.label().to_string(),
        }
    }

    #[test]
    fn merge_absorbs_short_island_into_longer_neighbour() {
        use SegmentType::*;
        let segs = vec![
            seg(0.0, 120.0, Speech),
            seg(120.0, 122.0, Mixed), // 2 s island
            seg(122.0, 240.0, Speech),
        ];
        let merged = merge_short_segments(&segs);
        // Island absorbed; the two Speech blocks collapse into one.
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].seg_type, Speech);
        assert_eq!(merged[0].start_sec, 0.0);
        assert_eq!(merged[0].end_sec, 240.0);
    }

    #[test]
    fn merge_keeps_single_short_segment() {
        let segs = vec![seg(0.0, 2.0, SegmentType::Speech)];
        assert_eq!(merge_short_segments(&segs).len(), 1);
    }

    #[test]
    fn merge_absorbs_head_island_into_next() {
        use SegmentType::*;
        // A short segment at the very start (no prev) is absorbed FORWARD.
        let segs = vec![seg(0.0, 2.0, Mixed), seg(2.0, 200.0, Speech)];
        let merged = merge_short_segments(&segs);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].seg_type, Speech);
        assert_eq!(merged[0].start_sec, 0.0);
        assert_eq!(merged[0].end_sec, 200.0);
    }

    #[test]
    fn merge_absorbs_tail_island_into_prev() {
        use SegmentType::*;
        // A short segment at the very end (no next) is absorbed BACKWARD.
        let segs = vec![seg(0.0, 198.0, Speech), seg(198.0, 200.0, Mixed)];
        let merged = merge_short_segments(&segs);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].seg_type, Speech);
        assert_eq!(merged[0].start_sec, 0.0);
        assert_eq!(merged[0].end_sec, 200.0);
    }

    #[test]
    fn merge_prefers_the_longer_of_two_neighbours() {
        use SegmentType::*;
        // Island between a 50 s Speech (prev) and a 148 s Music (next): the longer
        // next neighbour wins, so the boundary moves to the island's start (50 s)
        // and the island joins Music — not Speech.
        let segs = vec![
            seg(0.0, 50.0, Speech),
            seg(50.0, 52.0, Mixed), // 2 s island (< MIN_SEGMENT_SEC)
            seg(52.0, 200.0, Music),
        ];
        let merged = merge_short_segments(&segs);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].seg_type, Speech);
        assert_eq!(
            merged[0].end_sec, 50.0,
            "Speech keeps its end → island not absorbed left"
        );
        assert_eq!(merged[1].seg_type, Music);
        assert_eq!(merged[1].start_sec, 50.0, "Music absorbs the island");
    }
}
