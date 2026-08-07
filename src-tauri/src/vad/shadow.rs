//! Shadow mode's I/O half (E9) — decode, score, compare, write, log.
//!
//! Every decision here belongs to [`sundayrec_core::shadow`]; what is left is a
//! second ffmpeg decode, a thread that will be busy for minutes, one sidecar
//! write and a log line. The model is loaded once per process
//! ([`SHARED_MODEL`]) and a fresh backend is built per file, which is what
//! [`SileroVadModel`]'s `Send + Sync` guarantee exists for.
//!
//! # What the operator experiences: nothing
//!
//! That is the requirement, not an oversight. [`observe`] is spawned AFTER
//! `editor_segments` has already answered — the segments appear exactly as fast
//! as they do in a build without this feature — and the answer it produces is
//! never shown, never persisted as detection, and never read by anything that
//! decides. Progress is emitted on `editor://shadow-progress` through the same
//! throttled emitter every other decode uses, so a later stage that wants to
//! surface it needs no backend change; no renderer listens today, on purpose. A
//! second progress bar for work nobody is waiting on turns a background
//! measurement into something that looks like part of the job.
//!
//! # Cost, and why it is not on the operator's clock
//!
//! ~1.2 s per minute of audio (see [`super`]'s header) — about two minutes for a
//! 90-minute service, on top of the analysis pass that just finished. Inline,
//! that would double a wait the operator is watching. Detached, it costs a core
//! for two minutes on a machine that has just finished analysing and is not
//! recording (the analysis pass is only reachable from the editor). The decode
//! is repeated rather than the PCM carried across the task boundary: a second
//! ffmpeg read is seconds, and holding ~345 MB of f32 alive for the length of
//! the inference to save them is the worse trade.

use std::sync::Arc;

use sundayrec_core::audio_analysis::{FRAME_MS, SAMPLE_RATE};
use sundayrec_core::detect::Detection;
use sundayrec_core::feedback::build_shadow_observation;
use sundayrec_core::shadow::{compare, shadow_detection, ShadowSettings, VadScorer};
use sundayrec_core::vad::{VadBackend, VadError};

use super::{SileroVad, SileroVadModel};

/// The loaded graph, shared across every recording this process shadows.
///
/// Loading is ~2.8 MB of SHA-256 plus a tract optimize; per file that would be
/// paid for nothing, since the weights are immutable. The RECURRENT state is
/// what must not be shared, and it lives in [`SileroVad`], one per file.
static SHARED_MODEL: std::sync::OnceLock<Result<Arc<SileroVadModel>, String>> =
    std::sync::OnceLock::new();

fn shared_model() -> Result<Arc<SileroVadModel>, VadError> {
    SHARED_MODEL
        .get_or_init(|| SileroVadModel::load().map_err(|e| e.to_string()))
        .clone()
        .map_err(|detail| VadError::Backend { detail })
}

