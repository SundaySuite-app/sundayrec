//! Every number the sermon detector decides with — in one place, with what
//! moving it does (E10).
//!
//! # Read this before you change a value
//!
//! These constants decide, for a real church's real Sunday recording, where the
//! sermon starts and stops and whether a human is asked to look at it. There is
//! no separate "detector config": this file IS the detector's configuration, and
//! nothing else in the app holds a threshold it decides with.
//!
//! Three rules, in order of how much trouble breaking them causes:
//!
//! 1. **Nothing here may be changed without the golden tests going green
//!    afterwards.** `crates/sundayrec-core/tests/tuning_golden.rs` replays fixed
//!    recordings through the real pipeline and asserts the answers. A change
//!    that moves an answer WILL fail it, and the failure prints which recording
//!    and which boundary moved, in seconds. That is the point. Read the diff,
//!    decide whether the new answers are better, and only then re-bless.
//! 2. **One value per change.** Two moved constants produce one diff and no way
//!    to attribute it.
//! 3. **Write down why.** The "Provenance" line on each constant below is the
//!    only record that exists. Several of them say, honestly, that no reasoning
//!    was ever recorded — do not let yours join them.
//!
//! # Why this file exists at all
//!
//! Before E10 these numbers lived in two modules, half of them inlined at the
//! comparison that used them (`durationSec < 8 * 60`), and a constant nobody can
//! find is a constant nobody can tune. [`crate::audio_analysis`] and
//! [`crate::detect`] re-export from here, so every public path that ever worked
//! still works and there is exactly ONE definition of each value.
//!
//! # The shape of the pipeline these govern
//!
//! ```text
//!  PCM ──▶ frames ──▶ per-frame class ──▶ smooth ──▶ group ──▶ merge short
//!         (§Frame     (§Frame                (§Smoothing and grouping)
//!          geometry)   classification)
//!      ──▶ pick a sermon ──▶ flag for a human
//!          (§Sermon selection)  (§Attention)
//! ```
//!
//! Constants early in that chain are the dangerous ones: a frame-classification
//! threshold moves EVERY segment boundary in every recording, while a
//! sermon-selection threshold only changes which already-drawn block is chosen.
//!
//! # ⚠️ There is no trim-offset constant IN THIS TABLE, and you will come looking for one
//!
//! The most likely first finding from real corrections is a SKEW: "the proposed
//! trim starts consistently a few seconds too late". The instinct is then to
//! look down this list for the number that nudges it. **There isn't one here,
//! and nothing in this table is a substitute for it.**
//!
//! One offset does exist, and it is deliberately not in this table:
//! [`crate::local_adaptivity::DetectorTuning`] carries `sermon_start_offset_sec`
//! and `sermon_end_offset_sec`, both **shipped at zero**. They are moved by one
//! install's own corrections, within hard clamps, only when that install has
//! opted in — and they are reset to zero by
//! [`crate::local_adaptivity::SHIPPED_TUNING`]. They are NOT a knob for the
//! tuning ritual: this file is what every install starts from, and a number that
//! differs per install has no business being in it. If the fleet's corrections
//! say the detector opens late everywhere, the fix is below, not there —
//! nudging every install locally to paper over a shipped default would leave the
//! default wrong and make it invisible.
//!
//! The proposed trim is the chosen segment's own bounds, verbatim, on both
//! paths — [`crate::prep::build_episode_prep`] copies `start_sec`/`end_sec`
//! straight into `suggested_trim`, and [`crate::detect::promote_sermon`] hands
//! the same two numbers to the editor, which [`crate::editor::sermon_cut_regions`]
//! cuts to unchanged. (`editor::KEEP_EPSILON` is 0.05 s and only decides whether
//! a degenerate cut region is worth emitting at all; it never shifts a
//! boundary.) So between "where segmentation says the talk is" and "what the app
//! proposes" there is no padding, no lead-in, and no fudge factor of any kind.
//!
//! Which means a systematic skew has exactly two honest fixes:
//!
//! - move a constant that changes where the SEGMENT BOUNDARY falls — realistically
//!   [`SILENCE_DB`] (how much of the quiet lead-in counts as talk) or the
//!   classifier's feature windows; or
//! - or ship a non-zero default for the offset that
//!   [`crate::local_adaptivity::DetectorTuning`] already carries. The type exists;
//!   what does not exist is any argument for making it non-zero for everybody.
//!   That argument is the whole change, and it is a much larger claim than moving
//!   a threshold: it says the detector is right about where speech starts and the
//!   app should nonetheless propose something else.
//!
//! What it must NOT become is a reason to move [`MIN_SERMON_START_SEC`] — that
//! constant chooses WHICH block, and moving it to shave seconds off one
//! recording's start silently changes which twenty minutes get published on
//! every other recording. The per-constant notes below therefore say, for each
//! one, whether it can move a trim boundary at all.
//!
//! # A note on "confidence"
//!
//! The confidences here are not probabilities. They are a fixed table of six
//! numbers the frame classifier hands out by how many features agreed
//! (§Frame classification), averaged over the frames of a segment. So a segment
//! confidence of 0.8 means "somewhere between the 0.7 rung and the 0.9 rung on
//! average", not "80 % likely to be speech". [`ATTENTION_CONFIDENCE_THRESHOLD`]
//! is a threshold on THAT, which is why its provenance talks about rungs.

