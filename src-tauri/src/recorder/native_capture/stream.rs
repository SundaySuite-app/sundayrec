//! The consolidated cpal stream layer: host open, fuzzy device find, format
//! negotiation, and ONE typed stream-builder set shared by the native capture
//! engine and the VU engine (`audio/vu.rs`).
//!
//! Everything here is portable cpal code — the platform differences are
//! confined to [`open_host`]'s Windows-only host ids. The decision half of
//! format negotiation ([`pick_input_config`]) is pure and unit-tested off
//! hardware; [`negotiate`] is the thin wrapper that feeds it real device data.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait};
// `Sample` provides the ergonomic `f32::from_sample(s)`; `FromSample` is the bound.
use cpal::{FromSample, Sample, SampleFormat};
use ringbuf::traits::Producer;
use ringbuf::HeapProd;
use sundayrec_core::audio::{observe_levels, MeterBanks};
use sundayrec_core::device_match::{find_best_device_match, FfmpegDevice};

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
#[allow(deprecated)] // cpal 0.17 deprecates `name()`; still the human name we match on.
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
        .filter_map(|d| d.name().ok())
        .map(|n| FfmpegDevice::new(n, "cpal", None))
        .collect();
    let target = find_best_device_match(&candidates, name)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| name.to_string());
    devices
        .into_iter()
        .find(|d| d.name().ok().as_deref() == Some(target.as_str()))
        .ok_or_else(|| format!("input device not found: {name}"))
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
                // cpal 0.17: SampleRate is a plain u32.
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

/// Build an input stream for sample type `T`: convert each sample to f32 via
/// cpal `from_sample` (covers i16/i32/f32/**I24**/u16/…) and feed the sink.
/// `conv`/`scratch` are allocated once here, never in the real-time callback.
fn build_typed<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    total: usize,
    mut sink: StreamSink,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: FromSample<T>,
{
    let mut conv: Vec<f32> = Vec::with_capacity(4096);
    let mut scratch: Vec<f32> = Vec::with_capacity(4096);
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
    config: &cpal::StreamConfig,
    sample_format: SampleFormat,
    total: usize,
    sink: StreamSink,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
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
        assert!(open_host(CpalHostKind::Default).is_ok());
    }
}
