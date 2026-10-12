//! The consolidated cpal stream layer: host open, fuzzy device find, format
//! negotiation, and ONE typed stream-builder set shared by the native capture
//! engine and the VU engine (`audio/vu.rs`).
//!
//! Everything here is portable cpal code — the platform differences are
//! confined to [`open_host`]'s Windows-only host ids. The decision half of
//! format negotiation ([`pick_input_config`]) is pure and unit-tested off
//! hardware; [`negotiate`] is the thin wrapper that feeds it real device data.
//!
//! The same split holds for stream faults (A3b): cpal 0.18 reports fatal errors
//! and advisories through one error callback; [`stream_fault`] maps its typed
//! `ErrorKind` onto the core's `StreamFault`, and [`classify_stream_error`] —
//! the one error-callback body every cpal stream here uses — asks the core what
//! that means. Advisories are logged and recording carries on; anything else is
//! handed to the caller's supervisor as a [`StreamFaultReport`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait};
// `Sample` provides the ergonomic `f32::from_sample(s)`; `FromSample` is the bound.
use cpal::{FromSample, Sample, SampleFormat};
use ringbuf::traits::Producer;
use ringbuf::HeapProd;
use sundayrec_core::audio::{observe_levels, MeterBanks};
use sundayrec_core::device_match::{find_best_device_match, find_exact_device_match, FfmpegDevice};
use sundayrec_core::stream_fault::{stream_fault_action, StreamFault, StreamFaultAction};

use crate::audio::asio::{route_frame, ChannelRoute};

/// Which cpal host to open. `Default` is the platform's default host —
/// CoreAudio on macOS, WASAPI on Windows — and is the arm the macOS native
/// engine and the VU engine use. The explicit Windows kinds mirror the historic
/// `cpal_capture` behavior (WASAPI as the dshow replacement, ASIO for pro
/// interfaces).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpalHostKind {
    /// The platform default host (CoreAudio on macOS, WASAPI on Windows).
    Default,
    Wasapi,
    Asio,
}

impl CpalHostKind {
    /// The `set_audio_engine` label for diagnose output.
    pub fn label(self) -> &'static str {
        match self {
            CpalHostKind::Default => {
                if cfg!(target_os = "macos") {
                    "coreaudio"
                } else {
                    "wasapi"
                }
            }
            CpalHostKind::Wasapi => "wasapi",
            CpalHostKind::Asio => "asio",
        }
    }
}

/// Open the requested cpal host. The explicit WASAPI/ASIO ids exist only on
/// Windows; `Default` works everywhere.
pub fn open_host(kind: CpalHostKind) -> Result<cpal::Host, String> {
    // Before ANY cpal WASAPI call: see `audio::com_anchor` for the
    // use-after-free this prevents (a no-op outside Windows).
    crate::audio::com_anchor::ensure();
    match kind {
        CpalHostKind::Default => Ok(cpal::default_host()),
        CpalHostKind::Wasapi => {
            #[cfg(windows)]
            {
                cpal::host_from_id(cpal::HostId::Wasapi)
                    .map_err(|e| format!("could not open WASAPI host: {e}"))
            }
            #[cfg(not(windows))]
            {
                Err("WASAPI host is Windows-only".to_string())
            }
        }
        CpalHostKind::Asio => {
            #[cfg(all(windows, feature = "asio"))]
            {
                cpal::host_from_id(cpal::HostId::Asio)
                    .map_err(|e| format!("could not open ASIO host: {e}"))
            }
            #[cfg(not(all(windows, feature = "asio")))]
            {
                Err("ASIO support not compiled in (build with --features asio)".to_string())
            }
        }
    }
}

/// Find an input device by name (empty → host default).
///
/// The stored name comes from the frontend's Web Audio label, which is NOT
/// guaranteed identical to cpal's device name — so we FUZZY-match it against
/// the cpal device list with the same `find_best_device_match` moat the ffmpeg
/// path uses (exact → substring → word-overlap), then open the matched device
/// by its exact cpal name. This also gives every spawn a fresh device lookup by
/// NAME — virtual devices (Teams, iPhone Continuity) reshuffle device *indices*
/// mid-session, which is why the ffmpeg path re-resolves per attempt too.
pub fn find_device(host: &cpal::Host, name: &str) -> Result<cpal::Device, String> {
    if name.is_empty() {
        return host
            .default_input_device()
            .ok_or_else(|| "no default input device".to_string());
    }
    let devices: Vec<cpal::Device> = host
        .input_devices()
        .map_err(|e| format!("listing input devices: {e}"))?
        .collect();
    let candidates: Vec<FfmpegDevice> = devices
        .iter()
        .filter_map(crate::audio::devices::device_name)
        .map(|n| FfmpegDevice::new(n, "cpal", None))
        .collect();
    let target = resolve_device_target(&candidates, name);
    devices
        .into_iter()
        .find(|d| crate::audio::devices::device_name(d).as_deref() == Some(target.as_str()))
        .ok_or_else(|| format!("input device not found: {name}"))
}