// ── Frame geometry ──────────────────────────────────────────────────────────
//
// These four fix the grid everything downstream is quantised to. They are the
// least tunable numbers in the file: changing one does not shift an answer
// slightly, it re-times every feature the classifier was calibrated against.

/// The sample rate the analyzer works at (Hz).
///
/// **Up/down:** this is not a quality knob. Every threshold in §Frame
/// classification is expressed per-second or in Hz and was calibrated at 16 kHz;
/// raising it moves the spectral centroid of the same audio (more spectrum above
/// the voice band is now representable) and changes the zero-crossing rate of
/// the same noise. Lowering it below ~8 kHz clips the consonant energy speech
/// detection depends on.
///
/// **Coupled:** the `src-tauri` shell passes this to ffmpeg's `-ar` when it
/// decodes, and [`crate::vad::VAD_SAMPLE_RATE`] is independently pinned to the
/// same 16 kHz because the Silero-class model requires it. Changing this one
/// silently de-syncs the two and the VAD shadow comparison stops comparing like
/// with like.
///
/// **Provenance:** inherited from the Electron `audio-analysis.ts`
/// ("Sample rate the analyzer asks ffmpeg to produce"), which chose it to match
/// what a speech model wants. Recorded.
pub const SAMPLE_RATE: u32 = 16000;

/// Frame length in milliseconds — the time quantum of every segment boundary.
///
/// **Up/down:** longer frames (200 ms) smear short pauses and make every
/// boundary coarser; shorter frames (50 ms) give a boundary you can trust to
/// finer than a syllable but halve the FFT frequency resolution the spectral
/// centroid is computed from, at fixed [`FFT_SIZE`] zero-padding.
///
/// **Coupled:** [`SMOOTH_HALF_WIN`] is expressed in FRAMES, so halving this
/// halves the smoothing window in seconds without anyone editing that constant.
/// If you change this, restate `SMOOTH_HALF_WIN` in the same commit.
///
/// **Provenance:** Electron `audio-analysis.ts`, recorded: "100 ms gives 10
/// frames/sec, a good compromise between time resolution (catch short pauses)
/// and the FFT frequency resolution we need to compute spectral centroid."
pub const FRAME_MS: u32 = 100;

/// Samples per frame — derived, never set by hand.
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE as usize * FRAME_MS as usize) / 1000;

/// FFT size. The frame is Hann-windowed and zero-padded up to this.
///
/// **Up/down:** larger only interpolates (the frame is 1600 real samples; the
/// rest is zeros), so it buys smoother bins, not more information, at more CPU
/// per frame over a 90-minute service. Smaller than 1600 would TRUNCATE the
/// frame — `spectrum` caps the copy at `FFT_SIZE`, so it would silently analyse
/// part of each frame. Must stay a power of two: `fft` asserts it.
///
/// **Provenance:** Electron, recorded: "2048 at 16 kHz gives ~7.8 Hz bin width
/// which is more than enough for centroid."
pub const FFT_SIZE: usize = 2048;

// ── Frame classification ────────────────────────────────────────────────────
//
// The one hard cut, then two feature "signatures" scored out of 4 and out of 3.
// See `crate::audio_analysis::classify_frame` for the score table these feed.
//
// ⚠️  Everything here changes the segment MAP, not just the pick. When one of
// these moves, expect the golden diff to show boundaries moving in recordings
// you were not thinking about.
//
// MOVES A TRIM BOUNDARY: yes — this section and §Smoothing and grouping are the
// ONLY constants that can. They decide where a segment starts and ends, and the
// proposed trim is a segment's bounds verbatim (see the module header). If a
// correction says the proposal starts too late, the answer is somewhere in these
// two sections or it is a new concept.

/// Silence threshold in dBFS. A frame quieter than this is `silence`, full stop
/// — before any feature is looked at.
///
/// **Up (toward 0, e.g. −35):** more of the room is called silence. Quiet
/// speech — a lapel mic on a soft speaker, a congregation praying — starts
/// disappearing from the speech map, which shortens sermons from the ends and
/// can split one talk into two.
///
/// **Down (e.g. −55):** room tone, HVAC and mic hiss stop being silence and get
/// handed to the feature classifier, which will call the noise floor `unknown`
/// or `music`. Long "silences" between items vanish from the map, and
/// [`MID_RECORDING_SILENCE_RUN_SEC`] stops firing on real dropouts.
///
/// **Provenance:** Electron, recorded: "−45 dB catches typical church-room noise
/// floor without classifying whispered speech as silence." No measurement of a
/// real room is on record behind it.
pub const SILENCE_DB: f64 = -45.0;

