//! Windows ASIO device + channel enumeration (Fase 2).
//!
//! The DirectShow/WASAPI path the recorder uses today splits a pro multichannel
//! interface (e.g. a Soundcraft MADI-USB) into several stereo "devices", so you
//! can never address channel 9/10 of a mixer. ASIO exposes the whole interface
//! as ONE device with all its channels — which is exactly what church A/V rigs
//! need. This module enumerates the ASIO host so the picker can list those
//! devices + channels and tag them with a [`AudioBackendKind::Asio`] badge.
//!
//! ## Windows-only, behind a feature
//!
//! Every real ASIO call is `#[cfg(all(target_os = "windows", feature = "asio"))]`.
//! On macOS/Linux, or when the feature is off, the functions return empty/`false`
//! so the rest of the app reads identically on every platform and the recorder
//! falls back to the existing dshow/WASAPI capture automatically. See
//! `docs/BUILD_ASIO.md` for the Windows build env.
//!
//! ## Enumeration is not free — it LOADS drivers
//!
//! cpal's ASIO `host.devices()` calls `ASIOInit` on every installed driver. ASIO
//! drivers are single-client, so that sweep can pop a driver control panel or
//! take the sound card a WASAPI capture is about to open. So the sweep happens
//! only when it can change an answer ([`resolve_is_asio_device`]) and its result
//! is memoised for [`ASIO_CACHE_TTL`] ([`AsioCache`]).
//!
//! [`list_asio_devices`] is the ONE function that sweeps. Everything else reads
//! its result: the picker's `list_audio_devices`, the recorder's
//! [`is_asio_device`], [`list_asio_input_channels`]. Adding a second sweep
//! anywhere re-opens the finding.
//!
//! ## ⚠️ HARDWARE-UNVERIFIED
//!
//! The cpal ASIO calls can only be exercised on a Windows box with an ASIO driver
//! installed (ASIO4ALL is enough for a smoke test). Off-Windows builds compile the
//! stubs; the types + their serde/ts-rs derives are what the unit tests cover.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Standard sample rates we surface in the UI. A device advertises a *range*; we
/// report which of these well-known rates fall inside it. Mirrors the constant in
/// [`crate::audio::devices`].
#[cfg(all(target_os = "windows", feature = "asio"))]
const STANDARD_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// Which OS audio backend a device is reached through. The frontend renders this
/// as a small badge next to the device name ("ASIO" / "WASAPI" / "CoreAudio") so
/// the user can see they're getting the low-latency multichannel path — the rest
/// of the picker UI is identical across backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "AudioBackendKind.ts")]
#[serde(rename_all = "lowercase")]
pub enum AudioBackendKind {
    /// Windows ASIO (low-latency, single-device multichannel).
    Asio,
    /// Windows WASAPI / DirectShow (the default Windows fallback).
    Wasapi,
    /// macOS Core Audio.
    CoreAudio,
}

/// One ASIO device with the capabilities the picker needs. `id` and `name` are
/// the same string today (ASIO addresses devices by name); `id` is kept separate
/// so a later backend can use a stabler handle without changing the contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "AsioDevice.ts")]
#[serde(rename_all = "camelCase")]
pub struct AsioDevice {
    /// Stable-ish identifier the recorder addresses (the ASIO device name).
    pub id: String,
    /// Human-readable device name as reported by the driver.
    pub name: String,
    /// Always [`AudioBackendKind::Asio`] — present so the device shares one shape
    /// with any future unified device list.
    pub backend: AudioBackendKind,
    /// Number of input channels the interface exposes under this one device.
    pub input_channels: u16,
    /// Number of output channels (for later playback work; 0 if none).
    pub output_channels: u16,
    /// The device's default/native sample rate (Hz), or 0 if unknown.
    pub default_sample_rate: u32,
    /// Standard sample rates (Hz) the device supports.
    pub supported_sample_rates: Vec<u32>,
}

/// One addressable input channel. `label` is human-readable. cpal reports channel
/// *count*, not per-channel driver names, so v1 labels are `"Input N"`; true
/// driver-supplied names would need the ASIO SDK's `ASIOGetChannelInfo` (TODO).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "AudioChannel.ts")]
#[serde(rename_all = "camelCase")]
pub struct AudioChannel {
    /// 0-based channel index as the recorder addresses it.
    pub index: u16,
    /// Human-readable label, e.g. `"Input 1"`.
    pub label: String,
}

/// Build `["Input 1", "Input 2", …]` channel labels for `n` input channels.
/// Pure + unit-testable; shared by the real and stub paths.
pub fn input_channels_for(count: u16) -> Vec<AudioChannel> {
    (0..count)
        .map(|index| AudioChannel {
            index,
            label: format!("Input {}", index + 1),
        })
        .collect()
}

/// How one OUTPUT channel is produced from the ASIO device's interleaved input
/// frame. The cpal callback applies a `Vec<ChannelRoute>` per frame so the PCM it
/// pushes into the pipe is ALREADY the recorded layout — ffmpeg then needs no
/// `pan` filter (the dshow path's [`sundayrec_core::capture::channel_map_filter`]
/// equivalent, done in the callback for lower latency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRoute {
    /// Copy source channel `n` straight through.
    Pick(u16),
    /// Average source channels `a` and `b` (the MonoMix downmix).
    MixHalf(u16, u16),
}