/// RECONNECT counterpart of [`find_device`]: open the input device named `name`
/// and nothing that merely resembles it (case-insensitive, trimmed — see
/// [`find_exact_device_match`]). `name` is the cpal name the first segment
/// actually opened. A miss is an `Err`, which the supervisor reads as "the
/// device is not back yet" and keeps backing off, instead of recording the rest
/// of the service from a different microphone.
pub fn find_device_exact(host: &cpal::Host, name: &str) -> Result<cpal::Device, String> {
    let devices: Vec<cpal::Device> = host
        .input_devices()
        .map_err(|e| format!("listing input devices: {e}"))?
        .collect();
    let candidates: Vec<FfmpegDevice> = devices
        .iter()
        .filter_map(crate::audio::devices::device_name)
        .map(|n| FfmpegDevice::new(n, "cpal", None))
        .collect();
    let target = find_exact_device_match(&candidates, name)
        .map(|d| d.name.clone())
        .ok_or_else(|| format!("input device not back: {name}"))?;
    devices
        .into_iter()
        .find(|d| crate::audio::devices::device_name(d).as_deref() == Some(target.as_str()))
        .ok_or_else(|| format!("input device not back: {name}"))
}

/// The DECISION half of [`find_device`]: which cpal device name should be opened
/// for the user's stored label?
///
/// Split out so it can be exercised without a sound card, and so the ffmpeg
/// pipe path (`recorder::cpal_capture`) resolves through exactly the same rule
/// as the native engine instead of a second copy of it.
///
/// Falls back to the requested name VERBATIM when nothing matches, so the caller
/// reports "input device not found: <what the user asked for>" rather than the
/// name of some unrelated device the fuzzy matcher happened to like least.
pub fn resolve_device_target(candidates: &[FfmpegDevice], requested: &str) -> String {
    find_best_device_match(candidates, requested)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| requested.to_string())
}

// ── The capture ring ─────────────────────────────────────────────────────────

/// Seconds of routed audio the capture ring holds between the real-time cpal
/// callback and the writer thread.
///
/// **Was 1 s; raised to 5 s (2026-08-10).** The ring is the ONLY thing standing
/// between a stalled writer and lost samples: the callback may never block, so
/// once the ring is full it drops whole frames and counts them. One second
/// survives a scheduler hiccup; it does NOT survive the stalls that actually
/// happen on a church PC — a spinning-rust fsync behind a virus scanner, a
/// Windows Defender scan of the growing WAV, a Time Machine snapshot. Ardour's
/// precedent is the same reasoning (its disk-thread buffer is sized in seconds,
/// not milliseconds), and the cost is trivial: at the worst case this app can
/// negotiate — 96 kHz **stereo** f32 — 5 s is 96 000 × 2 × 5 × 4 B = **3.66 MiB**.
///
/// The overrun counter's MEANING is unchanged: it still counts *samples dropped
/// by the RT callback*, and 0 still means "nothing was lost". Only the stall
/// length required to reach the first drop moves (1 s → 5 s).
pub const RING_SECONDS: usize = 5;

/// Floor for [`ring_capacity`], in f32 samples: [`RING_SECONDS`] of 96 kHz mono.
///
/// A device that negotiates a low rate and one channel would otherwise get a
/// ring measured in tens of kilobytes; the floor keeps every capture on the same
/// stall budget regardless of format. (Pre-2026-08-10 this was a bare `96_000`,
/// i.e. the same 1 s of 96 kHz mono — the floor scales with the constant so the
/// two can never drift apart.)
pub const MIN_RING_SAMPLES: usize = 96_000 * RING_SECONDS;

/// Capacity (in f32 samples) of the ring between the RT callback and the writer:
/// [`RING_SECONDS`] of ROUTED audio, never below [`MIN_RING_SAMPLES`].
///
/// One function so the capture engine (`native_capture::segment`) and the
/// pre-roll buffer (`native_capture::preroll`) cannot end up with different
/// cushions — they were two hand-inlined copies of the same expression.
pub fn ring_capacity(sample_rate: u32, out_channels: u16) -> usize {
    (sample_rate as usize)
        .saturating_mul(out_channels.max(1) as usize)
        .saturating_mul(RING_SECONDS)
        .max(MIN_RING_SAMPLES)
}

// ── Format negotiation ───────────────────────────────────────────────────────

/// One supported-config range, reduced to the plain values the pure picker
/// needs (mirrors `cpal::SupportedStreamConfigRange`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeInfo {
    pub channels: u16,
    pub min_rate: u32,
    pub max_rate: u32,
    pub format: SampleFormat,
}