/// Confidence given to a frame that is only just below [`SILENCE_DB`].
///
/// **Provenance:** none recorded. Inherited from the Electron classifier, which
/// simply wrote `0.6 + 0.4 * margin`. It matters more than it looks: this floor
/// is EQUAL to [`ATTENTION_CONFIDENCE_THRESHOLD`], so a segment of borderline
/// silence sits exactly on the flag line.
pub const SILENCE_CONFIDENCE_FLOOR: f64 = 0.6;

/// How much confidence a deeply-silent frame gains over a borderline one, so
/// confidence runs `FLOOR ..= FLOOR + SPAN`.
///
/// **Provenance:** none recorded. `FLOOR + SPAN == 1.0` today; keep that true
/// unless you mean to emit a confidence above 1.
pub const SILENCE_CONFIDENCE_SPAN: f64 = 0.4;

/// How far below [`SILENCE_DB`] a frame must be to earn the full
/// [`SILENCE_CONFIDENCE_SPAN`] (dB).
///
/// **Up/down:** larger makes silence confidence rise more slowly with depth, so
/// more silence segments land near [`SILENCE_CONFIDENCE_FLOOR`]. Only affects
/// reporting and [`ATTENTION_CONFIDENCE_THRESHOLD`] on silence-heavy picks — it
/// never changes a frame's CLASS.
///
/// **Provenance:** none recorded.
pub const SILENCE_CONFIDENCE_FULL_MARGIN_DB: f64 = 10.0;

/// Lowest zero-crossing rate (crossings/second) still consistent with speech.
///
/// **Up:** voiced, low-pitched speech — a bass voice, a slow reading — stops
/// scoring the ZCR feature and drops from the 4/4 "clean speech" rung to 3/4, or
/// out of speech entirely if another feature also misses.
///
/// **Down:** overlaps further into [`MUSIC_ZCR_MAX_PER_SEC`], so sustained
/// instrumental tone scores BOTH signatures and lands as `mixed`.
///
/// **Provenance:** ⚠️ CONTRADICTED. The Electron doc-comment above this
/// classifier says the speech ZCR band is `[600, 6000]`; the code it documents
/// said `z >= 400`, and 400 is what shipped and what this port preserves. There
/// is no record of which was intended. Do not "fix" the code to match the
/// comment — that is a behaviour change wearing a typo's clothes, and the golden
/// tests will (correctly) fail on it.
pub const SPEECH_ZCR_MIN_PER_SEC: f64 = 400.0;

/// Highest zero-crossing rate still consistent with speech.
///
/// **Up/down:** the ceiling exists for unvoiced consonants (s, f, sh), which
/// push ZCR toward 6 k/s. Lowering it makes sibilant-heavy close-mic speech miss
/// the feature; raising it lets broadband noise (applause, a fan) score as
/// speech.
///
/// **Provenance:** Electron, recorded: "Unvoiced consonants push ZCR up to
/// ~6 k/s — we still allow these." No measurement recorded.
pub const SPEECH_ZCR_MAX_PER_SEC: f64 = 6000.0;

/// Low edge of the spectral-centroid band (Hz) counted as speech.
///
/// **Up/down:** raising it excludes bass-heavy or heavily proximity-affected
/// voices; lowering it lets low rumble and organ pedal notes score the feature.
///
/// **Provenance:** Electron, recorded only as "centroid in vocal band". No
/// measurement.
pub const SPEECH_CENTROID_MIN_HZ: f64 = 300.0;

/// High edge of the spectral-centroid band (Hz) counted as speech.
///
/// **Up:** bright instruments and cymbals start scoring the speech feature, so
/// worship bands drift toward `mixed`.
///
/// **Down:** thin/bright PA sound (a small horn-loaded speaker, an over-EQ'd
/// lectern mic) stops scoring, and the sermon loses its 4/4 rung.
///
/// **Provenance:** Electron, recorded only as "vocal band". No measurement.
pub const SPEECH_CENTROID_MAX_HZ: f64 = 3500.0;

