//! Mastering presets — the pure half (P2a, rewritten in B1b).
//!
//! A preset is a named loudness target plus the tone/dynamics chain that precedes it. The chain
//! runs in ffmpeg (or, from B2, in [`crate::chain`]); the level is set in Rust:
//! [`crate::loudness`] measures the processed signal, plans ONE linear gain to the preset's
//! target and puts a true-peak limiter after it only when the peaks need it, and
//! [`crate::pcm_stream`] runs that over a stream too long to hold in memory.
//!
//! What lives here: the preset table, the 16-bit dither table, `-progress` parsing, the preview
//! window clamps and ffmpeg argv, and the in-flight job registry. ffmpeg no longer sets the
//! level (B1b): the old two-pass measure/apply strings, their JSON parser, the pass-2 plan and
//! the read-back of ffmpeg's report are gone.

/// A mastering preset — a named target loudness + the ffmpeg filter chain that
/// precedes the level setting. Mirrors `mastering.ts` `MasterPreset`. `label` and
/// `description` are the Norwegian user-facing strings (kept verbatim).
#[derive(Debug, Clone, PartialEq)]
pub struct MasterPreset {
    pub id: String,
    pub label: String,
    pub description: String,
    /// Integrated LUFS target.
    pub target_lufs: f64,
    /// Loudness range target.
    pub target_lra: f64,
    /// Max true peak in dBTP.
    pub true_peak_db: f64,
    /// ffmpeg filter chain; the level is set in Rust, never here.
    pub filters: String,
}

/// The built-in presets, verbatim from `MASTER_PRESETS`. The four cover the
/// common church-service publishing targets (natural → punchy speech, plus a
/// dynamics-preserving music+speech chain).
pub fn master_presets() -> Vec<MasterPreset> {
    vec![
        MasterPreset {
            id: "speech-natural".into(),
            label: "Tale — naturlig".into(),
            description: "Lett polering. Bra for opptak som allerede er gode.".into(),
            target_lufs: -19.0,
            target_lra: 7.0,
            true_peak_db: -1.0,
            filters:
                "highpass=f=80,acompressor=threshold=-18dB:ratio=3:attack=5:release=50:makeup=2dB"
                    .into(),
        },
        MasterPreset {
            id: "speech-clear".into(),
            label: "Tale — tydelig (anbefalt)".into(),
            description:
                "Standard mastering for taler og prekener. Tydeligere stemme, jevnere lyd.".into(),
            target_lufs: -16.0,
            target_lra: 8.0,
            true_peak_db: -1.0,
            filters: "highpass=f=80,equalizer=f=200:t=q:w=2:g=-1.5,equalizer=f=3000:t=q:w=1:g=2,\
                      equalizer=f=7000:t=q:w=1.5:g=-2,\
                      acompressor=threshold=-18dB:ratio=2.5:attack=5:release=80:makeup=1.5dB"
                .into(),
        },
        MasterPreset {
            id: "speech-punchy".into(),
            label: "Tale — kraftig".into(),
            description: "For weak voices or noisy recordings. Stronger processing.".into(),
            target_lufs: -14.0,
            target_lra: 6.0,
            true_peak_db: -1.0,
            filters: "highpass=f=100,equalizer=f=200:t=q:w=2:g=-2,equalizer=f=2500:t=q:w=1:g=3,\
                      equalizer=f=7000:t=q:w=1.5:g=-3,\
                      acompressor=threshold=-24dB:ratio=4:attack=3:release=50:makeup=2dB,\
                      acompressor=threshold=-12dB:ratio=2:attack=50:release=300:makeup=1dB"
                .into(),
        },
        MasterPreset {
            id: "music-speech".into(),
            label: "Musikk + tale".into(),
            description: "For gudstjenester med salmer eller annen musikk. Bevarer dynamikk."
                .into(),
            target_lufs: -16.0,
            target_lra: 11.0,
            true_peak_db: -1.0,
            filters:
                "highpass=f=50,acompressor=threshold=-22dB:ratio=2:attack=10:release=100:makeup=1dB"
                    .into(),
        },
    ]
}

/// Find a preset by id. Mirrors `getPresetById`.
pub fn get_preset_by_id(id: &str) -> Option<MasterPreset> {
    master_presets().into_iter().find(|p| p.id == id)
}