/// Build the per-frame output routing for an ASIO capture from the recording's
/// channel mode + the user's explicit L/R picks, clamped to the device's actual
/// input-channel count. Pure + unit-tested; mirrors the dshow `pan` semantics in
/// [`sundayrec_core::capture::custom_channel_map_filter`]/`channel_map_filter`:
///   - Stereo → two channels (custom L/R picks, else 0 & 1),
///   - MonoL/MonoR → one channel (the picked L resp. R, else 0/1),
///   - MonoMix → one channel averaging 0 & 1.
///
/// Indices that would exceed `total_input_channels` are clamped to the last valid
/// channel so a stale settings pick can never read out of bounds.
pub fn build_route_plan(
    mode: sundayrec_core::settings::ChannelMode,
    input_channel_l: Option<i32>,
    input_channel_r: Option<i32>,
    total_input_channels: u16,
) -> Vec<ChannelRoute> {
    use sundayrec_core::settings::ChannelMode;
    let max = total_input_channels.saturating_sub(1);
    let clamp = |i: i32| -> u16 { (i.max(0) as u16).min(max) };
    let l = clamp(input_channel_l.unwrap_or(0));
    let r = clamp(input_channel_r.unwrap_or(1));
    match mode {
        ChannelMode::Stereo => vec![ChannelRoute::Pick(l), ChannelRoute::Pick(r)],
        ChannelMode::MonoL => vec![ChannelRoute::Pick(l)],
        ChannelMode::MonoR => vec![ChannelRoute::Pick(r)],
        // Mix channels 0 & 1; on a 1-channel device both clamp to 0 (mixes ch0
        // with itself = ch0).
        ChannelMode::MonoMix => vec![ChannelRoute::MixHalf(0, 1u16.min(max))],
    }
}

/// Apply a route plan to one interleaved input frame (`total` source samples),
/// appending the routed output samples to `out`. The real-time cpal callback
/// calls this per frame; it does only arithmetic + pushes (no allocation when
/// `out` is pre-reserved), so it is RT-safe. Pure → unit-tested off-Windows.
pub fn route_frame(plan: &[ChannelRoute], frame: &[f32], out: &mut Vec<f32>) {
    for route in plan {
        let s = match *route {
            ChannelRoute::Pick(n) => frame.get(n as usize).copied().unwrap_or(0.0),
            ChannelRoute::MixHalf(a, b) => {
                let av = frame.get(a as usize).copied().unwrap_or(0.0);
                let bv = frame.get(b as usize).copied().unwrap_or(0.0);
                0.5 * (av + bv)
            }
        };
        out.push(s);
    }
}

/// One entry in the unified, backend-tagged input-device list the picker renders.
/// ASIO devices and the host's cpal (WASAPI/CoreAudio) devices share this one
/// shape so the frontend renders them identically, differing only in the badge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "TaggedAudioInput.ts")]
#[serde(rename_all = "camelCase")]
pub struct TaggedAudioInput {
    /// Identifier the recorder addresses (device name today).
    pub id: String,
    /// Human-readable device name.
    pub name: String,
    /// Which backend this device is reached through (drives the UI badge).
    pub backend: AudioBackendKind,
    /// Number of input channels (drives the L/R channel selector).
    pub input_channels: u16,
    /// Standard sample rates (Hz) the device supports.
    pub sample_rates: Vec<u32>,
    /// Whether this is the host's default device.
    pub is_default: bool,
}

/// The backend kind for the host's non-ASIO cpal devices on this platform:
/// WASAPI on Windows, Core Audio everywhere else.
pub const fn host_backend_kind() -> AudioBackendKind {
    #[cfg(target_os = "windows")]
    {
        AudioBackendKind::Wasapi
    }
    #[cfg(not(target_os = "windows"))]
    {
        AudioBackendKind::CoreAudio
    }
}

/// Merge the ASIO devices with the host's cpal input devices into one tagged
/// list. ASIO devices come FIRST and take precedence: a cpal device whose name
/// matches an ASIO device is dropped, so a pro interface isn't listed twice (once
/// as ASIO, once as its WASAPI stereo-pair shadow). Pure → unit-testable.
pub fn merge_audio_inputs(
    asio: Vec<AsioDevice>,
    cpal: &[crate::audio::devices::AudioDevice],
) -> Vec<TaggedAudioInput> {
    let asio_names: std::collections::HashSet<&str> =
        asio.iter().map(|d| d.name.as_str()).collect();

    let mut out: Vec<TaggedAudioInput> = asio
        .iter()
        .map(|d| TaggedAudioInput {
            id: d.id.clone(),
            name: d.name.clone(),
            backend: AudioBackendKind::Asio,
            input_channels: d.input_channels,
            sample_rates: d.supported_sample_rates.clone(),
            is_default: false,
        })
        .collect();

    let host_backend = host_backend_kind();
    for d in cpal {
        // A WASAPI device names a pro card's stereo pair under the same interface
        // name — skip it when ASIO already exposes that interface in full.
        if asio_names.contains(d.name.as_str()) {
            continue;
        }
        out.push(TaggedAudioInput {
            id: d.name.clone(),
            name: d.name.clone(),
            backend: host_backend,
            input_channels: d.channels,
            sample_rates: d.sample_rates.clone(),
            is_default: d.is_default,
        });
    }
    out
}