/// Normalised spectral flux above which a frame looks like speech (unitless —
/// the L2 distance between consecutive UNIT-NORMALISED magnitude spectra; see
/// [`crate::audio_analysis::spectral_flux`]). Range `[0, √2]`: 0 means the
/// spectral shape did not move at all, √2 means it shares no energy with the
/// previous frame's.
///
/// **Level-invariant by construction.** Until 2026-08-08 flux was computed on
/// raw magnitudes (thresholds 8.0 speech / 6.0 music), so it grew with LEVEL:
/// the same sermon recorded ~10 dB hotter had ~3× the flux, and a tremolo
/// organ pad — spectral shape constant, only level moving — measured as
/// `speech` when hot and `music` 10 dB quieter
/// (`tests/flux_level_invariance.rs` pins that defect and its fix). The
/// normalisation removes the level axis entirely; what this constant now
/// thresholds is how fast the spectral SHAPE moves, which is the property that
/// actually separates syllabic speech from sustained music.
///
/// **Up:** only strongly modulated speech scores the feature — but no longer
/// only LOUD speech; gain staging cannot take the point away.
///
/// **Down:** toward [`MUSIC_FLUX_MAX`] and eventually past it, at which point
/// frames score BOTH flux features and the two signatures stop separating at
/// all. Keep `SPEECH_FLUX_MIN > MUSIC_FLUX_MAX` or the classifier's
/// discrimination collapses.
///
/// **Provenance:** derived 2026-08-08 from measurements over the synthetic
/// corpus in `tests/tuning_golden.rs`, taken at every gain from −20 to +10 dB
/// (the normalised value is identical at each, which is the point):
/// speech-like frames (syllabically modulated low-passed noise) measure
/// 0.565–0.753; broadband noise 0.62–0.70; a sustained chord ≤ 0.011;
/// sustained tone stacks ≤ 0.0005; a tremolo pad — the hardest sustained case,
/// its level moving but its shape fixed — ≤ 0.112. The gap between the highest
/// sustained-music value (0.112) and the lowest speech value (0.565) spans a
/// factor of ~5; 0.35 and [`MUSIC_FLUX_MAX`] = 0.2 divide it in roughly equal
/// log-thirds, leaving speech a 1.6× margin above this floor and the tremolo
/// pad 1.8× below the music ceiling. Caveat, recorded honestly: the corpus is
/// synthetic. These values reproduce every classification the old constants
/// gave on that corpus at its reference levels, but no real church recording
/// has validated them — the owner's listening test remains the real gate.
pub const SPEECH_FLUX_MIN: f64 = 0.35;

/// Low edge of the speech energy window (dBFS).
///
/// **⚠️ Currently INERT.** It is equal to [`SILENCE_DB`], and the silence branch
/// has already returned for anything below that, so `r >= SPEECH_ENERGY_MIN_DB`
/// is true for every frame that reaches the test — the speech-energy feature is
/// in practice `r <= SPEECH_ENERGY_MAX_DB` alone. Preserved rather than removed
/// because it stops being inert the moment [`SILENCE_DB`] is lowered, which is a
/// plausible tuning move. Raising THIS above `SILENCE_DB` makes it bite
/// immediately.
///
/// **Provenance:** Electron `r >= -45`, tied by hand to the silence threshold.
pub const SPEECH_ENERGY_MIN_DB: f64 = -45.0;

/// High edge of the speech energy window (dBFS) — above this a frame is too
/// loud to be talking.
///
/// **Up (toward 0):** a hot-mixed sermon that peaks near full scale keeps its
/// energy point. **Down:** loud speech loses it and drops a rung.
///
/// **Provenance:** Electron `r <= -5`. No reasoning recorded; −5 dBFS reads like
/// "just below clipping" rather than a measured speech level.
pub const SPEECH_ENERGY_MAX_DB: f64 = -5.0;

/// Zero-crossing rate below which a frame looks like music (crossings/second).
///
/// **Up:** more frames score the music ZCR point, including ordinary voiced
/// speech (whose ZCR overlaps this band), pushing sermons toward `mixed`.
///
/// **Down:** only very sustained low tone counts; bright or percussive music
/// stops scoring and can end up `unknown`.
///
/// **Provenance:** ⚠️ CONTRADICTED, same as [`SPEECH_ZCR_MIN_PER_SEC`]: the
/// Electron doc-comment says "ZCR < 1200 /sec or very stable", the code said
/// 1500 and had no notion of stability. 1500 shipped.
pub const MUSIC_ZCR_MAX_PER_SEC: f64 = 1500.0;

/// Normalised spectral flux below which a frame looks like music (same
/// unit-spectrum scale as [`SPEECH_FLUX_MIN`], and level-invariant the same
/// way — see there for the 2026-08-08 normalisation).
///
/// **Up toward [`SPEECH_FLUX_MIN`]:** the two flux features start overlapping
/// and frames score both. **Down:** only near-static tone counts as music, so
/// a live band with a drummer stops scoring the music flux point.
///
/// **Provenance:** derived 2026-08-08 with its speech twin, from the same
/// measurements (see [`SPEECH_FLUX_MIN`] for the full table). Sustained
/// musical content on the synthetic corpus measures ≤ 0.112 (tremolo pad, the
/// worst case) at every gain; 0.2 keeps a 1.8× margin above that, and speech's
/// floor (0.565) sits 2.8× above this ceiling. The GAP between 0.2 and 0.35 is
/// the classifier's entire flux discrimination, exactly as the old 6.0–8.0 gap
/// was — but where nothing recorded why that gap was 2 units wide, this one is
/// placed to split the measured no-man's-land between the corpus's most
/// speech-like sustained music and its least fluctuating speech in equal
/// log-steps. Same caveat as the twin: synthetic corpus, real church audio
/// still owed.
pub const MUSIC_FLUX_MAX: f64 = 0.2;