/// The negotiated stream format, plus how it was arrived at (for the log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
    /// `"range-walk"` when picked from `supported_input_configs`,
    /// `"default-config"` when that walk yielded nothing usable.
    pub via: &'static str,
}

/// Conversion fidelity rank: prefer formats that lose nothing on the way to
/// f32. Unlisted formats rank at the bottom but still work (everything
/// converts via `FromSample`).
fn format_rank(f: SampleFormat) -> u8 {
    match f {
        SampleFormat::F32 => 6,
        SampleFormat::F64 => 5,
        SampleFormat::I32 => 4,
        SampleFormat::I24 => 3,
        SampleFormat::I16 => 2,
        SampleFormat::U16 => 1,
        _ => 0,
    }
}

/// Pure negotiation: pick (rate, channels, format) from the supported ranges.
///
/// Rules (owner-locked design):
/// 1. Channels = the MAX across all ranges — a digital mixer must expose every
///    input (the Qu-5's 32), not the 2 its default config may advertise.
/// 2. Rate: a user-forced rate is honoured when any max-channel range contains
///    it; otherwise the device's native rate (`default_rate`), clamped into the
///    closest range. (Auto = native rate — the anti-choppiness rule; forced
///    delivery rates are conformed offline by the delivery transcode.)
/// 3. Format: among ranges that can carry the chosen (channels, rate), prefer
///    F32 > F64 > I32 > I24 > I16 (conversion fidelity to our f32 pipeline).
///
/// Returns `None` when `ranges` is empty — the caller falls back to the
/// device's default config verbatim.
pub fn pick_input_config(
    ranges: &[RangeInfo],
    requested_rate: Option<u32>,
    default_rate: u32,
) -> Option<(u32, u16, SampleFormat)> {
    let max_ch = ranges.iter().map(|r| r.channels).max()?;
    let candidates: Vec<&RangeInfo> = ranges.iter().filter(|r| r.channels == max_ch).collect();
    // A forced rate only counts if a max-channel range actually contains it.
    let preferred = match requested_rate {
        Some(hz)
            if candidates
                .iter()
                .any(|r| (r.min_rate..=r.max_rate).contains(&hz)) =>
        {
            hz
        }
        _ => default_rate,
    };
    let best = candidates.iter().min_by_key(|r| {
        let rate = preferred.clamp(r.min_rate, r.max_rate);
        let dist = rate.abs_diff(preferred);
        // Closest achievable rate first; fidelity as the tie-breaker.
        (dist, u8::MAX - format_rank(r.format))
    })?;
    let rate = preferred.clamp(best.min_rate, best.max_rate);
    Some((rate, max_ch, best.format))
}

/// Negotiate a real device's input format: run the range walk through
/// [`pick_input_config`], falling back to `default_input_config()` verbatim
/// when the walk yields nothing.
///
/// **`default_input_config()` is the device's CURRENT rate on our three hosts,
/// also in cpal 0.18 (checked against 0.18.2, A3b).** cpal 0.18's changelog
/// says default configs "now prefer 48 kHz, then 44.1 kHz" and rank I32/I24
/// above I16. That heuristic (`try_with_standard_sample_rate`,
/// `cmp_default_heuristics`) is only used by backends that DERIVE a default from
/// ranges (AAudio, WebAudio, …). CoreAudio still reads the device's
/// `kAudioDevicePropertyStreamFormat`, WASAPI still returns `GetMixFormat`, and
/// ASIO still returns the driver's rate — the same sources as 0.17.3. So the
/// "auto = native rate" rule above is intact and nothing is pinned here. If a
/// later cpal moves one of those three onto the heuristic, the anti-choppiness
/// rule breaks silently (a 44.1 kHz mixer opened at 48 kHz, and on macOS cpal
/// would RE-CLOCK the device to get it) — re-check this on every cpal bump.
pub fn negotiate(
    device: &cpal::Device,
    requested_rate: Option<u32>,
) -> Result<NegotiatedFormat, String> {
    let default_cfg = device
        .default_input_config()
        .map_err(|e| format!("querying default input config: {e}"))?;
    let ranges: Vec<RangeInfo> = device
        .supported_input_configs()
        .map(|it| {
            it.map(|c| RangeInfo {
                channels: c.channels(),
                // SampleRate is a plain u32 (since cpal 0.17).
                min_rate: c.min_sample_rate(),
                max_rate: c.max_sample_rate(),
                format: c.sample_format(),
            })
            .collect()
        })
        .unwrap_or_default();

    match pick_input_config(&ranges, requested_rate, default_cfg.sample_rate()) {
        Some((sample_rate, channels, sample_format)) => Ok(NegotiatedFormat {
            sample_rate,
            channels,
            sample_format,
            via: "range-walk",
        }),
        None => Ok(NegotiatedFormat {
            sample_rate: default_cfg.sample_rate(),
            channels: default_cfg.channels(),
            sample_format: default_cfg.sample_format(),
            via: "default-config",
        }),
    }
}