// ── Real ASIO enumeration (Windows + feature only) ───────────────────────────
// cpal 0.17: `SampleRate` is a plain `u32` (no `.0`), and `name()` is deprecated
// but is still the human device name we match settings against — hence the
// module-wide `allow(deprecated)`.
#[cfg(all(target_os = "windows", feature = "asio"))]
#[allow(deprecated)]
mod imp {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait};

    /// Open the ASIO host. Returns `None` if cpal can't reach an ASIO driver
    /// (none installed / driver error) — the caller then falls back to WASAPI.
    fn asio_host() -> Option<cpal::Host> {
        cpal::host_from_id(cpal::HostId::Asio).ok()
    }

    /// Summarise one ASIO device into an [`AsioDevice`]. Never panics: a device
    /// that refuses to report configs is returned with what we could read.
    fn summarise(device: &cpal::Device) -> AsioDevice {
        let name = device
            .name()
            .unwrap_or_else(|_| "Unknown ASIO device".to_string());

        let mut input_channels: u16 = 0;
        let mut output_channels: u16 = 0;
        let mut rate_min = u32::MAX;
        let mut rate_max = 0u32;

        if let Ok(configs) = device.supported_input_configs() {
            for cfg in configs {
                input_channels = input_channels.max(cfg.channels());
                rate_min = rate_min.min(cfg.min_sample_rate());
                rate_max = rate_max.max(cfg.max_sample_rate());
            }
        }
        if let Ok(configs) = device.supported_output_configs() {
            for cfg in configs {
                output_channels = output_channels.max(cfg.channels());
            }
        }

        let supported_sample_rates: Vec<u32> = if rate_min == u32::MAX {
            Vec::new()
        } else {
            STANDARD_RATES
                .into_iter()
                .filter(|r| *r >= rate_min && *r <= rate_max)
                .collect()
        };

        let default_sample_rate = device
            .default_input_config()
            .map(|c| c.sample_rate())
            .unwrap_or(0);

        AsioDevice {
            id: name.clone(),
            name,
            backend: AudioBackendKind::Asio,
            input_channels,
            output_channels,
            default_sample_rate,
            supported_sample_rates,
        }
    }

    pub fn list_asio_devices() -> Vec<AsioDevice> {
        let Some(host) = asio_host() else {
            return Vec::new();
        };
        let Ok(devices) = host.devices() else {
            return Vec::new();
        };
        devices.map(|d| summarise(&d)).collect()
    }
}

// ── Stub path (everything else) ──────────────────────────────────────────────
#[cfg(not(all(target_os = "windows", feature = "asio")))]
mod imp {
    use super::*;

    pub fn list_asio_devices() -> Vec<AsioDevice> {
        Vec::new()
    }
}

// ── Asking ASIO only when it is actually needed (F2-W8) ──────────────────────
//
// Enumerating the ASIO host is not a read: cpal's `host.devices()` LOADS every
// installed ASIO driver and calls `ASIOInit` on it. ASIO drivers are
// single-client, so an ASIO4ALL or a Realtek-ASIO waking up can pop a driver
// control panel, or take the sound card that the WASAPI capture is about to
// open two lines later. Recording start called `is_asio_device()` — hence a
// full sweep — on EVERY start, even when the operator had picked a WASAPI
// device and no ASIO path could ever be taken. This section makes the sweep
// conditional and memoises it.

/// Whether this build can reach ASIO at all: Windows AND the `asio` feature.
///
/// A `const` (not a `#[cfg]` block) so the decision below is one plain boolean
/// that unit tests can drive both ways off-Windows; the optimiser folds it, so
/// the macOS build still short-circuits without doing any work.
pub const ASIO_AVAILABLE: bool = cfg!(all(target_os = "windows", feature = "asio"));

/// How long one ASIO enumeration is reused before the drivers are asked again.
///
/// 30 s: long enough that a session in the device picker — open it, scroll,
/// close, re-open, then press record — costs ONE driver sweep instead of one
/// per click, and short enough that an interface plugged in mid-service shows
/// up without restarting the app. Windows ships no device-change listener
/// (see [`crate::audio::device_watch`]), so this TTL, plus the explicit
/// [`invalidate_asio_cache`] the diagnose tool calls, is the whole freshness
/// story on the platform ASIO exists on.
pub const ASIO_CACHE_TTL: Duration = Duration::from_secs(30);

/// A process-wide, TTL'd memo of the last ASIO enumeration.
///
/// Two locks, on purpose:
///   - `entry` guards the memo itself and is NEVER held across a probe. A sweep
///     can block for seconds inside a driver, and a stalled sweep must not also
///     freeze the next [`peek_at`](Self::peek_at).
///   - `probing` serialises the sweeps. ASIO drivers are single-client, so two
///     threads calling `ASIOInit` on the same driver at once is the very failure
///     this module exists to avoid — the picker's blocking enumeration and a
///     recording start CAN land together. The second caller waits for the first
///     sweep and then reads its result instead of starting its own.
pub(crate) struct AsioCache {
    ttl: Duration,
    entry: Mutex<Option<(Instant, Vec<AsioDevice>)>>,
    probing: Mutex<()>,
}