/// Energy above which a frame is loud enough to be music (dBFS).
///
/// **Up/down:** raising it makes quiet music (a solo acoustic prelude) miss the
/// energy point and score 2/3 instead of 3/3, demoting confident music (0.85) to
/// solid music (0.65). Lowering it toward [`SILENCE_DB`] makes nearly every
/// non-silent frame score it, so the point stops carrying information.
///
/// **Provenance:** Electron `r >= -40`, documented as part of "sustained tones /
/// chords: ... often higher energy". No measurement.
pub const MUSIC_ENERGY_MIN_DB: f64 = -40.0;

/// Confidence for a frame where all four speech features fit and music does not.
///
/// **Provenance:** none recorded. This is the top rung of a hand-written table,
/// not a calibrated probability. See the module header's note on what
/// "confidence" means here.
pub const CONF_SPEECH_ALL_FOUR: f64 = 0.9;

/// Confidence for a frame where all three music features fit and speech scores
/// at most 2. **Provenance:** none recorded.
pub const CONF_MUSIC_ALL_THREE: f64 = 0.85;

/// Confidence for "solid speech" — 3 of 4 features, beating the music score.
///
/// **Provenance:** none recorded for the value itself, but it is the anchor
/// [`ATTENTION_CONFIDENCE_THRESHOLD`] was chosen relative to. Moving this
/// without restating that one detaches the flag line from what it meant.
pub const CONF_SPEECH_SOLID: f64 = 0.7;

/// Confidence for "solid music" — ≥2 of 3 features, speech not dominating.
/// **Provenance:** none recorded.
pub const CONF_MUSIC_SOLID: f64 = 0.65;

/// Confidence for a frame that partially fits both signatures.
/// **Provenance:** none recorded.
pub const CONF_MIXED: f64 = 0.5;

/// Confidence for a frame that fits neither — transient noise, clicks, a cough.
/// **Provenance:** none recorded.
pub const CONF_UNKNOWN: f64 = 0.3;

// ── Smoothing and grouping ──────────────────────────────────────────────────
//
// MOVES A TRIM BOUNDARY: yes. Smoothing shifts where a class transition is
// declared; merging can make a boundary disappear entirely by absorbing the
// block it belonged to.

/// Half-width, IN FRAMES, of the mode filter over the frame-class sequence.
/// At [`FRAME_MS`] = 100 this is ±0.5 s, an 1.1 s window.
///
/// **Up:** stronger smoothing. Short real events — a 3-second spoken
/// introduction between two songs, a brief pause — get outvoted and disappear
/// from the map entirely. Genuine transitions also shift by up to this many
/// frames.
///
/// **Down (0 disables it):** single-frame classifier noise survives into the
/// segment map, producing many tiny segments. Most of them are then eaten by
/// [`MIN_SEGMENT_SEC`], so the visible effect is smaller than it sounds — which
/// is exactly why this constant needs a golden test rather than an argument.
///
/// **Coupled:** expressed in frames, so it silently follows [`FRAME_MS`].
///
/// **Provenance:** Electron, recorded: "5 frames either side ≈ ±0.5 s = a 1.1 s
/// window total. Removes single-frame outliers without blurring real
/// transitions."
pub const SMOOTH_HALF_WIN: usize = 5;

/// Shortest segment that survives on its own (seconds). Anything shorter is
/// merged into its longer neighbour, repeatedly, until nothing short is left.
///
/// **Up:** coarser chapters. A genuine 20-second announcement between two hymns
/// is absorbed into the music around it and stops existing — including for
/// [`SERMON_ONLY_MUSIC_RATIO`], which counts absorbed music as speech.
///
/// **Down:** finer chapters, more of them, and more chances for the sermon to be
/// split into several speech blocks — which matters because
/// [`MIN_SERMON_DURATION_SEC`] is applied to a SINGLE block outside the
/// sermon-only case.
///
/// **Provenance:** Electron, recorded: "5 s matches the granularity the editor
/// UI expects (chapters are sermon/hymn-sized chunks, not phrase-sized)."
pub const MIN_SEGMENT_SEC: f64 = 5.0;

/// Convergence bound on the short-segment merge loop.
///
/// **Not a tuning knob — a safety bound.** Each pass strictly reduces the number
/// of segments, so termination is guaranteed; this caps the pathological case.
/// If a recording ever needed more than 10 passes it would exit with short
/// segments still present, which is a bug to investigate, not a number to raise.
///
/// **Provenance:** Electron `mergeShortSegments`, ported as-is.
pub const MERGE_MAX_PASSES: usize = 10;