// ── The one typed stream-builder set ─────────────────────────────────────────

/// Where a stream's samples go. One enum so the VU engine and the capture
/// engine share the same typed builders (previously duplicated in `vu.rs` and
/// `cpal_capture.rs`).
pub enum StreamSink {
    /// Meter-only (the VU engine): observe peak+RMS on every native channel.
    Meters(Arc<MeterBanks>),
    /// Capture: route the chosen channels, meter the ROUTED signal (what lands
    /// in the file — parity with ffmpeg's post-pan astats), and push into the
    /// ring. On overrun the newest block is dropped and counted — the RT
    /// thread never blocks.
    Capture {
        meters: Arc<MeterBanks>,
        plan: Vec<ChannelRoute>,
        prod: HeapProd<f32>,
        overrun: Arc<AtomicU64>,
    },
    /// Pre-roll: route the chosen channels into the ring exactly like
    /// [`StreamSink::Capture`], but meter the **native** frame (every input
    /// channel) instead of the routed one.
    ///
    /// The native pre-roll buffer doubles as the `vu://levels` emitter while it
    /// runs — it holds the device, so nothing else may open it — and the
    /// channel grid needs one meter per REAL input channel (a Qu-5's 32), not
    /// the one or two the recording routes. `meters` is therefore sized to the
    /// stream's native channel count.
    Preroll {
        meters: Arc<MeterBanks>,
        plan: Vec<ChannelRoute>,
        prod: HeapProd<f32>,
        overrun: Arc<AtomicU64>,
    },
}

/// Samples to pre-reserve in the callback's `conv`/`scratch` buffers. The driver's
/// block size is unknown (`BufferSize::Default`), so reserve a generous number of
/// FRAMES per channel: a fixed 4096 samples was outgrown by one block from a
/// 32-channel mixer (512 frames x 32 = 16384), putting a heap allocation on the
/// real-time audio thread. `Vec` still grows past this rather than dropping audio.
fn rt_scratch_capacity(total_channels: usize) -> usize {
    (total_channels.max(1) * 4096).max(4096)
}

/// One stream fault the capture loop must act on: the class the recovery
/// decision is made on, plus cpal's own words for the log and the warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFaultReport {
    pub fault: StreamFault,
    pub detail: String,
}

/// Map cpal 0.18's `ErrorKind` onto the core's [`StreamFault`]. Typed, never by
/// matching the message: cpal's `Display` text changed between 0.17 and 0.18
/// and will change again. `ErrorKind` is `#[non_exhaustive]`, so a kind added
/// by a later cpal lands on [`StreamFault::Other`] — the pre-A3b behaviour
/// (reconnect) — until someone classifies it here.
pub fn stream_fault(kind: cpal::ErrorKind) -> StreamFault {
    use cpal::ErrorKind as K;
    match kind {
        K::DeviceNotAvailable => StreamFault::DeviceNotAvailable,
        K::StreamInvalidated => StreamFault::StreamInvalidated,
        K::DeviceBusy => StreamFault::DeviceBusy,
        K::PermissionDenied => StreamFault::PermissionDenied,
        K::DeviceChanged => StreamFault::DeviceChanged,
        K::Xrun => StreamFault::Xrun,
        K::RealtimeDenied => StreamFault::RealtimeDenied,
        _ => StreamFault::Other,
    }
}

/// The ONE classification of a report from cpal's error callback, shared by
/// every cpal stream in the app (native capture, the native pre-roll, the
/// Windows cpal pipe, the VU meter). [`classify_stream_error_counting`] is the
/// same decision plus an xrun counter, used by the recording engine's stream.
///
/// cpal 0.18 delivers fatal errors AND advisories through the same callback.
/// What the report means is decided in the core
/// ([`sundayrec_core::stream_fault`]): `None` here means "the stream is still
/// running, keep recording" (`Xrun`, `RealtimeDenied`, `DeviceChanged`) and is
/// only logged; `Some` is a fault the caller must hand to its supervisor.
///
/// Called on cpal's callback thread, so it only logs and allocates the detail
/// string — never blocks.
pub fn classify_stream_error(context: &str, e: &cpal::Error) -> Option<StreamFaultReport> {
    classify_stream_error_counting(context, e, None)
}