impl AsioCache {
    pub(crate) const fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entry: Mutex::new(None),
            probing: Mutex::new(()),
        }
    }

    /// The memoised devices if the entry is still younger than the TTL at `now`,
    /// else `None`. Never probes.
    pub(crate) fn peek_at(&self, now: Instant) -> Option<Vec<AsioDevice>> {
        let guard = crate::util::lock_recover(&self.entry);
        match guard.as_ref() {
            Some((stamp, devices)) if now.saturating_duration_since(*stamp) < self.ttl => {
                Some(devices.clone())
            }
            _ => None,
        }
    }

    /// The memoised devices, or `probe()` — which is then memoised at `now`.
    pub(crate) fn get_or_fill_at(
        &self,
        now: Instant,
        probe: impl FnOnce() -> Vec<AsioDevice>,
    ) -> Vec<AsioDevice> {
        if let Some(hit) = self.peek_at(now) {
            return hit;
        }
        let _sweeping = crate::util::lock_recover(&self.probing);
        // Re-check: whoever held `probing` may have just filled the memo, and
        // their sweep is as good as the one we were about to start.
        if let Some(hit) = self.peek_at(now) {
            return hit;
        }
        let fresh = probe();
        *crate::util::lock_recover(&self.entry) = Some((now, fresh.clone()));
        fresh
    }

    /// Drop the memo, so the next call probes.
    pub(crate) fn invalidate(&self) {
        *crate::util::lock_recover(&self.entry) = None;
    }
}

static ASIO_CACHE: AsioCache = AsioCache::new(ASIO_CACHE_TTL);

/// Forget the memoised ASIO enumeration, so the next call asks the drivers again.
///
/// Called by the diagnose tool, whose whole job is to report what is on the
/// machine RIGHT NOW; a Windows device-change listener would call it too, if one
/// shipped (see [`crate::audio::device_watch`] for why none does).
pub fn invalidate_asio_cache() {
    ASIO_CACHE.invalidate();
}

/// Enumerate the ASIO input devices visible on this machine. Empty when ASIO is
/// unavailable (non-Windows, feature off, or no driver installed).
///
/// Memoised for [`ASIO_CACHE_TTL`] — see [`AsioCache`]. Use
/// [`invalidate_asio_cache`] first when a stale answer would be wrong.
pub fn list_asio_devices() -> Vec<AsioDevice> {
    if !ASIO_AVAILABLE {
        return Vec::new();
    }
    ASIO_CACHE.get_or_fill_at(Instant::now(), imp::list_asio_devices)
}

/// List the input channels of one ASIO device. Empty if the device is gone or
/// ASIO is unavailable.
///
/// Reads the channel count out of [`list_asio_devices`] — so it shares the ONE
/// memo, and cannot become a second, unconditional way to load every installed
/// driver. It used to walk the ASIO host itself with its own `host.devices()`
/// sweep; nothing in the shipped path calls it today (the channel count comes
/// from `start_vu`'s negotiated reply — see [`crate::commands::audio`]), so that
/// sweep was a landmine for whichever caller brought it back, not a live cost.
///
/// Same answer as the old walk: `summarise` derives `input_channels` from the
/// same `supported_input_configs().map(channels).max()` this used to compute
/// inline. Device names are matched case-insensitively, like everywhere else in
/// this module.
pub fn list_asio_input_channels(device_id: &str) -> Vec<AudioChannel> {
    input_channels_of(&list_asio_devices(), device_id)
}

/// The lookup half of [`list_asio_input_channels`], with the device list passed
/// in so it can be exercised without an ASIO driver. Empty for a name no device
/// answers to.
fn input_channels_of(devices: &[AsioDevice], device_id: &str) -> Vec<AudioChannel> {
    let needle = device_id.to_lowercase();
    devices
        .iter()
        .find(|d| d.id.to_lowercase() == needle || d.name.to_lowercase() == needle)
        .map(|d| input_channels_for(d.input_channels))
        .unwrap_or_default()
}

/// Case-insensitive device-name membership. Windows spells the same interface
/// with different casing across its WASAPI and ASIO views.
fn contains_name(names: &[String], name: &str) -> bool {
    let needle = name.to_lowercase();
    names.iter().any(|n| n.to_lowercase() == needle)
}

/// Every string an enumerated ASIO device answers to.
///
/// Both `name` AND `id`, because that is what the membership test replaced here
/// did (`d.name == name || d.id == name`). The two are the same string today —
/// see [`AsioDevice`] — so this is a no-op that stays correct if a later backend
/// gives `id` a stabler handle.
fn device_names(devices: &[AsioDevice]) -> Vec<String> {
    devices
        .iter()
        .flat_map(|d| [d.name.clone(), d.id.clone()])
        .collect()
}