// ── Sermon selection ────────────────────────────────────────────────────────
//
// MOVES A TRIM BOUNDARY: no — and this is the distinction that matters when a
// correction reports a skew. These do not move any boundary; they choose which
// already-drawn speech block is called the sermon. Their failure mode is "the
// right recording, the wrong twenty minutes", never "the right twenty minutes,
// six seconds late". Moving one to chase a few seconds is how a small skew
// becomes a large, invisible one somewhere else.
//
// The one exception is arithmetic, not tuning: the sermon-only shape below picks
// first-speech → last-speech, so whether it fires changes the bounds wholesale.

/// Earliest start (seconds) a speech block may have and still be considered the
/// sermon under the strict gates.
///
/// **Up:** more of the service's opening is excluded. A church that starts with
/// a 3-minute welcome and goes straight into a 40-minute teaching loses its
/// sermon from the strict search entirely and gets `no_sermon_block` — the
/// editor still offers the block, the review queue flags it.
///
/// **Down (0 disables it):** the announcements/welcome become eligible. On a
/// service where the welcome happens to be the longest single speech block,
/// this picks the welcome.
///
/// **Interacts:** `SermonPolicy::BestGuess` relaxes THIS floor first (see
/// [`crate::detect::SermonPolicy`]), so lowering it makes the strict and relaxed
/// answers converge — and the review queue stops being able to disagree with the
/// editor, which is the mechanism that surfaces odd services.
///
/// **Provenance:** ⚠️ intent only, no reasoning. Electron says: "Skips the
/// worship/prayer prelude that opens most services." No survey of how long that
/// prelude actually is in the churches running this is on record. A prime
/// candidate for the first data-driven change.
pub const MIN_SERMON_START_SEC: f64 = 5.0 * 60.0;

/// Shortest single speech block that may be called a sermon (seconds).
///
/// **Up:** short-form services — a 10-minute devotion, a children's talk, a
/// midweek prayer meeting — stop producing a strict pick and start being flagged.
///
/// **Down:** a long Scripture reading or a testimony can out-qualify the actual
/// sermon if the sermon got split into shorter blocks by
/// [`MIN_SEGMENT_SEC`]-surviving music/silence in the middle.
///
/// **Provenance:** ⚠️ none recorded at all. Electron states the rule ("Shortest
/// segment we'll consider a 'real' sermon") and never the reason for 3 minutes.
pub const MIN_SERMON_DURATION_SEC: f64 = 3.0 * 60.0;

/// Recordings at or below this length (seconds) skip the "sermon-only" shape
/// entirely.
///
/// **Purpose:** a 30-second clip that is all speech is not a sermon. Without
/// this guard the ratio test below would fire on any short voice memo.
///
/// **Up:** short sermon-only recordings fall through to the ordinary gates,
/// which they cannot pass ([`MIN_SERMON_START_SEC`] alone rules out anything
/// under 5 minutes), so they become `no_sermon_block`. **Down:** ever shorter
/// clips get treated as whole sermons.
///
/// **Provenance:** recorded, in the commit that introduced the shape (v4.36.1):
/// the rule is "≥80 % speech AND <5 % music AND total > 60 s", added for churches
/// that record only the sermon.
pub const SERMON_ONLY_MIN_DURATION_SEC: f64 = 60.0;

/// Speech share of the whole recording at or above which it looks like nothing
/// but the sermon, so the pick becomes first-speech → last-speech.
///
/// **Up (toward 1.0):** sermon-only recordings with a little dead air at the
/// start, or an "amen" song at the end, stop qualifying and fall back to the
/// longest-single-block search — which on such a file typically picks a
/// fragment of the talk instead of the talk.
///
/// **Down (toward 0.5):** ordinary full services start qualifying, and then the
/// pick spans from the first word of the welcome to the last word of the
/// benediction — i.e. auto-trim stops trimming.
///
/// **Provenance:** recorded (v4.36.1). The 80/5 pair was chosen so that "full
/// gudstjeneste med musikk + preken" would NOT break the ratio and would fall
/// back to the ordinary search. Verified at the time against three shapes
/// (a 25-minute continuous sermon split into 3 blocks by breathing pauses;
/// a 23-minute sermon with silent edges; a full service). That is real evidence,
/// on three examples.
pub const SERMON_ONLY_SPEECH_RATIO: f64 = 0.80;

/// Music share of the whole recording at or above which the sermon-only shape is
/// rejected. Paired with [`SERMON_ONLY_SPEECH_RATIO`] — both must hold.
///
/// **Up:** a sermon-only recording that opens with 3 minutes of underscoring
/// music still qualifies. **Down (toward 0):** any music at all — a single
/// misclassified frame group surviving [`MIN_SEGMENT_SEC`] — disqualifies it.
///
/// **Provenance:** recorded (v4.36.1), same evidence as its speech twin.
pub const SERMON_ONLY_MUSIC_RATIO: f64 = 0.05;

// ── Attention: when a human is asked to look ────────────────────────────────
//
// MOVES A TRIM BOUNDARY: no. These change nothing about the cut. They decide
// whether an episode lands in the review queue as `needs-attention` and what it
// says. Too loose and the
// queue is noise nobody reads; too tight and a wrong cut ships silently. Both
// failure modes are bad and only one of them is visible.