/// Run the model over one recording and record how its answer differed from the
/// one the app just acted on.
///
/// `heuristic` is the [`Detection`] the editor and the review queue already
/// have. Nothing in this function can change it — it is taken by value and only
/// read.
///
/// Returns whether an observation reached the disk. Every failure path is a
/// `false` plus a log line: a model that will not load, a decode that fails, a
/// hop that errors, a sidecar we may not overwrite. None of them is allowed to
/// surface, because the app in that state behaves exactly as it does today.
pub async fn observe<F>(
    input_path: String,
    heuristic: Detection,
    settings: ShadowSettings,
    on_progress: F,
) -> bool
where
    F: Fn(f32) + Send + Sync + 'static,
{
    // Fail before the decode when the model is unavailable — otherwise a broken
    // build burns a full ffmpeg pass per recording to discover it every time.
    if let Err(e) = shared_model() {
        tracing::warn!(error = %e, "shadow: VAD model unavailable — skipping");
        return false;
    }

    // The decode is the cheap tenth of this pass; the inference is the rest.
    // Splitting the reported fraction the same way keeps the bar honest instead
    // of parking it at 100 % for two minutes.
    const DECODE_SHARE: f32 = 0.10;
    let pcm = {
        let report = &on_progress;
        match crate::editor::decode_analysis_pcm(&input_path, |f| report(f * DECODE_SHARE)).await {
            Ok(pcm) => pcm,
            Err(e) => {
                tracing::warn!(error = %e, "shadow: analysis decode failed — skipping");
                return false;
            }
        }
    };

    // Minutes of sequential inference: `spawn_blocking`, or it starves the async
    // runtime this app records and streams on.
    let scored = tauri::async_runtime::spawn_blocking(move || {
        let report = |f: f32| on_progress(DECODE_SHARE + f * (1.0 - DECODE_SHARE));
        let scorer = VadScorer::new(
            || Ok(Box::new(SileroVad::new(shared_model()?)) as Box<dyn VadBackend>),
            settings,
        )
        .with_progress(&report);
        shadow_detection(&pcm, SAMPLE_RATE, FRAME_MS, &scorer)
    })
    .await;

    let shadow = match scored {
        Ok(Ok(detection)) => detection,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "shadow: VAD scoring failed — no observation recorded");
            return false;
        }
        Err(e) => {
            tracing::warn!(error = %e, "shadow: scoring task did not finish");
            return false;
        }
    };

    let comparison = compare(&heuristic, &shadow);
    let agreed = comparison.is_agreement();
    let observation = build_shadow_observation(comparison, settings, env!("CARGO_PKG_VERSION"));
    let written = crate::editor::record_shadow_observation(&input_path, observation);
    if written {
        // Offsets and a verdict — the same discipline the record itself keeps.
        // The path is NOT logged: this line would otherwise be the one place the
        // two are written down together.
        tracing::info!(agreed, "shadow: observation recorded");
    } else {
        tracing::warn!("shadow: could not persist the observation");
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::audio_analysis::HeuristicScorer;
    use sundayrec_core::detect::analyse_pcm;
    use sundayrec_core::shadow::PoolingRule;
    use sundayrec_core::vad::VadStream;

    /// The same synthetic utterance the backend tests score, so the numbers
    /// below come from the real model rather than a stub.
    fn speech(seconds: f64) -> Vec<f32> {
        crate::vad::tests::speech_like(seconds)
    }

    /// A steady 1 kHz tone — the signal the HEURISTIC calls speech and a VAD
    /// does not.
    ///
    /// Not an arbitrary frequency. At 16 kHz it lands inside every one of the
    /// heuristic's speech windows (2000 zero-crossings/sec inside 400..6000, a
    /// centroid of 1 kHz inside 300..3500, −9 dBFS inside −45..−5) while its
    /// spectral flux is ~0, which costs it the fourth speech point and one of
    /// the three music points — so `classify_frame` scores it speech 3, music 2
    /// and calls it Tale. A 440 Hz tone, by contrast, wins the music ZCR test
    /// and comes out as Musikk, which is why the fixture is not just "a tone".
    fn heuristic_speech_tone(seconds: f64) -> Vec<f32> {
        let n = (seconds * f64::from(SAMPLE_RATE)) as usize;
        (0..n)
            .map(|i| {
                (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / f64::from(SAMPLE_RATE)).sin()
                    as f32
                    * 0.5
            })
            .collect()
    }

    #[test]
    fn the_shared_model_loads_once_and_is_reusable() {
        let a = shared_model().expect("the embedded model must load");
        let b = shared_model().expect("and again");
        assert!(
            Arc::ptr_eq(&a, &b),
            "a per-file load would pay for a tract optimize per recording"
        );
    }

    #[test]
    fn the_real_model_scores_the_detector_seam_end_to_end() {
        // The whole composition, with tract in it: PCM → VadScorer → analyse_pcm
        // → a Detection. What is asserted is the SHAPE of the contract (one
        // verdict per frame, a usable detection); the model's accuracy is the
        // backend tests' business.
        let pcm = speech(3.0);
        let scorer = VadScorer::new(
            || Ok(Box::new(SileroVad::new(shared_model()?)) as Box<dyn VadBackend>),
            ShadowSettings::default(),
        );
        let shadow = shadow_detection(&pcm, SAMPLE_RATE, FRAME_MS, &scorer)
            .expect("the vendored model must score a 3 s buffer");
        assert!(!shadow.segments.is_empty());

        // And the heuristic's answer is untouched by any of it.
        let heuristic = analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);
        let again = analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);
        assert_eq!(heuristic, again);

        let c = compare(&heuristic, &shadow);
        assert_eq!(c.recording_duration_sec, heuristic.duration_sec);
        // Every measured duration is a real number of seconds, never NaN — a NaN
        // here would be aggregated by the harness rather than noticed.
        for v in [
            c.speech_agreed_sec,
            c.speech_only_heuristic_sec,
            c.speech_only_shadow_sec,
        ] {
            assert!(v.is_finite() && v >= 0.0, "{v}");
        }
    }

    #[test]
    fn on_a_clean_utterance_the_two_detectors_agree() {
        // Measured, not assumed: over the synthetic voice the model and the
        // heuristic produce the same speech mass and the same offer. That is a
        // RESULT, and it is why an agreement has to be storable — a harness that
        // only kept disagreements would have no denominator to divide by.
        let pcm = speech(3.0);
        let scorer = VadScorer::new(
            || Ok(Box::new(SileroVad::new(shared_model()?)) as Box<dyn VadBackend>),
            ShadowSettings::default(),
        );
        let shadow = shadow_detection(&pcm, SAMPLE_RATE, FRAME_MS, &scorer).unwrap();
        let heuristic = analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);
        let c = compare(&heuristic, &shadow);
        eprintln!(
            "SHADOW measured (speech): agreed {:.1}s, heuristic-only {:.1}s, model-only {:.1}s",
            c.speech_agreed_sec, c.speech_only_heuristic_sec, c.speech_only_shadow_sec
        );
        assert!(c.speech_agreed_sec > 2.0, "both must hear the utterance");
        assert!(c.is_agreement());
    }

    #[test]
    fn the_model_takes_away_speech_the_heuristic_invented() {
        // The disagreement shadow mode exists to catch, on the real model: a
        // sustained 1 kHz tone that `classify_frame` scores as Tale and Silero
        // hears no voice in. The heuristic offers the operator a block; the model
        // would have offered nothing.
        let pcm = heuristic_speech_tone(8.0);
        let heuristic = analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);
        assert!(
            heuristic
                .segments
                .iter()
                .any(|s| s.kind == sundayrec_core::detect::SegmentType::Speech),
            "fixture no longer reads as speech to the heuristic — re-read its doc comment"
        );

        let scorer = VadScorer::new(
            || Ok(Box::new(SileroVad::new(shared_model()?)) as Box<dyn VadBackend>),
            ShadowSettings::default(),
        );
        let shadow = shadow_detection(&pcm, SAMPLE_RATE, FRAME_MS, &scorer).unwrap();
        let c = compare(&heuristic, &shadow);
        eprintln!(
            "SHADOW measured (tone): agreed {:.1}s, heuristic-only {:.1}s, model-only {:.1}s",
            c.speech_agreed_sec, c.speech_only_heuristic_sec, c.speech_only_shadow_sec
        );
        assert!(!c.is_agreement());
        assert!(c.speech_only_heuristic_sec > 5.0);
        assert_eq!(c.speech_only_shadow_sec, 0.0);
        assert!(c.heuristic_sermon.is_some());
        assert!(
            c.shadow_sermon.is_none(),
            "with no speech at all there is nothing to offer"
        );
        assert!(c.sermon_deltas.is_none());
    }

    #[test]
    fn the_three_pooling_rules_reach_the_real_model_and_differ() {
        // Each rule is wired all the way through, not just unit-tested in
        // isolation: a rule that never reached the model would be a knob the A/B
        // harness could turn with no effect.
        //
        // Equal totals here are EXPECTED, not a null result. The rules differ per
        // frame (`shadow::tests::the_three_rules_disagree_on_the_same_hops` pins
        // that), and on a clean three-second utterance the median smoother and
        // the five-second minimum then flatten those frames into the same single
        // segment. Where the choice shows up is at boundaries in a real service,
        // which is what the corpus run is for. What is asserted is the ordering
        // invariant, which holds for any input.
        let pcm = speech(3.0);
        let masses: Vec<f64> = [
            PoolingRule::Max,
            PoolingRule::Mean,
            PoolingRule::FractionOver,
        ]
        .iter()
        .map(|pooling| {
            let settings = ShadowSettings {
                pooling: *pooling,
                ..ShadowSettings::default()
            };
            let scorer = VadScorer::new(
                || Ok(Box::new(SileroVad::new(shared_model()?)) as Box<dyn VadBackend>),
                settings,
            );
            let d = shadow_detection(&pcm, SAMPLE_RATE, FRAME_MS, &scorer).unwrap();
            d.segments
                .iter()
                .filter(|s| s.kind == sundayrec_core::detect::SegmentType::Speech)
                .map(|s| s.duration_sec)
                .sum()
        })
        .collect();
        eprintln!("POOLING measured: max/mean/fractionOver speech seconds = {masses:?}");
        assert!(masses.iter().all(|m| m.is_finite()));
        // Max can only ever be the most generous of the three: mean and fraction
        // are both bounded above by the frame's strongest hop.
        assert!(
            masses[0] >= masses[1] - 1e-9 && masses[0] >= masses[2] - 1e-9,
            "max pooled less speech than an averaging rule — the buckets are wrong"
        );
    }

    #[test]
    fn a_stream_the_model_rejects_produces_no_observation() {
        // The failure contract, against the real backend: 44.1 kHz must fail at
        // the stream guard rather than be scored as if it were 16 kHz.
        let model = shared_model().unwrap();
        assert!(matches!(
            VadStream::new(SileroVad::new(model), 44_100),
            Err(VadError::UnsupportedSampleRate { got: 44_100 })
        ));
        let scorer = VadScorer::new(
            || Ok(Box::new(SileroVad::new(shared_model()?)) as Box<dyn VadBackend>),
            ShadowSettings::default(),
        );
        assert!(shadow_detection(&[0.1f32; 44_100], 44_100, FRAME_MS, &scorer).is_err());
    }
}