/// Whether resolving `name` requires ASKING the ASIO host — the call that loads
/// (`ASIOInit`s) every installed ASIO driver.
///
/// `false` when the host's own input devices already answer to `name`: a device
/// WASAPI/Core Audio names is reached through that backend, so there is nothing
/// for ASIO to add. `false` for an empty name — nothing is configured, and no
/// driver has ever reported `""` as its name, so a sweep could only answer "no".
/// `true` otherwise, INCLUDING when `known_non_asio_names` is empty: knowing
/// nothing about the host's devices is not evidence that the name isn't ASIO.
pub fn needs_asio_probe(name: &str, known_non_asio_names: &[String]) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    !contains_name(known_non_asio_names, name)
}

/// The whole decision behind [`is_asio_device`], with its three lookups passed
/// in so tests can count them.
///
/// In order of cost:
///   1. ASIO unreachable in this build, or no device configured → `false`, free.
///   2. A fresh enumeration is already memoised → answer from it, free.
///   3. The host (WASAPI/Core Audio) already knows the name → `false`, one cheap
///      name-only enumeration ([`crate::audio::devices::list_input_device_names`]).
///   4. Only now: sweep the ASIO drivers.
///
/// ⚠️ Step 3's trade-off, stated plainly: if a WASAPI endpoint reports EXACTLY
/// the same name as an installed ASIO driver, this routes to WASAPI without
/// asking ASIO. In practice the two views name a card differently ("ASIO4ALL
/// v2" / "Focusrite USB ASIO" vs "Mikrofon (Focusrite USB Audio)") — the
/// same-name dedup in [`merge_audio_inputs`] is defensive, not the common case —
/// and step 2 covers the ordinary flow, where the picker has just enumerated
/// ASIO and the user then presses record. The cost of being wrong is a WASAPI
/// recording instead of a multichannel ASIO one; the cost of the old
/// unconditional sweep was a driver panel, or a stolen sound card, at the start
/// of every service. Rig point (w14) checks it on real hardware.
pub(crate) fn resolve_is_asio_device(
    asio_available: bool,
    name: &str,
    fresh_asio_names: Option<Vec<String>>,
    host_names: impl FnOnce() -> Vec<String>,
    probe_asio_names: impl FnOnce() -> Vec<String>,
) -> bool {
    if !asio_available || name.trim().is_empty() {
        return false;
    }
    if let Some(known) = fresh_asio_names {
        return contains_name(&known, name);
    }
    if !needs_asio_probe(name, &host_names()) {
        return false;
    }
    contains_name(&probe_asio_names(), name)
}