/// Segment confidence below which the pick is flagged `low_confidence`.
///
/// **Up:** more episodes flagged. At 0.7 every pick that is not "all four
/// features agreed, on average, across the whole block" is flagged — which on
/// real material is most of them, and a queue that flags everything is a queue
/// nobody triages.
///
/// **Down:** fewer flags; per the Electron note, below 0.6 "starts pulling in
/// music-heavy worship sets and prayer meetings" as unflagged picks.
///
/// **Coupled:** this is a threshold on the AVERAGE of the fixed confidence table
/// in §Frame classification. It sits between [`CONF_MUSIC_SOLID`] (0.65) and
/// [`CONF_MIXED`] (0.5), and equals [`SILENCE_CONFIDENCE_FLOOR`] exactly.
///
/// **Provenance:** recorded, and the best-argued number in this file. Electron:
/// "0.6 chosen because the per-frame classifier emits 0.7 confidence for 'solid
/// speech' (3 of 4 features fit) and we want at least that level for a sermon
/// candidate. After mean-aggregating across many frames, 0.6 lands roughly at
/// 'two-thirds of the segment's frames passed the solid-speech bar'."
pub const ATTENTION_CONFIDENCE_THRESHOLD: f64 = 0.6;

/// Recordings must be longer than this (seconds) before the mid-recording
/// silence check runs at all.
///
/// **Purpose:** on a short file the "middle" carved out by
/// [`MID_SILENCE_EDGE_GUARD_SEC`] is degenerate, and a short file has its own
/// reason ([`VERY_SHORT_RECORDING_SEC`]).
///
/// **Provenance:** ⚠️ none recorded — it was inlined as `durationSec > 5 * 60`
/// in the Electron source with no comment. It coincides numerically with
/// [`MIN_SERMON_START_SEC`], and nothing says whether that is meaningful or a
/// coincidence of two people reaching for "five minutes".
pub const MID_SILENCE_MIN_RECORDING_SEC: f64 = 5.0 * 60.0;

/// How much of each end of the recording is exempt from the silence check
/// (seconds). Silence in the first or last two minutes is normal — a room
/// filling up, a room emptying.
///
/// **Up:** a larger exempt zone, so real dropouts near the edges stop being
/// reported. **Down:** ordinary pre-roll and post-roll silence starts counting
/// toward [`MID_RECORDING_SILENCE_RUN_SEC`] and flags healthy recordings.
///
/// **Provenance:** ⚠️ none recorded — inlined as `120` twice in the Electron
/// source, described only as "after the first 2 min and before the last 2 min".
pub const MID_SILENCE_EDGE_GUARD_SEC: f64 = 120.0;

/// Total silence within the guarded middle (seconds) above which the recording
/// is flagged `mid_silence`.
///
/// **Note the shape:** this is a SUM over the middle, not the longest continuous
/// run, despite the name it inherited. Twelve five-second pauses trip it exactly
/// as one 60-second dropout does. That is a real weakness of the check and worth
/// fixing before tuning the number — but fixing it is a behaviour change and
/// belongs in its own reviewed commit.
///
/// **Up:** only gross dropouts are reported. **Down:** ordinary reflective
/// pauses in a slow service trip it.
///
/// **Provenance:** ⚠️ none recorded. Electron: "Minimum mid-recording silence run
/// before we suspect editing/dropouts" — and, as above, it does not measure a
/// run.
pub const MID_RECORDING_SILENCE_RUN_SEC: f64 = 60.0;

/// Music share of the recording above which it is flagged `mostly_music`
/// ("is this a concert rather than a service?").
///
/// **Up:** only overwhelmingly musical recordings are questioned. **Down:** a
/// normal service with a long worship set — which is most Pentecostal and
/// charismatic services, and this app's users include them — gets flagged every
/// single week, and a weekly false flag trains the operator to ignore the queue.
///
/// **Provenance:** ⚠️ none recorded. Electron declares `0.5` with the comment
/// "Minimum total music duration before we suspect this is a concert" and no
/// reasoning. Strictly greater-than, so an exactly-half-music service does not
/// flag.
pub const CONCERT_MUSIC_RATIO_THRESHOLD: f64 = 0.5;

/// Recordings shorter than this (seconds) are flagged `very_short`.
///
/// **Up:** more recordings questioned — at 15 minutes, every midweek devotion is
/// flagged. **Down:** genuinely truncated recordings (a crash, a full disk) stop
/// being questioned, and a truncated service is exactly the case where a human
/// most needs to look.
///
/// **Provenance:** ⚠️ none recorded — inlined as `durationSec < 8 * 60` with the
/// comment "almost certainly not a normal service". No survey behind "8".
pub const VERY_SHORT_RECORDING_SEC: f64 = 8.0 * 60.0;