/// [`classify_stream_error`] that also COUNTS the xruns cpal reports (A3c).
///
/// Since cpal 0.18 CoreAudio and WASAPI report a buffer under-/overrun through
/// the error callback as `ErrorKind::Xrun`. That is the real number of audible
/// glitch candidates the OS saw, and it replaces what the ffmpeg path derives
/// from stderr phrases. Only the recording engine's stream passes a counter;
/// the pre-roll, the VU meter and the ffmpeg-fed Windows pipe pass none, so a
/// glitch in a meter or in audio that never reaches a file is not charged to
/// the recording.
///
/// A glitching device can report one per buffer, so the log line is thinned:
/// the 1st, 2nd, 4th, 8th… xruns are logged and the rest only counted.
/// `RealtimeDenied` is deliberately NOT counted: it is one report at start-up
/// (the audio thread runs without real-time priority), not a glitch, and the
/// glitches it makes more likely arrive as `Xrun` and are counted then.
pub fn classify_stream_error_counting(
    context: &str,
    e: &cpal::Error,
    xruns: Option<&AtomicU64>,
) -> Option<StreamFaultReport> {
    let fault = stream_fault(e.kind());
    match stream_fault_action(fault) {
        StreamFaultAction::KeepRecording => {
            if let (StreamFault::Xrun, Some(counter)) = (fault, xruns) {
                let n = counter.fetch_add(1, Ordering::Relaxed).saturating_add(1);
                if !n.is_power_of_two() {
                    return None;
                }
                tracing::warn!(
                    ?fault,
                    xruns = n,
                    "{context} advisory (still recording): {e}"
                );
                return None;
            }
            tracing::warn!(?fault, "{context} advisory (still recording): {e}");
            None
        }
        action => {
            tracing::error!(?fault, ?action, "{context} error: {e}");
            Some(StreamFaultReport {
                fault,
                detail: e.to_string(),
            })
        }
    }
}

/// Build an input stream for sample type `T`: convert each sample to f32 via
/// cpal `from_sample` (covers i16/i32/f32/**I24**/u16/…) and feed the sink.
/// `conv`/`scratch` are allocated once here, never in the real-time callback.
fn build_typed<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    total: usize,
    mut sink: StreamSink,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: FromSample<T>,
{
    let cap = rt_scratch_capacity(total);
    let mut conv: Vec<f32> = Vec::with_capacity(cap);
    let mut scratch: Vec<f32> = Vec::with_capacity(cap);
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if total == 0 {
                return;
            }
            conv.clear();
            conv.extend(data.iter().map(|&s| f32::from_sample(s)));
            match &mut sink {
                StreamSink::Meters(m) => observe_levels(&conv, total, m),
                StreamSink::Capture {
                    meters,
                    plan,
                    prod,
                    overrun,
                } => {
                    route_block(plan, &conv, total, &mut scratch);
                    // Meter what LANDS IN THE FILE (parity with ffmpeg's
                    // post-pan astats).
                    observe_levels(&scratch, plan.len(), meters);
                    push_routed(&scratch, plan.len(), prod, overrun);
                }
                StreamSink::Preroll {
                    meters,
                    plan,
                    prod,
                    overrun,
                } => {
                    // Meter the NATIVE frame: the pre-roll is the `vu://levels`
                    // emitter while it runs and the grid draws every input
                    // channel. Routing into the ring is identical to Capture.
                    observe_levels(&conv, total, meters);
                    route_block(plan, &conv, total, &mut scratch);
                    push_routed(&scratch, plan.len(), prod, overrun);
                }
            }
        },
        err_fn,
        None,
    )
}

/// Apply the route plan to a whole interleaved block, into `scratch` (cleared
/// first). RT-safe once `scratch` has grown to the block size.
#[inline]
fn route_block(plan: &[ChannelRoute], conv: &[f32], total: usize, scratch: &mut Vec<f32>) {
    scratch.clear();
    for frame in conv.chunks_exact(total) {
        route_frame(plan, frame, scratch);
    }
}

/// FRAME-ALIGNED push into the ring: on overrun, drop whole frames only. A
/// partial frame in the ring would permanently swap the L/R interleaving for the
/// rest of the file. Never blocks — the dropped sample count is recorded in
/// `overrun` and surfaces as the capture's xrun telemetry.
#[inline]
fn push_routed(scratch: &[f32], out_ch: usize, prod: &mut HeapProd<f32>, overrun: &AtomicU64) {
    use ringbuf::traits::Observer;
    let out_ch = out_ch.max(1);
    let want = scratch.len();
    let fit = prod.vacant_len();
    let take = if fit >= want {
        want
    } else {
        (fit / out_ch) * out_ch
    };
    let pushed = prod.push_slice(&scratch[..take]);
    debug_assert_eq!(pushed, take, "aligned push must fit fully");
    if take < want {
        overrun.fetch_add((want - take) as u64, Ordering::Relaxed);
    }
}