/// The triangular-dither resample, as one ffmpeg filter string.
///
/// `aresample`'s `dither_method` is part of core swresample and present in EVERY
/// ffmpeg build — unlike `resampler=soxr`, which depends on an optional
/// `--enable-libsoxr` and so is unsafe to bake into a path we can't probe. It
/// also folds the (possible) sample-RATE conversion into the same step, so a
/// graph with an internal sample rate reaches a 48 kHz 16-bit target in ONE
/// resample rather than two.
pub const DITHER_FILTER: &str = "aresample=osf=s16:dither_method=triangular";

/// The dither post-filter for an output format, or `None` when the target isn't
/// a 16-bit PCM one. THE shared table: both the mastering apply and the editor's
/// export ask this, so the two can never disagree about what gets dithered.
///
/// Why it matters: without it the internal float → 16-bit conversion folds
/// quantization distortion into the quiet passages a sermon is full of (pauses,
/// soft speech). This is what Audacity does on export to a lower bit depth.
///
/// The format set mirrors [`crate::editor::codec_args`]'s 16-bit PCM encoders:
/// WAV (unless 24-bit was asked for), the AIFF family (`pcm_s16be`), au/snd, and
/// the "no encoder available → transcode to `pcm_s16le`" set. Everything else is
/// lossless-at-source-depth (FLAC/WavPack/TTA keep the input's bit depth) or
/// lossy, where dithering before the encoder is pointless.
pub fn dither_filter_for(fmt: &str, bit_depth: Option<u8>) -> Option<String> {
    match fmt {
        // WAV is only 24-bit when explicitly asked for; anything else is s16.
        "wav" if bit_depth == Some(24) => None,
        "wav" | "aiff" | "aif" | "au" | "snd" => Some(DITHER_FILTER.to_string()),
        // No reliable encoder → `codec_args` transcodes these to `pcm_s16le`.
        "ape" | "dts" | "mpc" | "ra" | "ram" | "spx" | "gsm" => Some(DITHER_FILTER.to_string()),
        _ => None,
    }
}

// ── Progress parsing ───────────────────────────────────────────────────────────

/// Parse a current-time (seconds) from an ffmpeg `-progress` line, accepting
/// either `out_time_ms=` (microseconds, despite the name) or
/// `out_time=HH:MM:SS.ffffff`. Mirrors `parseProgressTime`.
pub fn parse_progress_time(line: &str) -> Option<f64> {
    for l in line.lines() {
        if let Some(rest) = l.strip_prefix("out_time_ms=") {
            if let Ok(us) = rest.trim().parse::<u64>() {
                return Some(us as f64 / 1_000_000.0);
            }
        }
    }
    for l in line.lines() {
        if let Some(rest) = l.strip_prefix("out_time=") {
            if let Some(sec) = parse_hms(rest) {
                return Some(sec);
            }
        }
    }
    None
}

/// Parse `HH:MM:SS.fff` → seconds.
fn parse_hms(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].parse().ok()?;
    let m: f64 = parts[1].parse().ok()?;
    let sec: f64 = parts[2].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Clamp a measure-preview snippet duration to the `[1, 60]` second band the
/// Electron `buildPreview` enforced, defaulting a non-finite request to 15 s.
pub fn clamp_preview_duration(requested: f64) -> f64 {
    let d = if requested.is_finite() {
        requested
    } else {
        15.0
    };
    d.clamp(1.0, 60.0)
}

/// Clamp a preview *start* second to `>= 0`, defaulting a non-finite request to
/// 0 — mirrors `buildPreview`'s `Math.max(0, Number.isFinite(startSec) ? startSec : 0)`.
pub fn clamp_preview_start(requested: f64) -> f64 {
    let s = if requested.is_finite() {
        requested
    } else {
        0.0
    };
    s.max(0.0)
}

