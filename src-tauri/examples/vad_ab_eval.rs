//! Does the neural VAD beat the heuristic? — the offline A/B harness (E9).
//!
//! ```text
//! cargo run --release --no-default-features --features vad --example vad_ab_eval -- \
//!     --dir ~/Documents/SundayRec
//! ```
//!
//! One command, one answer. It runs BOTH frame scorers over the same recordings
//! and reports which one agrees better with the corrections the owner actually
//! made — [`sundayrec_core::feedback`], the only ground truth there is for this
//! app. The judgement logic is pure and lives in
//! [`sundayrec_core::ab_eval`]; this file is its I/O shell and nothing more:
//! find files, decode them, load the model, hand the numbers over, print.
//!
//! **This is a developer/owner instrument, not a feature.** It is an example
//! target behind the default-off `vad` feature, so it is not in any release
//! build, no Tauri command reaches it, and nothing it computes is applied to
//! anything. It answers a question; a human acts on the answer.
//!
//! ## The expected answer, today, is "not enough data to tell"
//!
//! The corrections corpus started being captured in E8 and the owner has not yet
//! corrected anything. A harness that printed an impressive percentage over
//! three recordings would be worse than useless, so the sufficiency thresholds
//! in [`sundayrec_core::ab_eval`] are stated in the tool's own output and the
//! honest answer is the loud one. `--synthetic` proves the machinery runs
//! end to end WITHOUT pretending that is evidence about detection quality; every
//! line of that output says so.
//!
//! ## Exit codes
//!
//! `0` — the harness ran and produced a report, whatever the verdict was.
//! "Not enough data" is a successful run.
//! `1` — an operational failure: no ffmpeg, an unreadable directory, a file that
//! would not decode, a model that would not load. The report is not written.
//!
//! ## Why the arguments are parsed by hand
//!
//! Same reason `asio_spike.rs` takes none: this is a spike-grade owner tool in a
//! shipping app's dependency tree, and an argument-parsing crate would be a
//! permanent dependency for a temporary convenience.

#[cfg(feature = "vad")]
mod harness {
    use std::collections::BTreeSet;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitCode, Stdio};
    use std::sync::Arc;

    use sundayrec_core::ab_eval::{
        aggregate, evaluate_recording, ground_truths, parse_pooling_spec, render_summary, AbReport,
        PooledVadScorer, RecordingEvaluation, ScorerSelection, Span, VadHopStats, AB_REPORT_SCHEMA,
        MIN_CORPUS_FOR_ANY_CONCLUSION, MIN_CORPUS_FOR_A_DIRECTION, SELECTION_IOU_THRESHOLD, SWEEP,
    };
    use sundayrec_core::audio_analysis::{
        classify_and_group_with, extract_features, AnalysisFrame, FrameScorer, HeuristicScorer,
        ScoringInput, FRAME_MS, SAMPLE_RATE,
    };
    use sundayrec_core::detect::{detect, Detection, PrepAnalysisSegment};
    use sundayrec_core::editor::{analysis_decode_args, sidecar_path, Sidecar};
    use sundayrec_core::feedback::{
        FeedbackSegment, FeedbackSegmentKind, RecordingFeedback, SermonPickCorrection,
        FEEDBACK_SCHEMA,
    };
    use sundayrec_core::shadow::{ShadowSettings, DEFAULT_FRAME_SPEECH_THRESHOLD};
    use sundayrec_core::vad::{score_pcm, VAD_SAMPLE_RATE};
    use sundayrec_lib::vad::{SileroVad, SileroVadModel};

    /// Extensions the recorder and the editor produce. A directory scan rather
    /// than the SQLite history on purpose: the harness has to be runnable
    /// against a folder of exported recordings, or against `--synthetic`'s
    /// temporary one, neither of which the app's database knows about.
    const MEDIA_EXTENSIONS: [&str; 11] = [
        "wav", "flac", "m4a", "mp3", "aac", "ogg", "opus", "mp4", "mkv", "mov", "webm",
    ];

    struct Options {
        dirs: Vec<PathBuf>,
        json_out: Option<PathBuf>,
        /// One row per pooling rule to compare — see
        /// [`sundayrec_core::ab_eval::SWEEP`]. Each carries its own thresholds,
        /// so `--threshold` is applied when the row is built and never again.
        rows: Vec<ShadowSettings>,
        limit: Option<usize>,
        synthetic: Option<usize>,
    }

    const USAGE: &str = "\
vad_ab_eval — does the neural VAD agree with the human better than the heuristic?

  --dir <path>          Directory of recordings to scan. Repeatable.
  --json <path>         Write the machine-readable report here.
  --pooling <spec>      max | mean | fraction | fraction:<0..1> | sweep   (default: sweep)
                        `fraction:<t>` sets the per-HOP threshold the vote is
                        taken against; the other rules ignore it.
  --threshold <0..1>    Speech probability a pooled FRAME must reach. Applies to
                        every rule, which is why it is its own flag (default: 0.5)
  --limit <n>           Stop after n recordings (they are decoded in full, so a
                        90-minute service is a minute of work either way).
  --synthetic <n>       PLUMBING CHECK ONLY. Generate n fake recordings with
                        fabricated corrections in a temp directory and run over
                        those. The report is stamped as synthetic and is NOT
                        evidence about detection quality.
  --help