/// Build an input stream for ANY sample format, dispatching to the typed
/// builder. `total` is the stream's native channel count (frames arrive
/// interleaved at this stride).
pub fn build_input_stream_any(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sample_format: SampleFormat,
    total: usize,
    sink: StreamSink,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String> {
    use cpal::SampleFormat as SF;
    let built = match sample_format {
        SF::I8 => build_typed::<i8>(device, config, total, sink, err_fn),
        SF::I16 => build_typed::<i16>(device, config, total, sink, err_fn),
        SF::I24 => build_typed::<cpal::I24>(device, config, total, sink, err_fn),
        SF::I32 => build_typed::<i32>(device, config, total, sink, err_fn),
        SF::U8 => build_typed::<u8>(device, config, total, sink, err_fn),
        SF::U16 => build_typed::<u16>(device, config, total, sink, err_fn),
        SF::F32 => build_typed::<f32>(device, config, total, sink, err_fn),
        SF::F64 => build_typed::<f64>(device, config, total, sink, err_fn),
        other => return Err(format!("unsupported input sample format: {other:?}")),
    };
    built.map_err(|e| format!("building input stream: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 32-channel mixer's 512-frame block (16384 samples) must fit the
    /// pre-reserved callback buffers — no allocation on the audio thread.
    #[test]
    fn callback_buffers_are_reserved_for_a_many_channel_block() {
        assert!(rt_scratch_capacity(32) >= 32 * 512);
        assert!(rt_scratch_capacity(2) >= 4096);
        assert!(rt_scratch_capacity(0) >= 4096);
    }

    const fn r(channels: u16, min_rate: u32, max_rate: u32, format: SampleFormat) -> RangeInfo {
        RangeInfo {
            channels,
            min_rate,
            max_rate,
            format,
        }
    }

    #[test]
    fn picks_max_channels_over_default_pair() {
        // Qu-5-like: the mixer exposes 32 channels; a stereo shadow range too.
        let ranges = [
            r(2, 44_100, 48_000, SampleFormat::I16),
            r(32, 44_100, 96_000, SampleFormat::F32),
        ];
        assert_eq!(
            pick_input_config(&ranges, Some(96_000), 48_000),
            Some((96_000, 32, SampleFormat::F32))
        );
    }

    #[test]
    fn auto_rate_uses_device_native() {
        let ranges = [r(2, 44_100, 96_000, SampleFormat::F32)];
        assert_eq!(
            pick_input_config(&ranges, None, 48_000),
            Some((48_000, 2, SampleFormat::F32))
        );
    }

    #[test]
    fn unavailable_forced_rate_falls_back_to_native() {
        // User forced 96 kHz but the max-channel range tops out at 48 kHz.
        let ranges = [r(8, 44_100, 48_000, SampleFormat::I24)];
        assert_eq!(
            pick_input_config(&ranges, Some(96_000), 44_100),
            Some((44_100, 8, SampleFormat::I24))
        );
    }

    #[test]
    fn native_rate_outside_range_clamps() {
        // Default config says 48 kHz but the only max-channel range is 88.2–96.
        let ranges = [r(4, 88_200, 96_000, SampleFormat::F32)];
        assert_eq!(
            pick_input_config(&ranges, None, 48_000),
            Some((88_200, 4, SampleFormat::F32))
        );
    }

    #[test]
    fn format_fidelity_breaks_ties() {
        let ranges = [
            r(2, 44_100, 96_000, SampleFormat::I16),
            r(2, 44_100, 96_000, SampleFormat::F32),
            r(2, 44_100, 96_000, SampleFormat::I32),
        ];
        assert_eq!(
            pick_input_config(&ranges, None, 48_000),
            Some((48_000, 2, SampleFormat::F32))
        );
        let int_only = [
            r(2, 44_100, 96_000, SampleFormat::I24),
            r(2, 44_100, 96_000, SampleFormat::I32),
        ];
        assert_eq!(
            pick_input_config(&int_only, None, 48_000),
            Some((48_000, 2, SampleFormat::I32))
        );
    }

    #[test]
    fn closest_rate_wins_over_format() {
        // One range can hit the native rate exactly (I16); a nicer-format range
        // cannot. Rate proximity is primary — a resample-free capture beats a
        // higher-fidelity conversion.
        let ranges = [
            r(2, 44_100, 48_000, SampleFormat::I16),
            r(2, 88_200, 96_000, SampleFormat::F32),
        ];
        assert_eq!(
            pick_input_config(&ranges, None, 48_000),
            Some((48_000, 2, SampleFormat::I16))
        );
    }

    #[test]
    fn empty_ranges_yield_none_for_default_fallback() {
        assert_eq!(pick_input_config(&[], Some(48_000), 48_000), None);
    }

    #[test]
    fn forced_rate_only_counts_on_a_max_channel_range() {
        // 96 kHz exists only on the stereo shadow range; the 8-channel range
        // maxes at 48 kHz. Channels win: record 8ch at its native-clamped rate.
        let ranges = [
            r(2, 44_100, 96_000, SampleFormat::F32),
            r(8, 44_100, 48_000, SampleFormat::F32),
        ];
        assert_eq!(
            pick_input_config(&ranges, Some(96_000), 48_000),
            Some((48_000, 8, SampleFormat::F32))
        );
    }

    fn cpal_devices(names: &[&str]) -> Vec<FfmpegDevice> {
        names
            .iter()
            .map(|n| FfmpegDevice::new(*n, "cpal", None))
            .collect()
    }

    #[test]
    fn device_target_prefers_an_exact_name() {
        let devs = cpal_devices(&["MacBook Pro Microphone", "Qu-5", "Qu-5 (2)"]);
        assert_eq!(resolve_device_target(&devs, "Qu-5"), "Qu-5");
    }

    #[test]
    fn device_target_bridges_a_web_audio_label_to_the_cpal_name() {
        // The stored name is the frontend's Web Audio label, which is NOT the
        // cpal name. Before this bridge existed the mismatch made cpal silently
        // never engage.
        let devs = cpal_devices(&["MacBook Pro Microphone", "Allen & Heath Qu-5"]);
        assert_eq!(
            resolve_device_target(&devs, "Allen & Heath Qu-5 (USB Audio)"),
            "Allen & Heath Qu-5"
        );
    }

    #[test]
    fn device_target_falls_back_to_the_requested_name_verbatim() {
        // Nothing plausible in the list → hand back what the user asked for so
        // the error message names THEIR device, not a random survivor.
        assert_eq!(
            resolve_device_target(&cpal_devices(&[]), "Qu-5"),
            "Qu-5",
            "an empty device list must not invent a target"
        );
        assert_eq!(
            resolve_device_target(&cpal_devices(&["Zoom Audio Device"]), "Qu-5"),
            "Qu-5"
        );
    }

    #[test]
    fn ring_holds_five_seconds_of_routed_audio() {
        // 48 kHz stereo: 48 000 × 2 × 5 samples.
        assert_eq!(ring_capacity(48_000, 2), 480_000);
        // 96 kHz stereo — the worst case this app can negotiate.
        assert_eq!(ring_capacity(96_000, 2), 960_000);
    }

    #[test]
    fn ring_is_five_seconds_not_one() {
        // The regression guard for the 2026-08-10 raise: a 48 kHz stereo ring
        // must be FIVE times the old one-second capacity, and comfortably
        // above the old flat 96 000-sample floor.
        assert_eq!(ring_capacity(48_000, 2), 5 * (48_000 * 2));
        assert!(
            ring_capacity(48_000, 2) > 96_000,
            "the pre-2026-08-10 one-second ring must no longer be produced"
        );
    }

    #[test]
    fn ring_stays_under_four_megabytes_at_the_worst_format() {
        // The cost argument in RING_SECONDS' docs, pinned: 96 kHz stereo f32.
        let bytes = ring_capacity(96_000, 2) * std::mem::size_of::<f32>();
        assert!(
            bytes < 4 * 1024 * 1024,
            "worst-case ring is {bytes} B — the raise was justified as <4 MB"
        );
    }

    #[test]
    fn ring_floor_applies_to_small_formats() {
        // 8 kHz mono would otherwise get 40 000 samples; the floor lifts it.
        assert_eq!(ring_capacity(8_000, 1), MIN_RING_SAMPLES);
        // A zero channel count is treated as mono rather than producing a
        // zero-capacity ring the producer could never push into.
        assert_eq!(ring_capacity(8_000, 0), MIN_RING_SAMPLES);
    }

    /// A3c: only `Xrun` is counted — not the other advisories, not a fatal
    /// fault — and the count is exact even though the log line is thinned.
    #[test]
    fn only_xruns_are_counted() {
        use cpal::ErrorKind as K;
        let count = AtomicU64::new(0);
        let report = |kind: K| {
            classify_stream_error_counting("t", &cpal::Error::with_message(kind, ""), Some(&count))
        };
        for _ in 0..10 {
            assert!(report(K::Xrun).is_none(), "an xrun never ends the stream");
        }
        assert_eq!(count.load(Ordering::Relaxed), 10);
        assert!(report(K::RealtimeDenied).is_none());
        assert!(report(K::DeviceChanged).is_none());
        assert!(report(K::DeviceNotAvailable).is_some());
        assert_eq!(
            count.load(Ordering::Relaxed),
            10,
            "other kinds leave the count alone"
        );
        // Without a counter (meters, pre-roll) the same report is just logged.
        assert!(classify_stream_error("t", &cpal::Error::with_message(K::Xrun, "")).is_none());
        assert_eq!(count.load(Ordering::Relaxed), 10);
    }

    /// A3b: the four classes the reconnect machine acts on are mapped from
    /// cpal's TYPE, not its message — an empty message proves no text is read.
    #[test]
    fn cpal_kinds_map_onto_the_core_fault_classes() {
        use cpal::ErrorKind as K;
        for (kind, want) in [
            (K::DeviceNotAvailable, StreamFault::DeviceNotAvailable),
            (K::StreamInvalidated, StreamFault::StreamInvalidated),
            (K::DeviceBusy, StreamFault::DeviceBusy),
            (K::PermissionDenied, StreamFault::PermissionDenied),
            (K::DeviceChanged, StreamFault::DeviceChanged),
            (K::Xrun, StreamFault::Xrun),
            (K::RealtimeDenied, StreamFault::RealtimeDenied),
            (K::BackendError, StreamFault::Other),
            (K::HostUnavailable, StreamFault::Other),
            (K::ResourceExhausted, StreamFault::Other),
            (K::Other, StreamFault::Other),
        ] {
            assert_eq!(stream_fault(kind), want, "{kind:?}");
            let report = classify_stream_error("t", &cpal::Error::with_message(kind, ""));
            match stream_fault_action(want) {
                StreamFaultAction::KeepRecording => assert_eq!(report, None, "{kind:?}"),
                _ => assert_eq!(report.map(|r| r.fault), Some(want), "{kind:?}"),
            }
        }
    }

    /// A3b machine proof (macOS, real CoreAudio): a sample-rate change of the
    /// device under a running capture arrives through OUR error callback as
    /// `StreamInvalidated` and is classified as a fault that ends the stream —
    /// the "frekvensbytte" the reconnect machine now owns.
    ///
    /// The rate change is forced the way another app would force it: a second
    /// cpal stream on the same device at another rate (cpal re-clocks the
    /// device to get it). The device's original rate is restored at the end.
    /// Touches the default input device, so it is `#[ignore]`d:
    /// `cargo test -p sundayrec rate_change_under_a_running_stream -- --ignored`
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "re-clocks the real default input device"]
    fn rate_change_under_a_running_stream_is_a_stream_fault() {
        use cpal::traits::StreamTrait;
        use std::time::{Duration, Instant};

        let host = open_host(CpalHostKind::Default).expect("host");
        let device = host.default_input_device().expect("a default input device");
        let original = device.default_input_config().expect("default config");
        let r0 = original.sample_rate();
        let r1 = if r0 == 44_100 { 48_000 } else { 44_100 };
        let fmt = original.sample_format();
        let ch = original.channels();
        let supports = |rate: u32| {
            device
                .supported_input_configs()
                .map(|mut it| {
                    it.any(|c| (c.min_sample_rate()..=c.max_sample_rate()).contains(&rate))
                })
                .unwrap_or(false)
        };
        assert!(
            supports(r1),
            "default input cannot run at {r1} Hz — pick another device"
        );

        let open = |rate: u32, tx: Option<std::sync::mpsc::Sender<StreamFaultReport>>| {
            let cfg = cpal::StreamConfig {
                channels: ch,
                sample_rate: rate,
                buffer_size: cpal::BufferSize::Default,
            };
            let stream = build_input_stream_any(
                &device,
                cfg,
                fmt,
                ch as usize,
                StreamSink::Meters(Arc::new(MeterBanks::new(ch as usize))),
                move |e: cpal::Error| {
                    if let (Some(r), Some(tx)) = (classify_stream_error("proof", &e), tx.as_ref()) {
                        let _ = tx.send(r);
                    }
                },
            )
            .expect("build");
            stream.play().expect("play");
            stream
        };

        let (tx, rx) = std::sync::mpsc::channel();
        let victim = open(r0, Some(tx));
        std::thread::sleep(Duration::from_millis(300));
        let intruder = open(r1, None); // re-clocks the device to r1
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = None;
        while Instant::now() < deadline {
            if let Ok(r) = rx.recv_timeout(Duration::from_millis(100)) {
                got = Some(r);
                break;
            }
        }
        drop(intruder);
        drop(victim);
        // Put the device back where we found it.
        drop(open(r0, None));
        assert_eq!(
            device.default_input_config().map(|c| c.sample_rate()).ok(),
            Some(r0),
            "the device's rate must be restored"
        );

        let got = got.expect("no fault reported within 3 s of the rate change");
        assert_eq!(got.fault, StreamFault::StreamInvalidated, "{got:?}");
        assert!(!sundayrec_core::stream_fault::reconnect_keeps_rate_pin(
            Some(got.fault)
        ));
    }

    #[test]
    fn host_kind_labels() {
        assert_eq!(CpalHostKind::Wasapi.label(), "wasapi");
        assert_eq!(CpalHostKind::Asio.label(), "asio");
        let expect = if cfg!(target_os = "macos") {
            "coreaudio"
        } else {
            "wasapi"
        };
        assert_eq!(CpalHostKind::Default.label(), expect);
    }

    #[test]
    fn default_host_opens_everywhere() {
        // The default host must open on every platform (CoreAudio/WASAPI).
        // `open_host` also starts `audio::com_anchor` first, which is what keeps
        // the enumeration calls (`.default_input_device()`, `.input_devices()`)
        // from crashing the process once the thread that first made them exits
        // (the F2-W7 crash of #231, root-caused 2026-09-19).
        assert!(open_host(CpalHostKind::Default).is_ok());
    }
}