/// ffmpeg args that decode a `[start, start+dur]` window of `input_path` and stream it as raw
/// f32 PCM on stdout, with NO audio filter (B2b: the preset's tone/dynamics chain runs in Rust,
/// `crate::chain`; the level is then set in Rust too). `-ss`/`-t` come BEFORE `-i` for an accurate
/// container-index seek; `start`/`dur` are formatted to 3 decimals. The seam supplies the
/// clamped values (via [`clamp_preview_start`]/[`clamp_preview_duration`]).
pub fn preview_decode_args(
    input_path: &str,
    start: f64,
    dur: f64,
    fmt: crate::pcm_stream::PcmFormat,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-nostdin",
        "-hide_banner",
        "-ss",
        &format!("{start:.3}"),
        "-t",
        &format!("{dur:.3}"),
        "-i",
        input_path,
        "-vn",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    args.extend(crate::pcm_stream::output_args(fmt));
    args
}

/// ffmpeg args that encode the gained preview PCM (stdin) to a temp mp3. The preview is what the
/// user A/Bs against the original, so it runs at 320k (near-transparent) — at 192k the user was
/// comparing the master to a LOSSY render, which made the mastering itself sound more
/// compressed than the actual export.
pub fn preview_encode_args(fmt: crate::pcm_stream::PcmFormat, out_path: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["-nostdin".into(), "-hide_banner".into()];
    args.extend(crate::pcm_stream::input_args(fmt));
    args.extend(
        ["-c:a", "libmp3lame", "-b:a", "320k", "-y", out_path]
            .into_iter()
            .map(String::from),
    );
    args
}

/// Filename prefix for mastering-preview temp files left in the OS temp dir. The
/// startup sweep (and [`is_preview_temp_name`]) match this. Mirrors the Electron
/// `sundayrec-master-preview-` prefix.
pub const PREVIEW_TEMP_PREFIX: &str = "sundayrec-master-preview-";

/// Whether a temp-dir entry name is a leftover mastering-preview mp3 the startup
/// sweep should delete. Mirrors `cleanupOldPreviews`'s `startsWith(prefix) &&
/// endsWith('.mp3')`.
pub fn is_preview_temp_name(name: &str) -> bool {
    name.starts_with(PREVIEW_TEMP_PREFIX) && name.ends_with(".mp3")
}

// ── In-flight job tracking (P1 parity) ──────────────────────────────────────────
//
// The Electron `applyMastering` / `editor.exportEdited` kept a `Map<jobId,
// ChildProcess>` so the UI could abort a long render by job id (`cancelMastering`
// / `cancelExport`). That state machine — register on start, drop on
// completion, "was this id actually tracked?" on cancel — is pure and tested
// here; the seam holds the real abort handles in a parallel map and only asks
// this registry whether the cancel/complete is legitimate.

use std::collections::HashSet as JobSet;

/// A pure registry of in-flight job ids. The seam owns the real process/abort
/// handles; this mirror answers "is `id` a live job?" so `cancel`/`complete`
/// return the same booleans the Electron `Map.has`/`Map.delete` did.
#[derive(Debug, Default, Clone)]
pub struct JobRegistry {
    active: JobSet<String>,
}

impl JobRegistry {
    /// A fresh, empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a starting job. Returns `false` if the id was already live (the
    /// caller should reject a duplicate job id rather than orphan the first).
    pub fn register(&mut self, id: &str) -> bool {
        self.active.insert(id.to_string())
    }

    /// Whether `id` is currently a live job.
    pub fn is_active(&self, id: &str) -> bool {
        self.active.contains(id)
    }

    /// Mark a job finished (success or failure). Returns whether it was tracked.
    pub fn complete(&mut self, id: &str) -> bool {
        self.active.remove(id)
    }

    /// Request a cancel: drop the id, returning whether it was live. Mirrors
    /// `cancelMastering`/`cancelExport` returning `false` for an unknown id
    /// (so the renderer shows "nothing to cancel" rather than a phantom success).
    pub fn cancel(&mut self, id: &str) -> bool {
        self.active.remove(id)
    }

    /// How many jobs are in flight (for diagnostics/tests).
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── presets ────────────────────────────────────────────────────────────────