#[cfg(test)]
// `assertions_on_constants` fires on every assertion here, and here it is
// pointing at the feature rather than a defect: these tests exist precisely to
// assert RELATIONSHIPS BETWEEN CONSTANTS, which are by construction
// compile-time-known. The lint's usual target — `assert!(true)` as leftover
// scaffolding — is the opposite of this. Nothing else in the crate is silenced.
#[allow(clippy::assertions_on_constants)]
mod tests {
    use super::*;

    /// The relationships the classifier's discrimination actually rests on. A
    /// tuning pass that breaks one of these does not shift an answer, it removes
    /// the mechanism that produces answers — and it would otherwise do so
    /// silently, because every individual value still looks reasonable.
    #[test]
    fn the_invariants_between_constants_still_hold() {
        assert!(
            SPEECH_FLUX_MIN > MUSIC_FLUX_MAX,
            "the flux windows have collided: a frame now scores both signatures, \
             and speech/music stop separating on flux at all"
        );
        assert!(
            SPEECH_ENERGY_MAX_DB > SPEECH_ENERGY_MIN_DB,
            "the speech energy window is empty — nothing can score the energy point"
        );
        assert!(
            SPEECH_CENTROID_MAX_HZ > SPEECH_CENTROID_MIN_HZ,
            "the speech centroid window is empty"
        );
        assert!(
            SPEECH_ZCR_MAX_PER_SEC > SPEECH_ZCR_MIN_PER_SEC,
            "the speech ZCR window is empty"
        );
        assert!(
            SPEECH_ENERGY_MIN_DB >= SILENCE_DB,
            "the speech energy floor is below the silence cut, so it is not merely \
             inert — it is unreachable"
        );
        assert!(
            MUSIC_ENERGY_MIN_DB >= SILENCE_DB,
            "the music energy floor is below the silence cut"
        );
        assert!(
            SILENCE_CONFIDENCE_FLOOR + SILENCE_CONFIDENCE_SPAN <= 1.0,
            "a silent frame can now report a confidence above 1"
        );
        assert!(
            SERMON_ONLY_SPEECH_RATIO > SERMON_ONLY_MUSIC_RATIO,
            "the sermon-only shape can no longer be satisfied by any recording"
        );
        assert!(
            (0.0..=1.0).contains(&ATTENTION_CONFIDENCE_THRESHOLD),
            "the low-confidence flag is outside the range confidences live in, so it \
             fires always or never"
        );
        assert!(
            MIN_SERMON_DURATION_SEC > 0.0 && MIN_SERMON_START_SEC >= 0.0,
            "a negative sermon gate is not a relaxation, it is a bug"
        );
        assert!(
            MID_SILENCE_EDGE_GUARD_SEC * 2.0 < MID_SILENCE_MIN_RECORDING_SEC,
            "the two edge guards now meet or overlap inside the shortest recording \
             the mid-silence check runs on, so the check has no middle to look at"
        );
        assert!(
            FRAME_SAMPLES <= FFT_SIZE,
            "a frame no longer fits the FFT and `spectrum` would silently analyse \
             only part of it"
        );
        assert!(
            FFT_SIZE.is_power_of_two() && FFT_SIZE >= 2,
            "`fft` asserts a power-of-two size and would panic on every frame"
        );
        assert_eq!(
            FRAME_SAMPLES,
            (SAMPLE_RATE as usize * FRAME_MS as usize) / 1000,
            "FRAME_SAMPLES was set by hand instead of derived"
        );
    }

    /// The confidence table must stay ORDERED, because
    /// [`ATTENTION_CONFIDENCE_THRESHOLD`] is a cut through it: reordering two
    /// rungs silently changes which classifications are considered good enough
    /// without touching the threshold itself.
    #[test]
    fn the_confidence_rungs_are_still_in_order() {
        let rungs = [
            ("unknown", CONF_UNKNOWN),
            ("mixed", CONF_MIXED),
            ("music solid", CONF_MUSIC_SOLID),
            ("speech solid", CONF_SPEECH_SOLID),
            ("music all three", CONF_MUSIC_ALL_THREE),
            ("speech all four", CONF_SPEECH_ALL_FOUR),
        ];
        for w in rungs.windows(2) {
            assert!(
                w[0].1 < w[1].1,
                "confidence rungs out of order: {} ({}) is not below {} ({})",
                w[0].0,
                w[0].1,
                w[1].0,
                w[1].1
            );
        }
        for (name, v) in rungs {
            assert!(
                (0.0..=1.0).contains(&v),
                "{name} confidence {v} is outside 0..=1"
            );
        }
        assert!(
            CONF_MIXED < ATTENTION_CONFIDENCE_THRESHOLD
                && ATTENTION_CONFIDENCE_THRESHOLD < CONF_SPEECH_SOLID,
            "the low-confidence flag has drifted off the rung it was argued from: it \
             is meant to sit between `mixed` and `solid speech`"
        );
    }
}