/// Whether `name` matches a currently-present ASIO device. Used by the recorder to
/// decide whether to take the ASIO capture path or fall back to dshow/WASAPI.
/// Always `false` when ASIO is unavailable.
///
/// Every caller — recording start, the capture bench, the pre-service capture
/// probe — goes through here, so they all share one answer and one memo. See
/// [`resolve_is_asio_device`] for what it costs.
pub fn is_asio_device(name: &str) -> bool {
    resolve_is_asio_device(
        ASIO_AVAILABLE,
        name,
        ASIO_CACHE
            .peek_at(Instant::now())
            .map(|devices| device_names(&devices)),
        crate::audio::devices::list_input_device_names,
        || device_names(&list_asio_devices()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_channels_are_one_based_labels_zero_based_indices() {
        let chans = input_channels_for(3);
        assert_eq!(chans.len(), 3);
        assert_eq!(
            chans[0],
            AudioChannel {
                index: 0,
                label: "Input 1".into()
            }
        );
        assert_eq!(
            chans[2],
            AudioChannel {
                index: 2,
                label: "Input 3".into()
            }
        );
    }

    #[test]
    fn input_channels_for_zero_is_empty() {
        assert!(input_channels_for(0).is_empty());
    }

    #[test]
    fn enumeration_is_empty_without_asio() {
        // On the CI/dev machines (non-Windows or feature off) the stub returns
        // empty and never panics. On a Windows+asio rig this would be non-empty;
        // the contract here is only "does not panic / sane shape".
        let _ = list_asio_devices();
        let _ = list_asio_input_channels("whatever");
        assert!(!is_asio_device("definitely not a device"));
    }

    #[test]
    fn asio_device_serde_roundtrip() {
        let d = AsioDevice {
            id: "Soundcraft MADI USB".into(),
            name: "Soundcraft MADI USB".into(),
            backend: AudioBackendKind::Asio,
            input_channels: 32,
            output_channels: 32,
            default_sample_rate: 48_000,
            supported_sample_rates: vec![44_100, 48_000, 96_000],
        };
        let json = serde_json::to_string(&d).expect("serialise");
        let back: AsioDevice = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(d, back);
    }

    #[test]
    fn merge_puts_asio_first_and_dedups_wasapi_shadow() {
        use crate::audio::devices::AudioDevice;
        let asio = vec![AsioDevice {
            id: "Soundcraft MADI USB".into(),
            name: "Soundcraft MADI USB".into(),
            backend: AudioBackendKind::Asio,
            input_channels: 32,
            output_channels: 32,
            default_sample_rate: 48_000,
            supported_sample_rates: vec![48_000],
        }];
        let cpal = vec![
            // The same interface, seen by WASAPI as a stereo pair — must be dropped.
            AudioDevice {
                name: "Soundcraft MADI USB".into(),
                direction: "input".into(),
                channels: 2,
                sample_rates: vec![48_000],
                is_default: false,
            },
            // A genuine other device — must be kept.
            AudioDevice {
                name: "USB Audio CODEC".into(),
                direction: "input".into(),
                channels: 2,
                sample_rates: vec![44_100, 48_000],
                is_default: true,
            },
        ];
        let merged = merge_audio_inputs(asio, &cpal);
        assert_eq!(merged.len(), 2, "ASIO + the one genuine WASAPI device");
        assert_eq!(merged[0].backend, AudioBackendKind::Asio);
        assert_eq!(merged[0].name, "Soundcraft MADI USB");
        assert_eq!(merged[0].input_channels, 32);
        assert_eq!(merged[1].name, "USB Audio CODEC");
        assert_eq!(merged[1].backend, host_backend_kind());
        assert!(merged[1].is_default);
    }

    #[test]
    fn merge_with_no_asio_is_just_the_host_list() {
        use crate::audio::devices::AudioDevice;
        let cpal = vec![AudioDevice {
            name: "MacBook Pro-mikrofon".into(),
            direction: "input".into(),
            channels: 1,
            sample_rates: vec![48_000],
            is_default: true,
        }];
        let merged = merge_audio_inputs(Vec::new(), &cpal);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].backend, host_backend_kind());
    }

    #[test]
    fn route_plan_stereo_default_and_custom() {
        use sundayrec_core::settings::ChannelMode;
        // Default stereo on a 32-ch device → channels 0 and 1.
        let p = build_route_plan(ChannelMode::Stereo, None, None, 32);
        assert_eq!(p, vec![ChannelRoute::Pick(0), ChannelRoute::Pick(1)]);
        // Custom picks (mixer channels 8 & 9) flow through.
        let p = build_route_plan(ChannelMode::Stereo, Some(8), Some(9), 32);
        assert_eq!(p, vec![ChannelRoute::Pick(8), ChannelRoute::Pick(9)]);
    }

    #[test]
    fn route_plan_mono_modes() {
        use sundayrec_core::settings::ChannelMode;
        assert_eq!(
            build_route_plan(ChannelMode::MonoL, Some(4), Some(5), 8),
            vec![ChannelRoute::Pick(4)]
        );
        assert_eq!(
            build_route_plan(ChannelMode::MonoR, Some(4), Some(5), 8),
            vec![ChannelRoute::Pick(5)]
        );
        assert_eq!(
            build_route_plan(ChannelMode::MonoMix, None, None, 8),
            vec![ChannelRoute::MixHalf(0, 1)]
        );
    }

    #[test]
    fn route_plan_clamps_out_of_range_picks() {
        use sundayrec_core::settings::ChannelMode;
        // A stale settings pick of channel 30 on a 2-channel device clamps to 1,
        // so the callback can never read out of bounds.
        let p = build_route_plan(ChannelMode::Stereo, Some(30), Some(31), 2);
        assert_eq!(p, vec![ChannelRoute::Pick(1), ChannelRoute::Pick(1)]);
    }

    #[test]
    fn route_frame_picks_and_mixes() {
        // A 4-channel interleaved frame.
        let frame = [0.1f32, 0.2, 0.3, 0.4];
        let mut out = Vec::new();
        // Stereo picking channels 2 & 3.
        route_frame(
            &[ChannelRoute::Pick(2), ChannelRoute::Pick(3)],
            &frame,
            &mut out,
        );
        assert_eq!(out, vec![0.3, 0.4]);
        // MixHalf averages.
        let mut out2 = Vec::new();
        route_frame(&[ChannelRoute::MixHalf(0, 1)], &frame, &mut out2);
        assert!((out2[0] - 0.15).abs() < 1e-6);
        // Out-of-range index yields silence, never a panic.
        let mut out3 = Vec::new();
        route_frame(&[ChannelRoute::Pick(99)], &frame, &mut out3);
        assert_eq!(out3, vec![0.0]);
    }

    // ── F2-W8: asking ASIO only when it can change the answer ────────────────

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn device(name: &str) -> AsioDevice {
        AsioDevice {
            id: name.into(),
            name: name.into(),
            backend: AudioBackendKind::Asio,
            input_channels: 32,
            output_channels: 32,
            default_sample_rate: 48_000,
            supported_sample_rates: vec![48_000],
        }
    }

    #[test]
    fn needs_probe_is_false_for_a_name_the_host_already_knows() {
        let host = names(&["Mikrofon (Realtek(R) Audio)", "Line In (Focusrite USB)"]);
        assert!(!needs_asio_probe("Mikrofon (Realtek(R) Audio)", &host));
    }

    #[test]
    fn needs_probe_is_true_for_a_name_the_host_does_not_know() {
        let host = names(&["Mikrofon (Realtek(R) Audio)"]);
        assert!(needs_asio_probe("Focusrite USB ASIO", &host));
    }

    #[test]
    fn needs_probe_is_true_when_the_host_list_is_empty() {
        // Knowing nothing about the host's devices is not evidence that the
        // name isn't ASIO — an empty list must not short-circuit to "no".
        assert!(needs_asio_probe("Focusrite USB ASIO", &[]));
    }

    #[test]
    fn needs_probe_compares_case_insensitively() {
        // Windows spells the same interface with different casing in its WASAPI
        // and ASIO views; a casing difference must not force a driver sweep.
        let host = names(&["MIKROFON (Realtek(R) Audio)"]);
        assert!(!needs_asio_probe("mikrofon (realtek(r) audio)", &host));
    }

    #[test]
    fn needs_probe_is_false_for_an_empty_name() {
        // Nothing configured: no driver reports "" as its name, so a sweep
        // could only ever answer "no".
        assert!(!needs_asio_probe("", &[]));
        assert!(!needs_asio_probe("   ", &names(&["Some device"])));
    }

    /// The start path's decision, with both lookups counted. A WASAPI name must
    /// never reach the ASIO host — that sweep is what loads every installed
    /// driver.
    #[test]
    fn start_path_with_a_wasapi_name_never_asks_the_asio_host() {
        use std::cell::Cell;
        let host_calls = Cell::new(0u32);
        let asio_calls = Cell::new(0u32);

        let is_asio = resolve_is_asio_device(
            true, // pretend Windows + the `asio` feature
            "Mikrofon (Realtek(R) Audio)",
            None, // cold cache — the scheduler's first start after boot
            || {
                host_calls.set(host_calls.get() + 1);
                names(&["Mikrofon (Realtek(R) Audio)", "Stereo Mix"])
            },
            || {
                asio_calls.set(asio_calls.get() + 1);
                names(&["ASIO4ALL v2"])
            },
        );

        assert!(!is_asio, "a WASAPI device is not the ASIO path");
        assert_eq!(asio_calls.get(), 0, "the ASIO host must not be touched");
        assert_eq!(host_calls.get(), 1, "one cheap host enumeration");
    }

    #[test]
    fn a_device_answers_to_both_its_name_and_its_id() {
        // The membership test this replaced was `d.name == name || d.id == name`;
        // the two are the same string today, but the contract is kept.
        let mut d = device("Soundcraft MADI USB");
        d.id = "asio:madi-0".into();
        let names = device_names(std::slice::from_ref(&d));
        assert!(contains_name(&names, "Soundcraft MADI USB"));
        assert!(contains_name(&names, "asio:madi-0"));
        assert!(!contains_name(&names, "USB Audio CODEC"));
    }

    #[test]
    fn channel_listing_reads_the_memoised_device_list() {
        // `list_asio_input_channels` used to run its own `host.devices()` sweep
        // — a second, unconditional way to load every installed driver. It now
        // looks the count up in the ONE memoised list, and gives the same
        // answer the walk did.
        let mut d = device("Soundcraft MADI USB");
        d.input_channels = 32;
        d.id = "asio:madi-0".into();
        let devices = vec![d];

        assert_eq!(
            input_channels_of(&devices, "Soundcraft MADI USB").len(),
            32,
            "found by name"
        );
        assert_eq!(
            input_channels_of(&devices, "asio:madi-0").len(),
            32,
            "and by id"
        );
        assert_eq!(
            input_channels_of(&devices, "SOUNDCRAFT MADI USB").len(),
            32,
            "casing must not lose the device"
        );
        assert_eq!(
            input_channels_of(&devices, "Mikrofon (Realtek(R) Audio)"),
            Vec::new(),
            "a device the ASIO host does not have has no ASIO channels"
        );
        assert!(input_channels_of(&[], "anything").is_empty());
        // The labels are the shared ones, one-based.
        assert_eq!(
            input_channels_of(&devices, "asio:madi-0")[8].label,
            "Input 9",
            "channel 9/10 of the mixer is exactly what ASIO is here for"
        );
    }

    #[test]
    fn an_unknown_name_falls_through_to_the_asio_sweep() {
        use std::cell::Cell;
        let asio_calls = Cell::new(0u32);
        let is_asio = resolve_is_asio_device(
            true,
            "ASIO4ALL v2",
            None,
            || names(&["Mikrofon (Realtek(R) Audio)"]),
            || {
                asio_calls.set(asio_calls.get() + 1);
                names(&["ASIO4ALL v2"])
            },
        );
        assert!(is_asio);
        assert_eq!(asio_calls.get(), 1);
    }

    #[test]
    fn a_fresh_memo_answers_without_any_lookup_at_all() {
        use std::cell::Cell;
        let host_calls = Cell::new(0u32);
        let asio_calls = Cell::new(0u32);
        let bump = |c: &Cell<u32>| c.set(c.get() + 1);

        // The picker enumerated ASIO seconds ago; record is pressed. The memo
        // answers both ways — including for the pro card whose WASAPI shadow
        // shares its name, which is exactly the case the host short-circuit
        // would get wrong.
        let is_asio = resolve_is_asio_device(
            true,
            "Soundcraft MADI USB",
            Some(names(&["Soundcraft MADI USB"])),
            || {
                bump(&host_calls);
                names(&["Soundcraft MADI USB"])
            },
            || {
                bump(&asio_calls);
                Vec::new()
            },
        );
        assert!(is_asio, "the memo says this name IS an ASIO device");
        assert_eq!(host_calls.get(), 0);
        assert_eq!(asio_calls.get(), 0);

        let is_asio = resolve_is_asio_device(
            true,
            "Mikrofon (Realtek(R) Audio)",
            Some(names(&["Soundcraft MADI USB"])),
            || {
                bump(&host_calls);
                Vec::new()
            },
            || {
                bump(&asio_calls);
                Vec::new()
            },
        );
        assert!(!is_asio);
        assert_eq!(host_calls.get(), 0);
        assert_eq!(asio_calls.get(), 0);
    }

    #[test]
    fn an_unreachable_asio_backend_costs_nothing() {
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        // macOS, or Windows without the `asio` feature: no lookup may happen.
        let is_asio = resolve_is_asio_device(
            false,
            "Whatever",
            None,
            || {
                calls.set(calls.get() + 1);
                Vec::new()
            },
            || {
                calls.set(calls.get() + 1);
                Vec::new()
            },
        );
        assert!(!is_asio);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn an_empty_device_name_costs_nothing() {
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        let is_asio = resolve_is_asio_device(
            true,
            "",
            None,
            || {
                calls.set(calls.get() + 1);
                Vec::new()
            },
            || {
                calls.set(calls.get() + 1);
                Vec::new()
            },
        );
        assert!(!is_asio);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn two_calls_inside_the_ttl_sweep_the_drivers_once() {
        use std::cell::Cell;
        let sweeps = Cell::new(0u32);
        let probe = || {
            sweeps.set(sweeps.get() + 1);
            vec![device("ASIO4ALL v2")]
        };
        let cache = AsioCache::new(Duration::from_secs(30));
        let t0 = Instant::now();

        let first = cache.get_or_fill_at(t0, probe);
        let second = cache.get_or_fill_at(t0 + Duration::from_secs(29), probe);

        assert_eq!(sweeps.get(), 1, "the second call came out of the memo");
        assert_eq!(first, second);
    }

    #[test]
    fn a_call_past_the_ttl_sweeps_again() {
        use std::cell::Cell;
        let sweeps = Cell::new(0u32);
        let probe = || {
            sweeps.set(sweeps.get() + 1);
            vec![device("ASIO4ALL v2")]
        };
        let cache = AsioCache::new(Duration::from_secs(30));
        let t0 = Instant::now();

        let _ = cache.get_or_fill_at(t0, probe);
        let _ = cache.get_or_fill_at(t0 + Duration::from_secs(31), probe);

        assert_eq!(sweeps.get(), 2, "an interface plugged in later shows up");
    }

    #[test]
    fn peek_never_sweeps_and_expires_with_the_ttl() {
        let cache = AsioCache::new(Duration::from_secs(30));
        let t0 = Instant::now();
        assert_eq!(cache.peek_at(t0), None, "cold cache has nothing to offer");

        let _ = cache.get_or_fill_at(t0, || vec![device("ASIO4ALL v2")]);
        assert_eq!(
            cache.peek_at(t0 + Duration::from_secs(29)),
            Some(vec![device("ASIO4ALL v2")])
        );
        assert_eq!(cache.peek_at(t0 + Duration::from_secs(31)), None);
    }

    #[test]
    fn invalidating_the_memo_forces_the_next_sweep() {
        use std::cell::Cell;
        let sweeps = Cell::new(0u32);
        let probe = || {
            sweeps.set(sweeps.get() + 1);
            vec![device("ASIO4ALL v2")]
        };
        let cache = AsioCache::new(Duration::from_secs(30));
        let t0 = Instant::now();

        let _ = cache.get_or_fill_at(t0, probe);
        cache.invalidate();
        let _ = cache.get_or_fill_at(t0, probe);

        assert_eq!(
            sweeps.get(),
            2,
            "diagnose must see the machine as it is now"
        );
    }

    /// The picker's blocking enumeration and a recording start CAN land at the
    /// same moment. ASIO drivers are single-client, so two concurrent
    /// `ASIOInit` sweeps are exactly the failure this module is about: the
    /// second caller must wait for the first and read its result.
    #[test]
    fn two_threads_racing_a_cold_memo_sweep_the_drivers_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Barrier;

        let sweeps = AtomicUsize::new(0);
        let cache = AsioCache::new(Duration::from_secs(30));
        let gate = Barrier::new(2);
        let t0 = Instant::now();

        std::thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    gate.wait();
                    cache.get_or_fill_at(t0, || {
                        sweeps.fetch_add(1, Ordering::SeqCst);
                        // Long enough that a second, unserialised sweep would
                        // certainly have started before this one stored its
                        // result.
                        std::thread::sleep(Duration::from_millis(100));
                        vec![device("ASIO4ALL v2")]
                    });
                });
            }
        });

        assert_eq!(sweeps.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn asio_availability_matches_the_platform_and_feature() {
        // The const the whole decision hangs on. On this (macOS/Linux, or
        // feature-off) lane it must be false, so `is_asio_device` short-circuits
        // before enumerating anything.
        assert_eq!(
            ASIO_AVAILABLE,
            cfg!(all(target_os = "windows", feature = "asio"))
        );
        // And the public entry point agrees, with no device configured.
        assert!(!is_asio_device(""));
    }

    #[test]
    fn backend_kind_serialises_lowercase() {
        assert_eq!(
            serde_json::to_string(&AudioBackendKind::Asio).unwrap(),
            "\"asio\""
        );
        assert_eq!(
            serde_json::to_string(&AudioBackendKind::CoreAudio).unwrap(),
            "\"coreaudio\""
        );
    }
}