    #[test]
    fn four_presets_with_unique_ids() {
        let presets = master_presets();
        assert_eq!(presets.len(), 4);
        let mut ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn lookup_by_id() {
        assert_eq!(get_preset_by_id("speech-clear").unwrap().target_lufs, -16.0);
        assert!(get_preset_by_id("nope").is_none());
    }

    // ── filter builders ──────────────────────────────────────────────────────────

    #[test]
    fn default_preset_compression_is_gentle() {
        // The recommended default must not over-squash: a single ~2.5:1 compressor
        // with modest makeup, NOT the old aggressive 3:1/+2 dB. (Subjective; the
        // exact values are RIGG-VERIFIER'd, this just guards against regressing to
        // the harsher defaults.)
        let clear = get_preset_by_id("speech-clear").unwrap();
        assert!(
            clear.filters.contains("ratio=2.5"),
            "speech-clear compressor should be gentle (2.5:1); got: {}",
            clear.filters
        );
        assert!(!clear.filters.contains("ratio=3"));
        // …and "modest makeup" has to mean 1.5 dB. `acompressor:makeup` is a
        // LINEAR factor, so the bare `makeup=1.5` this preset used to carry was
        // +3.52 dB, and `makeup=2` in speech-natural/punchy was +6.02 dB — the
        // exact over-processing the numbers were tuned down to avoid. The `dB`
        // suffix is what makes the written number the applied number.
        assert!(
            clear.filters.contains("makeup=1.5dB"),
            "makeup must carry the dB suffix or the value is a linear factor; got: {}",
            clear.filters
        );
        // The opt-in punchy preset stays aggressive but with less first-stage
        // makeup to curb pumping.
        let punchy = get_preset_by_id("speech-punchy").unwrap();
        assert!(punchy.filters.contains("ratio=4"), "punchy stays strong");
        assert!(
            !punchy.filters.contains("makeup=3"),
            "less makeup → less pumping"
        );
    }

    #[test]
    fn every_preset_makeup_is_written_in_db() {
        // F2-C-A: the whole app calls this value dB — the mixer slider says dB,
        // the DTO field is `comp_makeup_db`, the tuning notes in
        // docs/archive/NATT-LYD-VU-PREKEN say "makeup 3→2" meaning decibels. ffmpeg
        // reads a bare number as a linear factor in [1, 64], so a bare `2` is
        // +6.02 dB and a bare `0.5` is not quiet — it is an "out of range"
        // ERROR that kills the export. Every `makeup=` we ship must therefore
        // end in `dB`.
        for p in master_presets() {
            for frag in p.filters.split(',') {
                for arg in frag.split(':') {
                    let Some(v) = arg.strip_prefix("makeup=") else {
                        continue;
                    };
                    assert!(
                        v.ends_with("dB"),
                        "preset {} writes a bare linear makeup ({arg}); it must be dB-suffixed",
                        p.id
                    );
                    // And the dB number must stay inside what the linear range
                    // [1, 64] can express: 0 … 36.12 dB.
                    let db: f64 = v.trim_end_matches("dB").parse().expect("numeric makeup");
                    assert!(
                        (0.0..=36.0).contains(&db),
                        "preset {} makeup {db} dB is outside acompressor's [1, 64] linear range",
                        p.id
                    );
                }
            }
        }
    }

    #[test]
    fn dither_table_covers_every_16bit_pcm_target() {
        // 16-bit PCM targets dither …
        for fmt in ["wav", "aiff", "aif", "au", "snd", "dts", "gsm"] {
            assert_eq!(
                dither_filter_for(fmt, None).as_deref(),
                Some(DITHER_FILTER),
                "{fmt} encodes to 16-bit PCM → it must dither"
            );
        }
        // … a 24-bit WAV does not (no bit-depth reduction to dither) …
        assert!(dither_filter_for("wav", Some(24)).is_none());
        assert_eq!(
            dither_filter_for("wav", Some(16)).as_deref(),
            Some(DITHER_FILTER)
        );
        // … and neither do the lossless-at-source-depth or lossy targets.
        for fmt in [
            "flac", "wv", "tta", "mka", "mp3", "aac", "m4a", "opus", "ogg",
        ] {
            assert!(
                dither_filter_for(fmt, None).is_none(),
                "{fmt} must not dither"
            );
        }
    }

    // ── progress parsing ───────────────────────────────────────────────────────────

    #[test]
    fn progress_prefers_out_time_ms_microseconds() {
        assert_eq!(parse_progress_time("out_time_ms=12345678"), Some(12.345678));
    }

    #[test]
    fn progress_falls_back_to_hms() {
        assert_eq!(
            parse_progress_time("frame=1\nout_time=00:01:30.500000\nspeed=1x"),
            Some(90.5)
        );
    }

    #[test]
    fn progress_none_when_absent() {
        assert!(parse_progress_time("frame=1\nfps=30").is_none());
    }

    // ── preview clamp ─────────────────────────────────────────────────────────────

    #[test]
    fn preview_duration_clamps_to_band() {
        assert_eq!(clamp_preview_duration(0.5), 1.0);
        assert_eq!(clamp_preview_duration(120.0), 60.0);
        assert_eq!(clamp_preview_duration(30.0), 30.0);
        assert_eq!(clamp_preview_duration(f64::NAN), 15.0);
    }

    #[test]
    fn preview_start_clamps_to_non_negative() {
        assert_eq!(clamp_preview_start(-5.0), 0.0);
        assert_eq!(clamp_preview_start(12.5), 12.5);
        assert_eq!(clamp_preview_start(f64::NAN), 0.0);
    }

    // ── preview args ───────────────────────────────────────────────────────────────

    #[test]
    fn preview_decode_seeks_before_input_and_streams_raw_pcm() {
        use crate::pcm_stream::PcmFormat;
        let fmt = PcmFormat {
            sample_rate: 44_100,
            channels: 1,
        };
        let a = preview_decode_args("/rec/a.mp3", 12.3456, 20.0, fmt);
        let at = |s: &str| a.iter().position(|x| x == s).unwrap();
        assert!(at("-ss") < at("-i") && at("-t") < at("-i"));
        assert_eq!(a[at("-ss") + 1], "12.346");
        assert_eq!(a[at("-t") + 1], "20.000");
        assert!(!a.iter().any(|x| x == "-af"), "no audio filter: the chain is Rust");
        let joined = a.join(" ");
        assert!(joined.contains("-f f32le"));
        assert!(joined.contains("-ar 44100 -ac 1"));
        assert!(joined.ends_with("pipe:1"));
        assert!(!joined.contains("-filter_complex"));
    }

    #[test]
    fn preview_encode_reads_pcm_from_stdin_at_320k() {
        use crate::pcm_stream::PcmFormat;
        let fmt = PcmFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        let a = preview_encode_args(fmt, "/tmp/x.mp3").join(" ");
        assert!(a.contains("-f f32le -ar 48000 -ac 2 -i pipe:0"));
        assert!(a.contains("libmp3lame -b:a 320k"));
        assert!(a.ends_with("-y /tmp/x.mp3"));
    }

    // ── preview temp cleanup ─────────────────────────────────────────────────────────

    #[test]
    fn preview_temp_name_matches_prefix_and_mp3() {
        assert!(is_preview_temp_name(
            "sundayrec-master-preview-deadbeef.mp3"
        ));
        assert!(!is_preview_temp_name(
            "sundayrec-master-preview-deadbeef.wav"
        ));
        assert!(!is_preview_temp_name("other.mp3"));
    }

    // ── job registry ────────────────────────────────────────────────────────────────

    #[test]
    fn job_registry_register_tracks_and_rejects_duplicates() {
        let mut reg = JobRegistry::new();
        assert!(reg.register("job-1"));
        assert!(reg.is_active("job-1"));
        assert_eq!(reg.active_count(), 1);
        // A duplicate id is rejected (returns false) without orphaning the first.
        assert!(!reg.register("job-1"));
        assert_eq!(reg.active_count(), 1);
    }

    #[test]
    fn job_registry_cancel_returns_whether_live() {
        let mut reg = JobRegistry::new();
        reg.register("job-1");
        assert!(reg.cancel("job-1"));
        assert!(!reg.is_active("job-1"));
        // Cancelling an unknown / already-cancelled id reports false.
        assert!(!reg.cancel("job-1"));
        assert!(!reg.cancel("never-started"));
    }

    #[test]
    fn job_registry_complete_drops_the_job() {
        let mut reg = JobRegistry::new();
        reg.register("job-1");
        reg.register("job-2");
        assert!(reg.complete("job-1"));
        assert!(!reg.is_active("job-1"));
        assert!(reg.is_active("job-2"));
        assert_eq!(reg.active_count(), 1);
        // Completing twice is a no-op false.
        assert!(!reg.complete("job-1"));
    }
}