Exit 0 when the harness ran, whatever the verdict. Exit 1 on operational failure.
ffmpeg is found via SUNDAYREC_FFMPEG, then this repo's fetched sidecar, then PATH.
";

    fn parse_args() -> Result<Option<Options>, String> {
        let mut options = Options {
            dirs: Vec::new(),
            json_out: None,
            rows: Vec::new(),
            limit: None,
            synthetic: None,
        };
        // The rows are built AFTER the whole line is read: the frame threshold
        // is a property of every row, so `--threshold` must reach a `--pooling`
        // that came before it as well as one that came after.
        let mut pooling = "sweep".to_string();
        let mut frame_threshold = DEFAULT_FRAME_SPEECH_THRESHOLD;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or(format!("{arg} needs a value"));
            match arg.as_str() {
                "--help" | "-h" => return Ok(None),
                "--dir" => options.dirs.push(PathBuf::from(value()?)),
                "--json" => options.json_out = Some(PathBuf::from(value()?)),
                "--pooling" => pooling = value()?,
                "--threshold" => {
                    let raw = value()?;
                    let parsed: f64 = raw.parse().map_err(|_| format!("bad threshold `{raw}`"))?;
                    if !(0.0..=1.0).contains(&parsed) {
                        return Err(format!("threshold `{raw}` is not a probability"));
                    }
                    frame_threshold = parsed;
                }
                "--limit" => {
                    let raw = value()?;
                    options.limit = Some(raw.parse().map_err(|_| format!("bad limit `{raw}`"))?);
                }
                "--synthetic" => {
                    let raw = value()?;
                    options.synthetic =
                        Some(raw.parse().map_err(|_| format!("bad count `{raw}`"))?);
                }
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        options.rows = if pooling == "sweep" {
            SWEEP
                .iter()
                .map(|row| ShadowSettings {
                    frame_speech_threshold: frame_threshold,
                    ..*row
                })
                .collect()
        } else {
            vec![parse_pooling_spec(&pooling, frame_threshold)
                .ok_or(format!("unknown pooling rule `{pooling}`"))?]
        };
        if options.dirs.is_empty() && options.synthetic.is_none() {
            return Err("nothing to do: pass --dir <path> or --synthetic <n>".into());
        }
        Ok(Some(options))
    }

    // ── ffmpeg ─────────────────────────────────────────────────────────────

    /// Where ffmpeg is, for a target that does not sit beside the app's own
    /// sidecar.
    ///
    /// The shipped resolver (`media::ffmpeg::ffmpeg_path`) looks next to
    /// `current_exe()`, which for `target/debug/examples/vad_ab_eval` is a
    /// directory with no ffmpeg in it. So the repo's fetched dev sidecar is
    /// tried in between — and the shipped resolver is still the last word, so
    /// there is only ever one answer to "what does the app run".
    fn resolve_ffmpeg() -> String {
        if let Ok(from_env) = std::env::var("SUNDAYREC_FFMPEG") {
            return from_env;
        }
        let extension = if cfg!(windows) { ".exe" } else { "" };
        let fetched = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!(
                "ffmpeg-{}{extension}",
                env!("SUNDAYREC_TARGET_TRIPLE")
            ));
        if fetched.is_file() {
            return fetched.to_string_lossy().into_owned();
        }
        sundayrec_lib::media::ffmpeg::ffmpeg_path()
    }

    /// Decode to the exact PCM the shipped analysis path uses — same argv, from
    /// the same function, so this harness cannot measure a scorer against audio
    /// the app would never have handed it.
    ///
    /// The whole file lands in memory (a 90-minute service is ~350 MB as f32).
    /// That is affordable for an offline owner tool run on one machine, and
    /// streaming it would mean either two decode passes or holding the hop
    /// probabilities and the frames anyway.
    fn decode(ffmpeg: &str, path: &Path) -> Result<Vec<f32>, String> {
        let args = analysis_decode_args(&path.to_string_lossy());
        let output = Command::new(ffmpeg)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("could not run `{ffmpeg}`: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
            return Err(format!("ffmpeg failed ({}): {:?}", output.status, tail));
        }
        Ok(output
            .stdout
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect())
    }

    // ── The corpus on disk ─────────────────────────────────────────────────

    fn find_recordings(dirs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
        // Sorted and de-duplicated so two runs over the same folders produce
        // comparable reports; two `--dir` arguments that overlap must not weigh
        // a recording twice.
        let mut found: BTreeSet<PathBuf> = BTreeSet::new();
        for dir in dirs {
            let entries = std::fs::read_dir(dir)
                .map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
            for entry in entries.flatten() {
                let path = entry.path();
                let is_media = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .is_some_and(|e| MEDIA_EXTENSIONS.contains(&e.as_str()));
                if is_media && path.is_file() {
                    found.insert(path);
                }
            }
        }
        Ok(found.into_iter().collect())
    }

    /// Read `<stem>.feedback.json`, or an empty record when there is none.
    ///
    /// A file whose schema we do not recognise is reported LOUDLY rather than
    /// treated as absent: silently reading it as "no correction" would move a
    /// recording from the corrected corpus into the uncorrected one, which is
    /// exactly the direction that makes a detector look better than it is.
    fn read_feedback(media: &Path) -> RecordingFeedback {
        let Some(path) = media
            .parent()
            .zip(media.file_stem())
            .and_then(|(dir, stem)| {
                sidecar_path(
                    &dir.to_string_lossy(),
                    &stem.to_string_lossy(),
                    Sidecar::Feedback,
                )
            })
        else {
            return RecordingFeedback::default();
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return RecordingFeedback::default();
        };
        match serde_json::from_str::<RecordingFeedback>(&raw) {
            Ok(file) if file.schema == FEEDBACK_SCHEMA => file,
            Ok(file) => {
                eprintln!(
                    "WARNING: {path} has schema {} (expected {FEEDBACK_SCHEMA}); its corrections \
                     are being IGNORED, and this recording will count as uncorrected.",
                    file.schema
                );
                RecordingFeedback::default()
            }
            Err(e) => {
                eprintln!("WARNING: {path} will not parse ({e}); corrections IGNORED.");
                RecordingFeedback::default()
            }
        }
    }

    // ── Running the two scorers ────────────────────────────────────────────

    fn selection_of(detection: &Detection) -> ScorerSelection {
        let span = |s: sundayrec_core::detect::SermonSegment| {
            Span::new(s.start_sec, s.end_sec, Some(s.confidence))
        };
        ScorerSelection {
            strict: detection.sermon.map(span),
            offered: detection.offered.map(span),
            duration_sec: detection.duration_sec,
            segment_count: detection.segments.len(),
            attention_codes: detection
                .attention_codes()
                .iter()
                .map(|r| r.code().to_string())
                .collect(),
        }
    }

    /// The body of [`sundayrec_core::detect::analyse_pcm`] with the feature
    /// extraction hoisted out.
    ///
    /// Not an optimisation for its own sake: `extract_features` runs a
    /// 2048-point FFT per 100 ms frame, which is ~54 000 of them on a
    /// 90-minute service and the dominant cost of the whole run. Calling
    /// `analyse_pcm` once per scorer would repeat that work four times per
    /// recording for a three-rule sweep, to produce bit-identical frames every
    /// time — the frames do not depend on the scorer, which is the entire point
    /// of the seam.
    fn detect_with(pcm: &[f32], frames: &[AnalysisFrame], scorer: &dyn FrameScorer) -> Detection {
        let input = ScoringInput {
            pcm,
            sample_rate: SAMPLE_RATE,
            frame_ms: FRAME_MS,
            frames,
        };
        let grouped = classify_and_group_with(input, scorer);
        detect(grouped.iter().map(PrepAnalysisSegment::from).collect())
    }

    // ── The plumbing check ─────────────────────────────────────────────────

    /// Deterministic pseudo-random noise. A fixed sequence so two `--synthetic`
    /// runs produce the same report and a difference between them means a
    /// difference in the harness.
    fn noise(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / 8_388_608.0 - 1.0
    }

    /// Voice-like audio: source-filter synthesis, syllable-paced.
    ///
    /// Deliberately close to the generator `src-tauri/src/vad/mod.rs`'s own
    /// tests use, because a cruder one does not work and the way it fails is
    /// instructive. A first version here used bare impulses through PARALLEL
    /// resonators with a sinusoidal envelope; the heuristic called it speech and
    /// Silero scored it 0.001 — so the "plumbing check" exercised the VAD's
    /// found-nothing path only, while looking like a working comparison. Four
    /// things are load-bearing for the model to bite: a glottal pulse SHAPED
    /// over part of the period rather than a bare impulse (a bare one is far too
    /// bright), resonators in CASCADE rather than summed in parallel, formants
    /// that GLIDE between vowel targets, and broadband noise at each syllable
    /// onset standing in for consonants.
    ///
    /// Its job is to be audio both scorers will bite on so the comparison path
    /// runs at all. It is NOT a stand-in for a sermon, and nothing measured over
    /// it is evidence about detection quality — see this file's header.
    fn voice_like(seconds: f64, pitch_hz: f64, seed: &mut u32) -> Vec<f32> {
        /// (F1, F2, F3) for /a/, /i/, /u/, /e/, /o/ — adult-male values.
        const VOWELS: [[f64; 3]; 5] = [
            [730.0, 1090.0, 2440.0],
            [270.0, 2290.0, 3010.0],
            [300.0, 870.0, 2240.0],
            [530.0, 1840.0, 2480.0],
            [570.0, 840.0, 2410.0],
        ];
        const BANDWIDTHS: [f64; 3] = [80.0, 100.0, 140.0];

        let sr = SAMPLE_RATE as f64;
        let n = (seconds * sr) as usize;
        let syllable = (sr * 0.238) as usize;
        let onset = (sr * 0.025) as usize;
        let mut z1 = [0.0f64; 3];
        let mut z2 = [0.0f64; 3];
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(n);

        for i in 0..n {
            let t = i as f64 / sr;
            let index = i / syllable.max(1);
            let position = (i % syllable.max(1)) as f64 / syllable.max(1) as f64;
            let from = VOWELS[index % VOWELS.len()];
            let to = VOWELS[(index + 3) % VOWELS.len()];
            let glide = (position / 0.85).min(1.0);

            // Declination over the utterance, a per-syllable accent, and jitter:
            // a perfectly steady f0 is the most obvious way for synthetic speech
            // to stop sounding like any.
            let f0 = (pitch_hz - 14.0 * t / seconds.max(1e-9))
                * (1.0 + 0.06 * (std::f64::consts::PI * position).sin())
                * (1.0 + 0.01 * noise(seed) as f64);
            phase += f0 / sr;
            let fraction = phase.fract();
            let pulse = if fraction < 0.17 {
                (std::f64::consts::PI * fraction / 0.17).sin().powi(2) - 0.35
            } else {
                0.0
            };
            let voiced = if position < 0.85 { 1.0 } else { 0.0 };
            let mut x = if i % syllable.max(1) < onset {
                noise(seed) as f64 * 0.5
            } else {
                voiced * (pulse * 2.0 + noise(seed) as f64 * 0.02)
            };

            for k in 0..3 {
                let centre = from[k] + (to[k] - from[k]) * glide;
                let r = (-std::f64::consts::PI * BANDWIDTHS[k] / sr).exp();
                let theta = 2.0 * std::f64::consts::PI * centre / sr;
                let (a1, a2) = (2.0 * r * theta.cos(), -r * r);
                // `1 - a1 - a2` is the resonator's unity-gain form, applied to
                // the INPUT. Scaling the output by `1 - r` instead — which looks
                // equivalent and is not — attenuates by ~(1/57)³ through a
                // three-stage cascade. That is how the previous version of this
                // function ended up below the silence floor.
                let y = (1.0 - a1 - a2) * x + a1 * z1[k] + a2 * z2[k];
                z2[k] = z1[k];
                z1[k] = y;
                x = y;
            }
            // A short fade so the clip does not begin or end on a
            // discontinuity, which reads as a transient to both scorers.
            let fade = (t / 0.05).min(1.0).min(((seconds - t) / 0.05).max(0.0));
            out.push((x * 0.6 * fade).clamp(-1.0, 1.0) as f32);
        }
        out
    }

    /// A sustained chord — the prelude and postlude the sermon has to be picked
    /// out from.
    fn music_like(seconds: f64) -> Vec<f32> {
        let n = (seconds * SAMPLE_RATE as f64) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / SAMPLE_RATE as f64;
                let mut sample = 0.0;
                for harmonic in [220.0, 277.2, 329.6] {
                    sample += (2.0 * std::f64::consts::PI * harmonic * t).sin() / 3.0;
                }
                (sample * 0.3) as f32
            })
            .collect()
    }

    /// Write `count` fake services with fabricated corrections and return their
    /// paths.
    ///
    /// The corrections are FABRICATED. They exist so the truth-extraction,
    /// comparison and aggregation paths are exercised at all; every number
    /// derived from them is stamped [`sundayrec_core::ab_eval::SYNTHETIC_WARNING_CODE`]
    /// and must never be quoted as a measurement of anything.
    fn synthesize(dir: &Path, count: usize) -> Result<Vec<PathBuf>, String> {
        use sundayrec_core::wav::{encode_s16le, header, WavSpec};
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let spec = WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
        };
        let mut paths = Vec::new();
        for index in 0..count {
            let mut seed = 0x5EED_0000 + index as u32;
            // The talk starts past MIN_SERMON_START_SEC and runs past
            // MIN_SERMON_DURATION_SEC, so the STRICT pick has something to find;
            // otherwise every synthetic recording would exercise only the
            // "found nothing" path.
            let prelude = 310.0 + index as f64 * 5.0;
            let talk = 340.0 + index as f64 * 20.0;
            let mut pcm = music_like(prelude);
            pcm.extend(voice_like(talk, 128.0 + index as f64 * 7.0, &mut seed));
            pcm.extend(music_like(50.0));

            let mut bytes = Vec::new();
            encode_s16le(&pcm, &mut bytes);
            let path = dir.join(format!("synthetic-service-{index}.wav"));
            let mut file = std::fs::File::create(&path)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            let head = header(spec, bytes.len() as u32);
            file.write_all(&head)
                .and_then(|()| file.write_all(&bytes))
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;

            // A fabricated sermon-pick correction naming the talk, so the
            // recording lands in the CORRECTED corpus and the comparison path
            // runs. Every other recording is left uncorrected, so the weaker
            // category is exercised too.
            if index % 2 == 0 {
                let block = |start: f64, end: f64, kind| FeedbackSegment {
                    index: 1,
                    start_sec: start,
                    end_sec: end,
                    duration_sec: end - start,
                    kind,
                    confidence: Some(0.8),
                };
                let feedback = RecordingFeedback {
                    sermon_picks: vec![SermonPickCorrection {
                        auto: Some(block(0.0, prelude, FeedbackSegmentKind::Sermon)),
                        chosen: block(prelude, prelude + talk, FeedbackSegmentKind::Speech),
                        candidates: Vec::new(),
                        attention: Vec::new(),
                        recording_duration_sec: prelude + talk + 50.0,
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                    }],
                    ..Default::default()
                };
                let sidecar = dir.join(format!("synthetic-service-{index}.feedback.json"));
                std::fs::write(&sidecar, serde_json::to_string_pretty(&feedback).unwrap())
                    .map_err(|e| format!("cannot write {}: {e}", sidecar.display()))?;
            }
            paths.push(path);
        }
        Ok(paths)
    }

    // ── The run ────────────────────────────────────────────────────────────

    pub fn run() -> ExitCode {
        let options = match parse_args() {
            Ok(Some(options)) => options,
            Ok(None) => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Err(message) => {
                eprintln!("error: {message}\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
        };
        match execute(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::FAILURE
            }
        }
    }

    fn execute(options: Options) -> Result<(), String> {
        let mut dirs = options.dirs.clone();
        if let Some(count) = options.synthetic {
            let dir = std::env::temp_dir().join("sundayrec-vad-ab-synthetic");
            let made = synthesize(&dir, count)?;
            eprintln!(
                "synthesised {} recording(s) in {} — PLUMBING CHECK ONLY",
                made.len(),
                dir.display()
            );
            dirs.push(dir);
        }

        let ffmpeg = resolve_ffmpeg();
        let mut recordings = find_recordings(&dirs)?;
        if let Some(limit) = options.limit {
            recordings.truncate(limit);
        }
        eprintln!("ffmpeg: {ffmpeg}");
        eprintln!("recordings: {}", recordings.len());

        // One parse + optimize of the graph for the whole run; a fresh
        // `SileroVad` per file, because the recurrent state must not carry from
        // the end of one recording into the start of the next.
        let model = SileroVadModel::load().map_err(|e| format!("VAD model: {e}"))?;
        let app_version = env!("CARGO_PKG_VERSION");
        let heuristic_scorer = HeuristicScorer;

        // One accumulator per sweep row; the heuristic's side is identical
        // across rows and is recomputed into each so every row's report is
        // self-contained.
        let mut per_row: Vec<Vec<RecordingEvaluation>> =
            options.rows.iter().map(|_| Vec::new()).collect();

        for (n, path) in recordings.iter().enumerate() {
            let id = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            eprintln!("[{}/{}] {id}", n + 1, recordings.len());

            let pcm = decode(&ffmpeg, path)?;
            if pcm.is_empty() {
                eprintln!("  skipped: decoded to no audio");
                continue;
            }
            let frames = extract_features(&pcm, SAMPLE_RATE, FRAME_MS);
            let heuristic = selection_of(&detect_with(&pcm, &frames, &heuristic_scorer));

            // One inference pass over the file; every sweep row is arithmetic
            // over its output.
            let hops = score_pcm(SileroVad::new(Arc::clone(&model)), VAD_SAMPLE_RATE, &pcm)
                .map_err(|e| format!("{id}: {e}"))?;

            let feedback = read_feedback(path);
            let truths = ground_truths(&feedback, heuristic.strict, app_version);

            for (row_index, row) in options.rows.iter().enumerate() {
                let scorer = PooledVadScorer::new(&hops, *row);
                let vad = selection_of(&detect_with(&pcm, &frames, &scorer));
                per_row[row_index].push(evaluate_recording(
                    id.clone(),
                    heuristic.clone(),
                    vad,
                    Some(VadHopStats::of(&hops, row.frame_speech_threshold)),
                    truths.clone(),
                ));
            }
        }

        let report = AbReport {
            schema: AB_REPORT_SCHEMA,
            generated_at: chrono::Local::now().to_rfc3339(),
            app_version: app_version.to_string(),
            synthetic: options.synthetic.map(|_| true),
            min_corpus_for_any_conclusion: MIN_CORPUS_FOR_ANY_CONCLUSION,
            min_corpus_for_a_direction: MIN_CORPUS_FOR_A_DIRECTION,
            selection_iou_threshold: SELECTION_IOU_THRESHOLD,
            results: options
                .rows
                .iter()
                .zip(per_row)
                .map(|(row, evaluations)| aggregate(*row, evaluations))
                .collect(),
        };

        if let Some(path) = &options.json_out {
            let json = serde_json::to_string_pretty(&report)
                .map_err(|e| format!("cannot serialise the report: {e}"))?;
            std::fs::write(path, json)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        print!("{}", render_summary(&report));
        Ok(())
    }
}

#[cfg(feature = "vad")]
fn main() -> std::process::ExitCode {
    harness::run()
}

/// Without the `vad` feature there is no second scorer to compare against, so
/// there is no harness. It still has to COMPILE here — `npm run check` runs
/// `cargo clippy --workspace --all-targets -- -D warnings` with default
/// features, which builds every example, and dead code on either side of a
/// `cfg` is a hard failure there. Everything above therefore lives inside the
/// gated module, and this is the whole of the ungated half.
#[cfg(not(feature = "vad"))]
fn main() {
    println!(
        "vad_ab_eval does nothing without the `vad` feature. Run:\n  \
         cargo run --release --no-default-features --features vad --example vad_ab_eval -- --help"
    );
}
