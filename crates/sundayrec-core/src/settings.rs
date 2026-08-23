//! The typed, validated SundayRec settings model.
//!
//! Ported from the Electron build's `src/types/index.ts` `interface Settings`,
//! its defaults in `src/main/store.ts` (the `defaults` object, lines 6+), and
//! the `clampNum` validation it applied on profile import (lines 261+). The
//! Electron code is the behavioural specification; the values, ranges and
//! string enum-tags here mirror it EXACTLY so old/exported settings keep their
//! meaning across the migration.
//!
//! This module is pure: the [`Settings`] struct, its [`Default`] impl, the
//! [`Settings::validate`] clamping pass and [`Settings::from_json_merged`]
//! (partial-JSON-over-defaults parsing) are all deterministic and unit-tested
//! here. The `src-tauri` `settings` layer is the thin persistence/command shell
//! that serialises this to/from the SQLite `app_setting` bag.
//!
//! ## Scope (migration plan, Fase 1)
//!
//! This is the Fase-1 subset of the Electron `Settings`. Fields that belong to
//! later phases are deliberately NOT modelled yet and will be added in their
//! own phase so the model stays honest about what is actually wired:
//!   - `email*` / notify* (notifications)                  → Fase 6
//!   - `editorIntroPath` / `editorOutroPath` (editor)      → Fase 4
//!   - `deviceChannels` (per-device channel maps)          → Fase 2/3
//!   - `video*`, church profile                            → their phases
//!
//! When those land, add the field here with its serde tag matching the Electron
//! key and extend [`Settings::validate`] / [`Default`] accordingly.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::schedule::{ScheduleSlot, SpecialRecording};

/// Input channel layout. Serialised to the EXACT Electron string union
/// (`'stereo' | 'monoL' | 'monoR' | 'monoMix'`, see `types/index.ts:1`), so the
/// tags are camelCase — NOT snake_case — to match stored/exported settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "ChannelMode.ts")]
#[serde(rename_all = "camelCase")]
pub enum ChannelMode {
    /// Both channels, stereo.
    Stereo,
    /// Mono from the left channel only.
    MonoL,
    /// Mono from the right channel only.
    MonoR,
    /// Mono mixed down from both channels.
    MonoMix,
}

/// Capture sample-rate policy. `Auto` (the default) captures at the device's
/// NATIVE rate — the recorder omits `-ar` entirely so ffmpeg never resamples
/// (forcing a 48 kHz `-ar` on a 44.1 kHz USB mixer dropped samples → choppy
/// audio). The explicit rates force that rate via `-ar`. Serialised camelCase
/// (`"auto" | "r44100" | "r48000" | "r96000"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "SampleRate.ts")]
#[serde(rename_all = "camelCase")]
pub enum SampleRate {
    /// Capture at the device's native rate (omit `-ar`).
    Auto,
    /// Force 44.1 kHz.
    R44100,
    /// Force 48 kHz.
    R48000,
    /// Force 96 kHz.
    R96000,
}

/// Output audio container/codec. Serialised lowercase to match the Electron
/// union (`'mp3' | 'wav' | 'flac' | 'aac'`, see `types/index.ts:2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "FileFormat.ts")]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    Mp3,
    Wav,
    Flac,
    Aac,
}

/// How recording filenames are generated. Serialised lowercase to match the
/// Electron union (`'date' | 'church' | 'plain' | 'datetime'`,
/// see `types/index.ts:3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "FilenamePattern.ts")]
#[serde(rename_all = "lowercase")]
pub enum FilenamePattern {
    /// Date only (the Electron default).
    Date,
    /// Liturgical/church name + date.
    Church,
    /// "Gudstjeneste" + date.
    Plain,
    /// Date + time.
    Datetime,
}

/// Which release feed this install follows (E7). Serialised lowercase
/// (`"stable" | "beta"`) — the tag IS the path segment the update Worker serves
/// (`/v1/update/{channel}`, see [`crate::update::channel_feed_url`]), so a
/// renamed variant is a renamed live URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "UpdateChannel.ts")]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Versions that have already been through a real Sunday somewhere. Where
    /// every install stays unless someone deliberately moves it.
    Stable,
    /// Promoted-but-unverified builds — the ring that finds what QA did not,
    /// on machines whose owners accepted that job.
    Beta,
}

impl UpdateChannel {
    /// The stored tag / feed path segment for this channel.
    pub fn as_tag(self) -> &'static str {
        match self {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Beta => "beta",
        }
    }

    /// Parse a stored tag, resolving anything unrecognised to
    /// [`UpdateChannel::Stable`].
    ///
    /// A value we cannot read is a value we cannot trust to mean "this operator
    /// asked for unverified builds", so it can only mean the safe channel —
    /// never beta, and never a load failure (see
    /// [`deserialize_update_channel`]).
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "beta" => UpdateChannel::Beta,
            _ => UpdateChannel::Stable,
        }
    }
}

/// Lenient deserializer for [`Settings::update_channel`].
///
/// This one field needs its own because [`Settings::from_json_merged`] falls
/// back to the FULL defaults the moment ANY field rejects its value: a
/// hand-edited `"canary"` would otherwise reset the save folder, the schedule
/// and every audio setting along with it. Here an unreadable channel costs the
/// channel and nothing else.
fn deserialize_update_channel<'de, D>(de: D) -> Result<UpdateChannel, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(de)?;
    Ok(raw
        .as_str()
        .map(UpdateChannel::parse)
        .unwrap_or_else(default_update_channel))
}

/// Lenient per-field deserializer (R4): tolerate a malformed VALUE by taking
/// the field's `Default` instead of failing the whole blob.
///
/// Same reasoning as [`deserialize_update_channel`]: [`Settings::from_json_merged`]
/// falls back to the FULL defaults the moment ANY field rejects its value, so a
/// hand-edited or drifted value in one of the R4 fields would otherwise reset
/// the save folder, the schedule and every audio setting along with it. Here a
/// bad value costs that one field and nothing else. Fields whose default is not
/// `T::default()` (e.g. `update_channel`'s `Stable`) are subsequently
/// normalised by [`Settings::validate`], which every load/save runs.
fn lenient<'de, D, T>(de: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let raw = serde_json::Value::deserialize(de)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// A per-device input-channel pair — which two native device channels feed the
/// LEFT/RIGHT of a stereo recording (e.g. an X32's 16/17). Keyed by device id in
/// [`Settings::device_channels`]; the flat [`Settings::input_channel_l`]/`_r`
/// the recorder reads are DERIVED from this map in [`Settings::validate`].
/// Serialised camelCase to match the Electron `DeviceChannels`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "DeviceChannels.ts")]
#[serde(rename_all = "camelCase")]
pub struct DeviceChannels {
    /// 0-based device channel routed to the LEFT output. Clamped 0..=31.
    #[serde(default)]
    pub channel_l: i32,
    /// 0-based device channel routed to the RIGHT output. Clamped 0..=31.
    #[serde(default)]
    pub channel_r: i32,
}

/// The complete (Fase-1 subset) settings model.
///
/// Every field carries `#[serde(default)]` so a partial or older JSON blob
/// deserialises by filling in the per-field [`Default`] — this is the Electron
/// `store.get(key, default)` semantics, see [`Settings::from_json_merged`].
/// Numeric ranges are enforced separately by [`Settings::validate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "Settings.ts")]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    // ── System ──────────────────────────────────────────────────────────────
    /// UI language code (e.g. `"no"`, `"en"`), or `None` to follow the OS.
    #[serde(default = "default_language")]
    pub language: Option<String>,
    /// Has the user completed onboarding?
    #[serde(default)]
    pub onboarding_done: bool,

    // ── Audio device ─────────────────────────────────────────────────────────
    /// Stored capture device id (browser `MediaDeviceInfo.deviceId` heritage).
    #[serde(default)]
    pub device_id: Option<String>,
    /// Stored capture device human-readable name (the device-match moat input).
    #[serde(default)]
    pub device_name: Option<String>,
    /// Per-DEVICE channel routing, keyed by device id (R4). This is the SOURCE
    /// the channel grid writes; the flat `input_channel_l`/`_r` the recorder
    /// reads are derived from it for the selected device in
    /// [`Settings::validate`], so switching devices switches routing with it
    /// (the api-shim bridge used to do this flattening renderer-side).
    #[serde(default, deserialize_with = "lenient")]
    pub device_channels: HashMap<String, DeviceChannels>,

    // ── Video device (F2.1 — "alt som mater opptak") ─────────────────────────
    /// Capture video (camera) alongside audio? Default false (audio-only is the
    /// common church-recording case).
    #[serde(default)]
    pub video_enabled: bool,
    /// Stored camera human-readable name — the device-match moat input for video
    /// (matched with [`crate::device_enum::find_best_video_device_match`]).
    #[serde(default)]
    pub video_device_name: Option<String>,
    /// Last-known avfoundation index for the chosen camera. A fallback for when
    /// the name lookup fails (e.g. after a reconnect); the name match wins when
    /// it succeeds. dshow cameras are addressed by name, so this stays `None`.
    #[serde(default)]
    pub video_device_index: Option<i32>,
    // (v0.15 — «lyd + video, ett valg»: `videoResolution`, `videoFramerate`,
    // `videoContainer`, `videoCodec` and `videoEncoder` left the settings. They
    // are the constants in `crate::capture` now — 1080p / 30 fps / mp4 / H.264 /
    // hardware-where-available — with the argument for each beside it.)
    /// Mirror the camera horizontally (preview + recording). Default false.
    /// Electron `videoFlip` — handy for front-facing / mirrored stage cameras.
    /// Kept: it is a per-machine preference the Home preview toggle persists.
    #[serde(default)]
    pub video_flip: bool,
    /// Also keep the standalone high-quality audio file next to a combined MP4?
    /// Default TRUE (R4): the renderer's default has always been «behold også
    /// ren lydfil», and the api-shim bridge synced that `true` into sqlite on
    /// every boot — so `true` is the deployed behaviour on every install. The
    /// old `false` here was only ever visible to code reading defaults before
    /// the first bridge sync.
    #[serde(default = "default_true")]
    pub keep_separate_audio: bool,
    /// Windows ONLY escape hatch: force the legacy ffmpeg **DirectShow** audio
    /// capture instead of the modern cpal (WASAPI/ASIO) path. Default `false` —
    /// cpal is the standard Windows capture (more stable, full multichannel via
    /// ASIO). Flip on only if cpal misbehaves on a specific rig. No effect on macOS.
    #[serde(default)]
    pub classic_directshow: bool,
    /// Escape hatch: force the legacy **ffmpeg** audio capture (avfoundation on
    /// macOS) instead of the native cpal engine that records the WAV directly.
    /// Default `false` — the native engine is the standard path (avfoundation
    /// measurably drops samples below ffmpeg's observability; the 2026-08-01
    /// rebuild). Flip on only if the native engine misbehaves on a specific rig;
    /// scheduled for removal once the rig has verified 0 % loss.
    #[serde(default)]
    pub classic_ffmpeg_audio: bool,
    /// Escape hatch: force the legacy **ffmpeg** pre-roll buffer (a 90-second
    /// rolling avfoundation/dshow capture) instead of the native cpal one.
    /// Default `false` — the native buffer holds the device with the same stack
    /// the recorder uses, harvests its clip by byte copy, and doubles as the
    /// `vu://levels` emitter so the meters and the buffer can coexist. Flip on
    /// only if the native buffer misbehaves on a specific rig.
    #[serde(default)]
    pub classic_ffmpeg_preroll: bool,
    // (v0.15: `separateAudioFormat` left — the separate audio sidecar now
    // follows `format`, the one audio-format choice the app has; `avSync`,
    // `videoBitrate` and `outputMode` left as dead fields with no reader. Old
    // blobs carrying them are dropped tolerantly like every retired key; see
    // `legacy_blob_with_v015_dead_fields_imports_cleanly`.)

    // ── Audio processing ───────────────────────────────────────────────────────
    /// Input channel layout.
    #[serde(default = "default_channels")]
    pub channels: ChannelMode,
    /// Explicit 0-based input channel to record into the LEFT output channel.
    /// `None` keeps the `channels`-mode default routing. Set for multi-channel
    /// mixers (e.g. an X32) where you want to record specific channels (17 & 18).
    /// Clamped 0..=31 in `validate()`. Only honoured for `ChannelMode::Stereo`.
    #[serde(default)]
    pub input_channel_l: Option<i32>,
    /// Explicit 0-based input channel to record into the RIGHT output channel.
    /// See [`Settings::input_channel_l`].
    #[serde(default)]
    pub input_channel_r: Option<i32>,
    // (v0.15: the legacy numeric `sampleRate` field left. Nothing read it since
    // `sample_rate_mode` arrived; an old profile still carrying it imports
    // cleanly — serde ignores the key — see
    // `legacy_blob_with_v015_dead_fields_imports_cleanly`.)
    /// How the capture sample rate is chosen. `Auto` (default) captures at the
    /// device's native rate (no resample → no choppiness); the explicit variants
    /// force a rate. This is what the recorder actually consults.
    #[serde(default = "default_sample_rate_mode")]
    pub sample_rate_mode: SampleRate,
    // (v0.15: `inputVolume`, the EQ trio, the compressor quartet and the limiter
    // pair left. They were the Electron capture-chain knobs; the Tauri recorder
    // has recorded RAW since v4.31 — dynamics/EQ live in the editor — and no
    // UI or reader ever consulted them here.)

    // ── Output ─────────────────────────────────────────────────────────────────
    /// Output file format. Default mp3.
    #[serde(default = "default_format")]
    pub format: FileFormat,
    /// Bitrate (kbps) as a string, matching the Electron `'192'` heritage.
    #[serde(default = "default_bitrate")]
    pub bitrate: String,
    /// Filename generation pattern. Default `date`.
    #[serde(default = "default_filename_pattern")]
    pub filename_pattern: FilenamePattern,
    /// Folder recordings are written to, or `None` for the default location.
    #[serde(default)]
    pub save_folder: Option<String>,
    /// Auto-delete recordings older than N days. Valid 0..=3650, 0 = off.
    #[serde(default)]
    pub auto_delete_days: i32,

    // ── Recording behaviour ──────────────────────────────────────────────────
    /// Stop the recording after a sustained silent stretch?
    #[serde(default)]
    pub stop_on_silence: bool,
    /// Silence detection threshold in dBFS. Valid -90..=0, default -50.
    #[serde(default = "default_silence_threshold")]
    pub silence_threshold: i32,
    /// Minutes of silence before auto-stop. Valid 1..=120, default 5.
    #[serde(default = "default_silence_timeout_minutes")]
    pub silence_timeout_minutes: i32,
    /// Auto-split interval in minutes. Valid 0..=480, 0 = off.
    #[serde(default)]
    pub split_minutes: i32,
    /// Auto-stop manual recordings after N minutes. Valid 0..=1440, 0 = off.
    #[serde(default)]
    pub manual_max_minutes: i32,
    /// Pre-roll buffer in seconds. Valid 0..=60, 0 = off. Default **15** (P1b).
    ///
    /// This is the ONE control the redesigned Advanced screen shows for
    /// pre-roll, and 0 on it means off. It defaults to 15 because the owner's
    /// choice for «Frivilligen først» is «pre-roll on and invisible»: the
    /// twelve seconds between «the service started» and «somebody pressed
    /// Start» are the ones nobody can record twice.
    ///
    /// A profile written before this change carries its own value (usually 0)
    /// and keeps it — only a profile with no key at all, i.e. a fresh install,
    /// gets 15.
    #[serde(default = "default_pre_roll_seconds")]
    pub pre_roll_seconds: i32,
    /// ⚠️ **DEPRECATED — stored, never read.** Kept only so an existing profile
    /// survives a round-trip through `settings_save` unchanged.
    ///
    /// It was the advanced opt-in for the ROLLING pre-roll buffer (R4), and the
    /// doc here used to name `preroll-lifecycle.ts` as its gatekeeper. That
    /// renderer is gone: the redesigned Advanced screen shows the SECONDS and
    /// only the seconds, so the seconds had to become the switch — otherwise a
    /// screen saying «15 sekunder» would buffer nothing. `app/state/preroll.ts`
    /// derives `enabled` from `pre_roll_seconds > 0`, and telemetry's
    /// `WireSettings::preroll_enabled` derives it the same way. Nothing reads
    /// THIS field, in Rust or in the shell.
    ///
    /// The old reasoning — "not derivable, or a user who picked a length but
    /// never flipped the switch would get a background capture they never asked
    /// for" — was answered by removing the second control instead: the length
    /// IS the asking now. See `docs/APP-SHELL.md` («Forhåndsbufferen er ÉN
    /// kontroll nå»).
    ///
    /// Do NOT delete the field. Stored profiles carry `prerollEnabled`, and
    /// `Settings` deserialises strictly enough that dropping a key nobody reads
    /// is churn with a migration attached. It costs one bool.
    #[serde(default)]
    pub preroll_enabled: bool,
    // (v0.15: `trimSilence` — a control with no consumer — and `showLiveLevels`
    // — a reader with no control — left. The meters are always on: the
    // recorder's `live_levels` is hardcoded `true` where the opts are built.)
    /// Reminder notification N minutes before a scheduled recording.
    /// Valid 0..=60, 0 = off.
    #[serde(default)]
    pub reminder_minutes: i32,

    // ── System behaviour ─────────────────────────────────────────────────────
    /// Launch the app at OS login?
    #[serde(default)]
    pub launch_at_login: bool,
    /// Wake the machine from sleep for scheduled recordings? Default true.
    #[serde(default = "default_true")]
    pub wake_from_sleep: bool,
    /// Require confirmation before stopping an in-progress recording? Default true.
    #[serde(default = "default_true")]
    pub protect_recording: bool,

    // ── Schedule (Fase 5) ─────────────────────────────────────────────────────
    /// Is the weekly plan ARMED? Default `true`.
    ///
    /// P1b. Before this field an empty `slots` list was the only spelling of
    /// "automatic recording is off", so the UI's off-switch had to DELETE the
    /// time — a switch that throws away data it does not show. The flag
    /// separates "armed" from "configured": turning it off keeps the times and
    /// stops the planning.
    ///
    /// `default = true` and not `false` is the half that matters for existing
    /// installs: a profile written before this field has no key for serde to
    /// read, and `false` would silently disarm every church that already had a
    /// Sunday slot — the exact failure this app exists to prevent. A fresh
    /// profile has no slots anyway, so `true` there arms nothing.
    ///
    /// Only WEEKLY slots are gated. `special_recordings` are dated one-offs
    /// somebody entered by hand for a specific concert; the level-1 switch is
    /// about the recurring plan and never silently cancels those.
    #[serde(default = "default_true")]
    pub auto_record_enabled: bool,
    /// Weekly recurring recording windows. Empty by default. The scheduler
    /// engine turns these into start/stop/reminder/preflight timers; see
    /// [`crate::schedule`] for the decision logic.
    ///
    /// ⚠️ Read them through [`Settings::active_slots`], never directly: that is
    /// the one place `auto_record_enabled` is honoured. The claim was false for
    /// a while — both wake commands read this field raw, so a machine with «Ta
    /// opp automatisk» OFF still woke at 10:50 on a Sunday for a recording the
    /// scheduler would refuse to make, and `wake_verify` then reported the
    /// wakes it had itself cancelled as missing.
    ///
    /// The ONE deliberate raw reader is [`crate::telemetry::WireSettings`]'s
    /// `slot_count`, which reports what is CONFIGURED and carries
    /// `auto_record_enabled` beside it (see that field's doc). Anything else
    /// that reads `slots` directly is a bug in waiting.
    #[serde(default)]
    pub slots: Vec<ScheduleSlot>,
    /// One-off dated recordings (concerts, special services). Empty by default.
    /// Auto-pruned 7 days after they end ([`crate::schedule::prune_specials`]).
    #[serde(default)]
    pub special_recordings: Vec<SpecialRecording>,

    // ── Church profile (R7 — Electron `churchName`/`responsiblePerson`) ────────
    /// Congregation/church name. Drives the `church` filename pattern and the
    /// localized "church" labels. Empty string = unset (matches the Electron
    /// `''` default, not `null`). See `store.ts` `churchName: ''`.
    #[serde(default)]
    pub church_name: String,
    /// Person responsible for recordings (shown in diagnostics + email alerts).
    /// Empty string = unset (Electron `responsiblePerson: ''`).
    #[serde(default)]
    pub responsible_person: String,

    // ── Notifications (R7 — Electron `notifyStart`/`notifyStop`) ───────────────
    /// Fire a native notification when a scheduled recording starts? Default true.
    #[serde(default = "default_true")]
    pub notify_start: bool,
    /// Fire a native notification when a recording stops? Default true.
    #[serde(default = "default_true")]
    pub notify_stop: bool,
    // (The chat webhook — `webhookUrl`/`webhookOnWarning`/`webhookAllowLocal` —
    // was removed with the sharing cluster. Old blobs still carrying the keys
    // are DROPPED tolerantly on the next load/save, like the stream fields
    // below; see `legacy_blob_with_removed_sharing_fields_imports_cleanly`.)

    // ── Email alerts (R7 — Electron `email*`; the SMTP pass lives in the OS ────
    //    keychain, NEVER here — mirrors `store.ts` `setSmtpPassword`) ───────────
    /// Send an email when a recording fails / a scheduled one is missed?
    #[serde(default)]
    pub email_on_error: bool,
    /// Recipient address for alert emails. Empty = unset (Electron `''`).
    #[serde(default)]
    pub email_address: String,
    /// SMTP host. Blank = no transport at all (the Gmail-OAuth alternative left
    /// with the cloud-backup OAuth client). Electron `emailSmtp`.
    #[serde(default)]
    pub email_smtp: String,
    /// SMTP port. Valid 1..=65535, default 587. Electron `emailSmtpPort: 587`.
    #[serde(default = "default_smtp_port")]
    pub email_smtp_port: i32,
    /// SMTP username. Empty = unset (Electron `emailSmtpUser: ''`). The PASSWORD
    /// is intentionally absent — it is stored in the OS keychain by the `email`
    /// seam, never persisted to the settings bag.
    #[serde(default)]
    pub email_smtp_user: String,
    /// Explicit envelope/`From:` address for alert mail. Empty = derive it, which
    /// is what every pre-existing config does: the renderer used to synthesise
    /// `emailSmtpUser || recipient` client-side. Providers increasingly reject a
    /// `From:` that isn't the authenticated identity (or a verified alias), and
    /// the login username is not always a mailbox — SendGrid wants `apikey`,
    /// Fastmail/Migadu use `user@domain` handles — so the address has to be
    /// settable on its own. The derivation stays as the fallback (see
    /// `commands::email::resolve_from_address`) so old configs keep working
    /// untouched.
    #[serde(default)]
    pub email_smtp_from: String,

    // ── Editor intro/outro (R7 — Electron `editorIntroPath`/`editorOutroPath`) ─
    /// Path to an intro clip prepended on export, or `None`. Electron used
    /// `undefined`; we keep it `Option` so an unset value stays absent.
    #[serde(default)]
    pub editor_intro_path: Option<String>,
    /// Path to an outro clip appended on export, or `None`.
    #[serde(default)]
    pub editor_outro_path: Option<String>,
    // (`editorHwEncode` left in v0.15: the editor's video export tries the
    // hardware encoder first wherever the platform has one and falls back to
    // software on a failed render — a toggle for that guarded nothing.)

    // (Live streaming was removed in v0.14, cloud backup with the sharing
    // cluster after it. Old sqlite blobs may still carry `streamDestinations`/
    // `streamResolution`/`streamFramerate`/`streamVideoBitrate`/`streamOverlays`
    // and `cloudGoogleDrive`/`cloudDropbox`/`cloudOneDrive`/`podcast` — serde
    // ignores unknown fields, so they are DROPPED tolerantly on the next
    // load/save. See
    // the tests `legacy_blob_with_stream_fields_imports_cleanly` and
    // `legacy_blob_with_removed_sharing_fields_imports_cleanly`.)

    // ── Misc ─────────────────────────────────────────────────────────────────
    /// Download and install updates automatically? Default true.
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// Which release feed to follow (E7). Default [`UpdateChannel::Stable`];
    /// `Beta` is opted into per machine and is never inherited from an imported
    /// profile's neighbour fields — an unreadable value lands on stable.
    #[serde(
        default = "default_update_channel",
        deserialize_with = "deserialize_update_channel"
    )]
    pub update_channel: UpdateChannel,
    /// Prompt to open the editor after a recording finishes? Default true.
    #[serde(default = "default_true")]
    pub ask_open_editor: bool,
}

// ── Per-field default helpers (so `#[serde(default = "...")]` and the `Default`
//    impl share one source of truth) ──────────────────────────────────────────

fn default_language() -> Option<String> {
    None
}
fn default_channels() -> ChannelMode {
    ChannelMode::Stereo
}
fn default_sample_rate_mode() -> SampleRate {
    SampleRate::Auto
}
fn default_format() -> FileFormat {
    FileFormat::Mp3
}
fn default_bitrate() -> String {
    // 256 kbps: transparent for speech+music, and matches the editor export
    // default (`editor::codec_args`) so a recording isn't re-compressed harder on
    // the way out. The ceiling stays 320 (see `bitrate_kbps`).
    "256".to_string()
}
fn default_filename_pattern() -> FilenamePattern {
    FilenamePattern::Date
}
fn default_silence_threshold() -> i32 {
    -50
}
fn default_silence_timeout_minutes() -> i32 {
    5
}
/// See [`Settings::pre_roll_seconds`] — 15 s, the middle of the 0/15/30 the UI
/// offers, and the length that covers a late Start without holding a minute of
/// audio nobody asked for.
fn default_pre_roll_seconds() -> i32 {
    15
}

fn default_true() -> bool {
    true
}
fn default_smtp_port() -> i32 {
    587
}
fn default_update_channel() -> UpdateChannel {
    UpdateChannel::Stable
}
impl Default for Settings {
    /// The Electron `defaults` object (`store.ts` lines 6+), field-for-field.
    fn default() -> Self {
        Self {
            language: default_language(),
            onboarding_done: false,

            device_id: None,
            device_name: None,
            device_channels: HashMap::new(),

            video_enabled: false,
            video_device_name: None,
            video_device_index: None,
            video_flip: false,
            keep_separate_audio: true,
            classic_directshow: false,
            classic_ffmpeg_audio: false,
            classic_ffmpeg_preroll: false,

            channels: default_channels(),
            input_channel_l: None,
            input_channel_r: None,
            sample_rate_mode: default_sample_rate_mode(),

            format: default_format(),
            bitrate: default_bitrate(),
            filename_pattern: default_filename_pattern(),
            save_folder: None,
            auto_delete_days: 0,

            stop_on_silence: false,
            silence_threshold: default_silence_threshold(),
            silence_timeout_minutes: default_silence_timeout_minutes(),
            split_minutes: 0,
            manual_max_minutes: 0,
            pre_roll_seconds: default_pre_roll_seconds(),
            preroll_enabled: false,
            reminder_minutes: 0,

            launch_at_login: false,
            wake_from_sleep: true,
            protect_recording: true,

            auto_record_enabled: true,
            slots: Vec::new(),
            special_recordings: Vec::new(),

            church_name: String::new(),
            responsible_person: String::new(),

            notify_start: true,
            notify_stop: true,

            email_on_error: false,
            email_address: String::new(),
            email_smtp: String::new(),
            email_smtp_port: default_smtp_port(),
            email_smtp_user: String::new(),
            email_smtp_from: String::new(),

            editor_intro_path: None,
            editor_outro_path: None,

            auto_update: true,
            update_channel: default_update_channel(),
            ask_open_editor: true,
        }
    }
}

/// Clamp an integer to `[min, max]`. (The float twin, `clamp_f64`, left in
/// v0.15 with the compressor/limiter fields — the only non-integer settings.)
fn clamp_i32(v: i32, min: i32, max: i32) -> i32 {
    v.clamp(min, max)
}

impl Settings {
    /// Clamp every numeric field to its valid range, mirroring the Electron
    /// `importProfile` clamping (`store.ts:299+`). Non-numeric/enum/bool fields
    /// are already constrained by their types, so they pass through untouched.
    ///
    /// This is idempotent: validating an already-valid `Settings` is a no-op.
    pub fn validate(&mut self) {
        // Audio processing
        self.input_channel_l = self.input_channel_l.map(|c| clamp_i32(c, 0, 31));
        self.input_channel_r = self.input_channel_r.map(|c| clamp_i32(c, 0, 31));

        // Output
        self.auto_delete_days = clamp_i32(self.auto_delete_days, 0, 3650);

        // Recording behaviour
        self.silence_threshold = clamp_i32(self.silence_threshold, -90, 0);
        self.silence_timeout_minutes = clamp_i32(self.silence_timeout_minutes, 1, 120);
        self.split_minutes = clamp_i32(self.split_minutes, 0, 480);
        self.manual_max_minutes = clamp_i32(self.manual_max_minutes, 0, 1440);
        self.pre_roll_seconds = clamp_i32(self.pre_roll_seconds, 0, 60);
        self.reminder_minutes = clamp_i32(self.reminder_minutes, 0, 60);

        // Email (R7). The SMTP port is the only numeric email field; clamp it to
        // a valid TCP port (Electron left it un-clamped, but a 0/negative port
        // would be a hard ffmpeg/lettre error — clamp defensively).
        self.email_smtp_port = clamp_i32(self.email_smtp_port, 1, 65_535);

        // Per-device channel map (R4): clamp every stored pair to real channel
        // indices, then DERIVE the flat recorder fields from the map. The map is
        // the source of truth whenever it exists at all: the selected device's
        // entry becomes `input_channel_l`/`_r`, and a selected device WITHOUT an
        // entry clears them (default routing) — exactly what the api-shim bridge
        // used to compute, so switching to an unmapped device cannot inherit the
        // previous device's channels. An EMPTY map leaves the flat fields alone,
        // so an older exported profile that only carried `inputChannelL`/`R`
        // keeps working.
        for ch in self.device_channels.values_mut() {
            ch.channel_l = clamp_i32(ch.channel_l, 0, 31);
            ch.channel_r = clamp_i32(ch.channel_r, 0, 31);
        }
        if !self.device_channels.is_empty() {
            if let Some(id) = self.device_id.as_deref() {
                let pair = self.device_channels.get(id);
                self.input_channel_l = pair.map(|p| p.channel_l);
                self.input_channel_r = pair.map(|p| p.channel_r);
            }
        }
    }

    /// Validated copy — convenience for callers that prefer a value.
    pub fn validated(mut self) -> Self {
        self.validate();
        self
    }

    /// The weekly slots the scheduler is allowed to plan on — **the only way
    /// slots should ever be read** outside this module.
    ///
    /// `auto_record_enabled == false` answers with an empty slice instead of
    /// the stored list. One function and not a check at each call site: the
    /// scheduler reads slots in six places (next start, wake horizon, reminder
    /// events, the late-start window, the missed-check and the status command),
    /// and a flag honoured in five of six is a machine that wakes at 10:50 on a
    /// Sunday for a recording it will then refuse to make.
    ///
    /// Indices stay meaningful: a `TriggerKind::Slot(i)` produced from this
    /// slice must be resolved against this slice too — which is automatic,
    /// because "all of them" and "none of them" are the only two answers.
    ///
    /// Specials are deliberately NOT gated; see [`Settings::auto_record_enabled`].
    pub fn active_slots(&self) -> &[ScheduleSlot] {
        if self.auto_record_enabled {
            &self.slots
        } else {
            &[]
        }
    }

    /// The lossy-codec bitrate in kbps, parsed from the Electron-heritage
    /// `bitrate` String and clamped to a sane CBR range. Any unparseable / empty /
    /// out-of-range value falls back to 256 kbps so the recorder never receives a
    /// nonsense `-b:a`. (PCM/FLAC ignore this entirely.)
    pub fn bitrate_kbps(&self) -> u32 {
        self.bitrate
            .trim()
            .trim_end_matches(['k', 'K'])
            .parse::<u32>()
            .ok()
            .map(|k| k.clamp(32, 320))
            .unwrap_or(256)
    }

    /// The capture sample rate the recorder should use, derived from
    /// [`Settings::sample_rate_mode`]. `Auto` → `None` (omit `-ar`, capture at the
    /// device's native rate → no resample → no choppiness); the explicit variants
    /// → `Some(hz)`. This is the recorder's source of truth (the legacy numeric
    /// `sampleRate` field left in v0.15).
    pub fn resolved_sample_rate(&self) -> Option<u32> {
        match self.sample_rate_mode {
            SampleRate::Auto => None,
            SampleRate::R44100 => Some(44_100),
            SampleRate::R48000 => Some(48_000),
            SampleRate::R96000 => Some(96_000),
        }
    }

    /// Parse a (possibly partial or older) settings JSON blob, MERGING it over
    /// the defaults: any missing or unknown field falls back to its default,
    /// matching the Electron `store.get(key, default)` semantics. A malformed
    /// blob (not a JSON object) falls back to the full defaults rather than
    /// erroring, so a corrupt store never bricks the app.
    ///
    /// The returned value is NOT yet validated — call [`Settings::validate`]
    /// (the persistence layer does this).
    pub fn from_json_merged(value: &str) -> Settings {
        serde_json::from_str::<Settings>(value).unwrap_or_default()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Save-folder resolution — THE canonical resolver
// ─────────────────────────────────────────────────────────────────────────────

/// The default folder name under the OS Documents directory, i.e. the
/// `<Documents>/SundayRec` every screen and background job means when no
/// `save_folder` is configured (mirrors Electron `preflight.ts:39`).
pub const DEFAULT_SAVE_SUBFOLDER: &str = "SundayRec";

/// No save folder could be resolved: nothing is configured AND the platform
/// could not report a usable Documents (or app-data) directory. The message
/// leads with the stable `no_save_folder` snake code the renderer localizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoSaveFolder;

impl std::fmt::Display for NoSaveFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "no_save_folder: no folder is configured and the OS reported no Documents directory",
        )
    }
}

impl std::error::Error for NoSaveFolder {}

/// Resolve the effective save folder — the ONE rule every caller shares
/// (recorder, scheduler, preflight, diagnostics, prune, trash sweep, disk
/// probes, path guards):
///
///   - a non-blank configured `save_folder` wins verbatim;
///   - otherwise the default is `<documents_dir>/SundayRec` — the SUBFOLDER,
///     never the bare Documents directory (three pre-R3 callers pruned/swept
///     the PARENT because they skipped the join);
///   - no configured folder and no usable Documents dir is an ERROR — never a
///     silent `"."`, which would point destructive jobs at whatever the
///     process's working directory happens to be.
pub fn resolve_save_folder(
    save_folder: Option<&str>,
    documents_dir: Option<&std::path::Path>,
) -> Result<std::path::PathBuf, NoSaveFolder> {
    if let Some(f) = save_folder {
        if !f.trim().is_empty() {
            return Ok(std::path::PathBuf::from(f));
        }
    }
    match documents_dir {
        Some(d) if !d.as_os_str().is_empty() => Ok(d.join(DEFAULT_SAVE_SUBFOLDER)),
        _ => Err(NoSaveFolder),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_electron() {
        let s = Settings::default();
        // System
        assert_eq!(s.language, None);
        assert!(!s.onboarding_done);
        // Audio device
        assert_eq!(s.device_id, None);
        assert_eq!(s.device_name, None);
        // Video device
        assert!(!s.video_enabled);
        assert_eq!(s.video_device_name, None);
        assert_eq!(s.video_device_index, None);
        assert!(!s.video_flip);
        // R4: true — the deployed (bridge-synced) renderer default, see the field doc.
        assert!(s.keep_separate_audio);
        // Audio processing
        assert_eq!(s.channels, ChannelMode::Stereo);
        assert_eq!(s.sample_rate_mode, SampleRate::Auto);
        // Output
        assert_eq!(s.format, FileFormat::Mp3);
        assert_eq!(s.bitrate, "256");
        assert_eq!(s.filename_pattern, FilenamePattern::Date);
        assert_eq!(s.save_folder, None);
        assert_eq!(s.auto_delete_days, 0);
        // Recording behaviour
        assert!(!s.stop_on_silence);
        assert_eq!(s.silence_threshold, -50);
        assert_eq!(s.silence_timeout_minutes, 5);
        assert_eq!(s.split_minutes, 0);
        assert_eq!(s.manual_max_minutes, 0);
        // P1b: pre-roll is ON and invisible — 15 s is the default for a fresh
        // profile. (Electron's 0 lives on in every profile already written.)
        assert_eq!(s.pre_roll_seconds, 15);
        assert_eq!(s.reminder_minutes, 0);
        // System behaviour
        assert!(!s.launch_at_login);
        assert!(s.wake_from_sleep);
        assert!(s.protect_recording);
        // Schedule (Fase 5)
        assert!(s.slots.is_empty());
        assert!(s.special_recordings.is_empty());
        // Church profile (R7) — Electron `''` not `null`.
        assert_eq!(s.church_name, "");
        assert_eq!(s.responsible_person, "");
        // Notifications (R7)
        assert!(s.notify_start);
        assert!(s.notify_stop);
        // Email (R7)
        assert!(!s.email_on_error);
        assert_eq!(s.email_address, "");
        assert_eq!(s.email_smtp, "");
        assert_eq!(s.email_smtp_port, 587);
        assert_eq!(s.email_smtp_user, "");
        // Editor intro/outro (R7)
        assert_eq!(s.editor_intro_path, None);
        assert_eq!(s.editor_outro_path, None);
        // Misc
        assert!(s.auto_update);
        assert!(s.ask_open_editor);
    }

    #[test]
    fn validate_clamps_smtp_port() {
        let mut over = Settings {
            email_smtp_port: 999_999,
            ..Default::default()
        };
        over.validate();
        assert_eq!(over.email_smtp_port, 65_535);

        let mut under = Settings {
            email_smtp_port: 0,
            ..Default::default()
        };
        under.validate();
        assert_eq!(under.email_smtp_port, 1);
    }

    #[test]
    fn r7_fields_merge_from_partial_json() {
        // A partial blob carrying only the new R7 keys fills the rest from
        // defaults (Electron `store.get(key, default)` semantics).
        let s = Settings::from_json_merged(
            r#"{"churchName":"Domkirken","emailOnError":true,"emailAddress":"a@b.no"}"#,
        );
        assert_eq!(s.church_name, "Domkirken");
        assert!(s.email_on_error);
        assert_eq!(s.email_address, "a@b.no");
        // Untouched field keeps its default.
        assert_eq!(s.email_smtp_port, 587);
        assert!(s.notify_start);
    }

    #[test]
    fn bitrate_kbps_parses_clamps_and_defaults() {
        let mk = |b: &str| Settings {
            bitrate: b.into(),
            ..Default::default()
        };
        assert_eq!(mk("192").bitrate_kbps(), 192);
        assert_eq!(mk("320").bitrate_kbps(), 320);
        assert_eq!(mk("256k").bitrate_kbps(), 256, "tolerates a trailing k");
        assert_eq!(mk("999").bitrate_kbps(), 320, "clamps above the ceiling");
        assert_eq!(mk("16").bitrate_kbps(), 32, "clamps below the floor");
        assert_eq!(mk("").bitrate_kbps(), 256, "empty → safe default");
        assert_eq!(mk("abc").bitrate_kbps(), 256, "garbage → safe default");
    }

    #[test]
    fn resolved_sample_rate_maps_modes() {
        let mk = |m: SampleRate| Settings {
            sample_rate_mode: m,
            ..Default::default()
        };
        assert_eq!(mk(SampleRate::Auto).resolved_sample_rate(), None);
        assert_eq!(mk(SampleRate::R44100).resolved_sample_rate(), Some(44_100));
        assert_eq!(mk(SampleRate::R48000).resolved_sample_rate(), Some(48_000));
        assert_eq!(mk(SampleRate::R96000).resolved_sample_rate(), Some(96_000));
        // Default settings = Auto = native (None).
        assert_eq!(Settings::default().resolved_sample_rate(), None);
    }

    #[test]
    fn sample_rate_mode_serde_matches_camel_case() {
        assert_eq!(
            serde_json::to_string(&SampleRate::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::to_string(&SampleRate::R44100).unwrap(),
            "\"r44100\""
        );
        assert_eq!(
            serde_json::to_string(&SampleRate::R48000).unwrap(),
            "\"r48000\""
        );
        assert_eq!(
            serde_json::to_string(&SampleRate::R96000).unwrap(),
            "\"r96000\""
        );
        let back: SampleRate = serde_json::from_str("\"r96000\"").unwrap();
        assert_eq!(back, SampleRate::R96000);
    }

    #[test]
    fn sample_rate_mode_merges_from_partial_json_and_defaults_to_auto() {
        let s = Settings::from_json_merged(r#"{"sampleRateMode":"r44100"}"#);
        assert_eq!(s.sample_rate_mode, SampleRate::R44100);
        assert_eq!(s.resolved_sample_rate(), Some(44_100));
        // An older blob without the key defaults to Auto (native).
        let legacy = Settings::from_json_merged(r#"{"sampleRate":44100}"#);
        assert_eq!(legacy.sample_rate_mode, SampleRate::Auto);
        assert_eq!(legacy.resolved_sample_rate(), None);
    }

    #[test]
    fn validate_clamps_input_channels() {
        let mut s = Settings {
            input_channel_l: Some(99),
            input_channel_r: Some(-5),
            ..Default::default()
        };
        s.validate();
        assert_eq!(s.input_channel_l, Some(31));
        assert_eq!(s.input_channel_r, Some(0));

        // None stays None (mode-default routing).
        let mut none = Settings::default();
        none.validate();
        assert_eq!(none.input_channel_l, None);
        assert_eq!(none.input_channel_r, None);
    }

    #[test]
    fn validate_clamps_silence_threshold_and_timeout() {
        let mut s = Settings {
            silence_threshold: 50,
            silence_timeout_minutes: 999,
            ..Default::default()
        };
        s.validate();
        assert_eq!(s.silence_threshold, 0);
        assert_eq!(s.silence_timeout_minutes, 120);

        let mut lo = Settings {
            silence_threshold: -200,
            silence_timeout_minutes: 0,
            ..Default::default()
        };
        lo.validate();
        assert_eq!(lo.silence_threshold, -90);
        assert_eq!(lo.silence_timeout_minutes, 1);
    }

    #[test]
    fn validate_clamps_split_manual_preroll_reminder() {
        let mut s = Settings {
            split_minutes: 9_999,
            manual_max_minutes: 99_999,
            pre_roll_seconds: 600,
            reminder_minutes: 600,
            ..Default::default()
        };
        s.validate();
        assert_eq!(s.split_minutes, 480);
        assert_eq!(s.manual_max_minutes, 1440);
        assert_eq!(s.pre_roll_seconds, 60);
        assert_eq!(s.reminder_minutes, 60);
    }

    #[test]
    fn validate_clamps_auto_delete_days() {
        let mut over = Settings {
            auto_delete_days: 100_000,
            ..Default::default()
        };
        over.validate();
        assert_eq!(over.auto_delete_days, 3650);

        let mut under = Settings {
            auto_delete_days: -5,
            ..Default::default()
        };
        under.validate();
        assert_eq!(under.auto_delete_days, 0);
    }

    #[test]
    fn validate_is_idempotent_on_defaults() {
        let mut s = Settings::default();
        let before = s.clone();
        s.validate();
        assert_eq!(s, before);
    }

    #[test]
    fn channel_mode_serde_matches_electron_strings() {
        // camelCase tags, NOT snake_case — must match types/index.ts:1.
        assert_eq!(
            serde_json::to_string(&ChannelMode::Stereo).unwrap(),
            "\"stereo\""
        );
        assert_eq!(
            serde_json::to_string(&ChannelMode::MonoL).unwrap(),
            "\"monoL\""
        );
        assert_eq!(
            serde_json::to_string(&ChannelMode::MonoR).unwrap(),
            "\"monoR\""
        );
        assert_eq!(
            serde_json::to_string(&ChannelMode::MonoMix).unwrap(),
            "\"monoMix\""
        );
        // round-trip
        let back: ChannelMode = serde_json::from_str("\"monoMix\"").unwrap();
        assert_eq!(back, ChannelMode::MonoMix);
    }

    #[test]
    fn file_format_and_pattern_serde_match_electron_strings() {
        assert_eq!(serde_json::to_string(&FileFormat::Mp3).unwrap(), "\"mp3\"");
        assert_eq!(serde_json::to_string(&FileFormat::Wav).unwrap(), "\"wav\"");
        assert_eq!(
            serde_json::to_string(&FileFormat::Flac).unwrap(),
            "\"flac\""
        );
        assert_eq!(serde_json::to_string(&FileFormat::Aac).unwrap(), "\"aac\"");

        assert_eq!(
            serde_json::to_string(&FilenamePattern::Date).unwrap(),
            "\"date\""
        );
        assert_eq!(
            serde_json::to_string(&FilenamePattern::Church).unwrap(),
            "\"church\""
        );
        assert_eq!(
            serde_json::to_string(&FilenamePattern::Plain).unwrap(),
            "\"plain\""
        );
        assert_eq!(
            serde_json::to_string(&FilenamePattern::Datetime).unwrap(),
            "\"datetime\""
        );
    }

    #[test]
    fn settings_field_keys_serialise_as_camel_case() {
        // The JSON keys must match the Electron `Settings` interface (camelCase),
        // so an exported profile interoperates with the old build.
        let json = serde_json::to_value(Settings::default()).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("onboardingDone"));
        assert!(obj.contains_key("deviceName"));
        assert!(obj.contains_key("videoEnabled"));
        assert!(obj.contains_key("videoDeviceName"));
        assert!(obj.contains_key("videoDeviceIndex"));
        assert!(obj.contains_key("keepSeparateAudio"));
        assert!(obj.contains_key("sampleRateMode"));
        assert!(obj.contains_key("inputChannelL"));
        assert!(obj.contains_key("filenamePattern"));
        assert!(obj.contains_key("stopOnSilence"));
        assert!(obj.contains_key("silenceTimeoutMinutes"));
        assert!(obj.contains_key("autoUpdate"));
        assert!(obj.contains_key("updateChannel"));
        assert!(obj.contains_key("askOpenEditor"));
        // Schedule keys must match the Electron `Settings` interface.
        assert!(obj.contains_key("autoRecordEnabled"));
        assert!(obj.contains_key("slots"));
        assert!(obj.contains_key("specialRecordings"));
    }

    // ── `auto_record_enabled` (P1b) ─────────────────────────────────────────

    #[test]
    fn auto_record_enabled_defaults_on_and_survives_an_older_profile() {
        assert!(Settings::default().auto_record_enabled);

        // A profile written before the field existed: the key is simply absent.
        // `false` here would disarm every church that already had a Sunday slot
        // — the failure this app exists to prevent — so the serde default is
        // `true`, and the stored slots keep planning exactly as they did.
        let older = Settings::from_json_merged(
            r#"{"slots":[{"days":[6],"start":"11:00","stop":"12:30","max":null}]}"#,
        );
        assert!(older.auto_record_enabled, "an older profile stays armed");
        assert_eq!(older.active_slots().len(), 1);

        // An explicit `false` round-trips (it is a real answer, not an absence).
        let off = Settings::from_json_merged(r#"{"autoRecordEnabled":false}"#);
        assert!(!off.auto_record_enabled);
    }

    #[test]
    fn active_slots_is_the_one_place_the_flag_is_honoured() {
        let mut s = Settings {
            slots: vec![ScheduleSlot {
                days: vec![6],
                start: "11:00".into(),
                stop: "12:30".into(),
                max: None,
            }],
            ..Settings::default()
        };
        assert_eq!(s.active_slots().len(), 1);

        s.auto_record_enabled = false;
        assert!(s.active_slots().is_empty(), "disarmed → nothing to plan");
        // …and the times are still in the store. That is the whole point of the
        // field: the switch parks the plan, it does not delete it.
        assert_eq!(s.slots.len(), 1);
        assert_eq!(s.slots[0].start, "11:00");
    }

    #[test]
    fn slots_and_specials_round_trip_through_json() {
        use crate::schedule::{ScheduleSlot, SpecialRecording};
        let original = Settings {
            slots: vec![ScheduleSlot {
                days: vec![6],
                start: "11:00".to_string(),
                stop: "12:30".to_string(),
                max: Some(120),
            }],
            special_recordings: vec![SpecialRecording {
                id: Some("s1".to_string()),
                date: "2026-12-24".to_string(),
                name: "Julaften".to_string(),
                start: "16:00".to_string(),
                stop: "17:00".to_string(),
                device_id: None,
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&original).unwrap();
        let back = Settings::from_json_merged(&json);
        assert_eq!(back.slots, original.slots);
        assert_eq!(back.special_recordings, original.special_recordings);
        // An older blob without the schedule keys defaults them to empty.
        let legacy = Settings::from_json_merged(r#"{ "sampleRate": 44100 }"#);
        assert!(legacy.slots.is_empty());
        assert!(legacy.special_recordings.is_empty());
    }

    #[test]
    fn update_channel_serde_tags_are_the_feed_path_segments() {
        assert_eq!(
            serde_json::to_string(&UpdateChannel::Stable).unwrap(),
            "\"stable\""
        );
        assert_eq!(
            serde_json::to_string(&UpdateChannel::Beta).unwrap(),
            "\"beta\""
        );
        assert_eq!(UpdateChannel::Stable.as_tag(), "stable");
        assert_eq!(UpdateChannel::Beta.as_tag(), "beta");
    }

    #[test]
    fn update_channel_parse_falls_back_to_stable() {
        assert_eq!(UpdateChannel::parse("beta"), UpdateChannel::Beta);
        assert_eq!(UpdateChannel::parse("  BETA "), UpdateChannel::Beta);
        assert_eq!(UpdateChannel::parse("stable"), UpdateChannel::Stable);
        // Anything we cannot read is not a request for unverified builds.
        assert_eq!(UpdateChannel::parse("canary"), UpdateChannel::Stable);
        assert_eq!(UpdateChannel::parse(""), UpdateChannel::Stable);
        assert_eq!(UpdateChannel::parse("bet"), UpdateChannel::Stable);
    }

    #[test]
    fn a_garbage_update_channel_costs_only_the_channel() {
        // The regression this guards: without the lenient deserializer the whole
        // blob would fail and `from_json_merged` would reset EVERY setting.
        let s = Settings::from_json_merged(
            r#"{ "updateChannel": "canary", "silenceThreshold": -40, "format": "flac" }"#,
        );
        assert_eq!(s.update_channel, UpdateChannel::Stable);
        assert_eq!(s.silence_threshold, -40);
        assert_eq!(s.format, FileFormat::Flac);

        // Same for a value that is not even a string.
        let s = Settings::from_json_merged(r#"{ "updateChannel": 3, "silenceThreshold": -40 }"#);
        assert_eq!(s.update_channel, UpdateChannel::Stable);
        assert_eq!(s.silence_threshold, -40);
    }

    #[test]
    fn update_channel_round_trips_and_defaults_to_stable() {
        assert_eq!(Settings::default().update_channel, UpdateChannel::Stable);
        // Absent key → stable.
        assert_eq!(
            Settings::from_json_merged("{}").update_channel,
            UpdateChannel::Stable
        );
        // A deliberate opt-in survives the round trip.
        let beta = Settings {
            update_channel: UpdateChannel::Beta,
            ..Default::default()
        };
        let json = serde_json::to_string(&beta).unwrap();
        assert_eq!(
            Settings::from_json_merged(&json).update_channel,
            UpdateChannel::Beta
        );
    }

    #[test]
    fn from_json_merged_fills_defaults_for_empty_object() {
        let s = Settings::from_json_merged("{}");
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn from_json_merged_falls_back_on_garbage() {
        // Not an object → full defaults rather than a panic.
        assert_eq!(Settings::from_json_merged("not json"), Settings::default());
        assert_eq!(Settings::from_json_merged("42"), Settings::default());
        assert_eq!(Settings::from_json_merged("[]"), Settings::default());
    }

    #[test]
    fn from_json_merged_overlays_partial_over_defaults() {
        // Only two fields present + one unknown field — the rest must default,
        // the unknown must be ignored.
        let s = Settings::from_json_merged(
            r#"{ "silenceThreshold": -40, "format": "wav", "someFutureField": true }"#,
        );
        assert_eq!(s.silence_threshold, -40);
        assert_eq!(s.format, FileFormat::Wav);
        // Untouched fields kept their defaults.
        assert_eq!(s.silence_timeout_minutes, 5);
        assert_eq!(s.channels, ChannelMode::Stereo);
        assert!(s.wake_from_sleep);
    }

    #[test]
    fn round_trip_through_json_is_identical_after_validate() {
        let original = Settings {
            language: Some("en".to_string()),
            device_name: Some("Soundcraft USB".to_string()),
            channels: ChannelMode::MonoMix,
            sample_rate_mode: SampleRate::R44100,
            silence_threshold: -40,
            format: FileFormat::Flac,
            filename_pattern: FilenamePattern::Datetime,
            stop_on_silence: true,
            silence_timeout_minutes: 10,
            ..Default::default()
        }
        .validated();

        let json = serde_json::to_string(&original).unwrap();
        let mut back = Settings::from_json_merged(&json);
        back.validate();
        assert_eq!(back, original);
    }

    // ── R4 unification fields ────────────────────────────────────────────────

    #[test]
    fn r4_fields_default_and_serialise_camel_case() {
        let s = Settings::default();
        assert!(s.device_channels.is_empty());
        assert!(!s.preroll_enabled);

        let json = serde_json::to_value(&s).unwrap();
        let obj = json.as_object().unwrap();
        for key in ["deviceChannels", "prerollEnabled"] {
            assert!(obj.contains_key(key), "missing camelCase key {key}");
        }
    }

    #[test]
    fn preroll_enabled_is_stored_but_never_the_answer() {
        // The doc on the field used to name a reader (`preroll-lifecycle.ts`)
        // that no longer exists, which is how a dead field keeps looking alive.
        // The seconds ARE the switch now — in the shell (`app/state/preroll.ts`)
        // and on the wire — so the stored bool must not be able to change any
        // answer. Both directions, because "false wins" and "true wins" are two
        // different ways of re-wiring it by accident.
        let armed_but_flag_off = Settings {
            pre_roll_seconds: 15,
            preroll_enabled: false,
            ..Default::default()
        };
        let unarmed_but_flag_on = Settings {
            pre_roll_seconds: 0,
            preroll_enabled: true,
            ..Default::default()
        };
        assert!(
            crate::telemetry::WireSettings::from_settings(&armed_but_flag_off).preroll_enabled,
            "15 s with the legacy flag off is pre-roll ON — the seconds decide"
        );
        assert!(
            !crate::telemetry::WireSettings::from_settings(&unarmed_but_flag_on).preroll_enabled,
            "0 s with the legacy flag on is pre-roll OFF — the seconds decide"
        );

        // …and it still survives a save/load round-trip, which is the ONLY
        // reason the field is still here.
        let json = serde_json::to_string(&unarmed_but_flag_on).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(back.preroll_enabled, "a stored profile keeps its own value");
    }

    #[test]
    fn r4_fields_round_trip_through_json() {
        let mut dc = HashMap::new();
        dc.insert(
            "dev-1".to_string(),
            DeviceChannels {
                channel_l: 16,
                channel_r: 17,
            },
        );
        let original = Settings {
            device_id: Some("dev-1".to_string()),
            device_channels: dc,
            preroll_enabled: true,
            ..Default::default()
        }
        .validated();

        let json = serde_json::to_string(&original).unwrap();
        let back = Settings::from_json_merged(&json).validated();
        assert_eq!(back, original);
    }

    #[test]
    fn r4_malformed_field_costs_only_that_field() {
        // The lenient rule, per field: garbage in one R4 value must not reset
        // the rest of the blob (the from_json_merged full-defaults trapdoor).
        let s = Settings::from_json_merged(
            r#"{
                "silenceThreshold": -40,
                "deviceChannels": "not-a-map"
            }"#,
        )
        .validated();
        // The neighbour survived — the whole point.
        assert_eq!(s.silence_threshold, -40);
        // The malformed field landed on its (validated) default.
        assert!(s.device_channels.is_empty());
    }

    // Live streaming was removed in v0.14, but installed apps upgraded from
    // older versions still carry the stream fields in their sqlite settings
    // blob. The migration contract: those fields are DROPPED tolerantly — the
    // blob imports cleanly, every neighbour keeps its value, and nothing fails.
    #[test]
    fn legacy_blob_with_stream_fields_imports_cleanly() {
        let s = Settings::from_json_merged(
            r#"{
                "silenceThreshold": -40,
                "churchName": "Domkirken",
                "streamDestinations": [
                    {"id": "yt", "name": "YouTube",
                     "rtmpUrl": "rtmp://a.rtmp.youtube.com/live2",
                     "enabled": true, "hasKey": true}
                ],
                "streamResolution": "1080p",
                "streamFramerate": 25,
                "streamVideoBitrate": 4500,
                "streamOverlays": [{"id": "o1", "type": "image"}]
            }"#,
        )
        .validated();
        // The neighbours survived — dropping stream fields costs nothing else.
        assert_eq!(s.silence_threshold, -40);
        assert_eq!(s.church_name, "Domkirken");
        // And the round-trip writes a blob WITHOUT the retired fields.
        let json = serde_json::to_value(&s).unwrap();
        let obj = json.as_object().unwrap();
        for gone in [
            "streamDestinations",
            "streamResolution",
            "streamFramerate",
            "streamVideoBitrate",
            "streamOverlays",
        ] {
            assert!(
                !obj.contains_key(gone),
                "{gone} must not survive the round-trip"
            );
        }
    }

    // The sharing cluster (cloud backup, chat webhook, podcast RSS) was removed in R1 of
    // «Frivilligen først». Upgraded installs still carry its keys in the sqlite
    // blob, and an exported profile from an older build carries them too. Same
    // contract as the stream fields: DROPPED tolerantly, neighbours intact, and
    // the round-trip writes a blob without them.
    #[test]
    fn legacy_blob_with_removed_sharing_fields_imports_cleanly() {
        let s = Settings::from_json_merged(
            r#"{
                "silenceThreshold": -40,
                "emailAddress": "vakt@kirka.no",
                "webhookUrl": "https://hooks.slack.com/services/T/B/X",
                "webhookOnWarning": true,
                "webhookAllowLocal": true,
                "cloudGoogleDrive": {"enabled": true, "autoUpload": true,
                                     "folderId": "f1", "folderName": "Opptak"},
                "cloudDropbox": null,
                "cloudOneDrive": {"enabled": false},
                "podcast": {"enabled": true, "service": "google-drive",
                            "title": "Domkirken taler", "autoPrepEnabled": true,
                            "defaultMasterPreset": "speech-clear"}
            }"#,
        )
        .validated();
        assert_eq!(s.silence_threshold, -40);
        assert_eq!(s.email_address, "vakt@kirka.no");
        let json = serde_json::to_value(&s).unwrap();
        let obj = json.as_object().unwrap();
        for gone in [
            "webhookUrl",
            "webhookOnWarning",
            "webhookAllowLocal",
            "cloudGoogleDrive",
            "cloudDropbox",
            "cloudOneDrive",
            "podcast",
        ] {
            assert!(
                !obj.contains_key(gone),
                "{gone} must not survive the round-trip"
            );
        }
    }

    // v0.15 («Frivilligen først» R2) removed the dead settings fields — the
    // Electron capture-chain knobs nothing read, the legacy numeric sampleRate,
    // and the controls without consumers. Every upgraded install and every
    // exported profile still carries them. Same contract as the two tests
    // above: DROPPED tolerantly, neighbours intact (including the owner's
    // imported `separateAudioFormat: "flac"` — the value itself is gone, the
    // `format` beside it survives and is what the sidecar follows now), and
    // the round-trip writes a blob without them.
    #[test]
    fn legacy_blob_with_v015_dead_fields_imports_cleanly() {
        let s = Settings::from_json_merged(
            r#"{
                "hasLaunched": true,
                "sampleRate": 44100,
                "sampleRateMode": "r44100",
                "inputVolume": 150,
                "eqEnabled": true, "eqBass": 3, "eqMid": -2, "eqTreble": 1,
                "compEnabled": true, "compThreshold": -18.0, "compRatio": 3.0,
                "compAttack": 5.0, "compRelease": 100.0,
                "limiterEnabled": false, "limiterCeiling": -0.5,
                "avSync": false,
                "minimizeToTray": false,
                "videoBitrate": 8000,
                "outputMode": "separate",
                "trimSilence": true,
                "showLiveLevels": false,
                "separateAudioFormat": "flac",
                "format": "flac",
                "localAdaptivity": true,
                "videoResolution": "2160p",
                "videoFramerate": 60,
                "videoContainer": "mov",
                "videoCodec": "h265",
                "videoEncoder": "software",
                "editorHwEncode": true,
                "churchName": "Domkirken"
            }"#,
        )
        .validated();
        assert_eq!(s.sample_rate_mode, SampleRate::R44100);
        assert_eq!(s.format, FileFormat::Flac);
        assert_eq!(s.church_name, "Domkirken");
        let json = serde_json::to_value(&s).unwrap();
        let obj = json.as_object().unwrap();
        for gone in [
            "hasLaunched",
            "sampleRate",
            "inputVolume",
            "eqEnabled",
            "eqBass",
            "eqMid",
            "eqTreble",
            "compEnabled",
            "compThreshold",
            "compRatio",
            "compAttack",
            "compRelease",
            "limiterEnabled",
            "limiterCeiling",
            "avSync",
            "minimizeToTray",
            "videoBitrate",
            "outputMode",
            "trimSilence",
            "showLiveLevels",
            "separateAudioFormat",
            "localAdaptivity",
            "videoResolution",
            "videoFramerate",
            "videoContainer",
            "videoCodec",
            "videoEncoder",
            "editorHwEncode",
        ] {
            assert!(
                !obj.contains_key(gone),
                "{gone} must not survive the round-trip"
            );
        }
    }

    #[test]
    fn validate_derives_recorder_channels_from_the_device_map() {
        let mut dc = HashMap::new();
        dc.insert(
            "qu5".to_string(),
            DeviceChannels {
                channel_l: 99, // clamps to 31
                channel_r: -3, // clamps to 0
            },
        );
        let mut s = Settings {
            device_id: Some("qu5".to_string()),
            device_channels: dc,
            // Stale flat values that must be overwritten by the derivation.
            input_channel_l: Some(4),
            input_channel_r: Some(5),
            ..Default::default()
        };
        s.validate();
        assert_eq!(s.input_channel_l, Some(31));
        assert_eq!(s.input_channel_r, Some(0));
        // Idempotent: a second validate changes nothing.
        let once = s.clone();
        s.validate();
        assert_eq!(s, once);
    }

    #[test]
    fn validate_clears_recorder_channels_for_an_unmapped_selected_device() {
        // The bridge behaviour, preserved: switching to a device with no stored
        // mapping means DEFAULT routing — inheriting the previous device's
        // channels would record the wrong source (the 2026-07-31 Qu-5 class).
        let mut dc = HashMap::new();
        dc.insert(
            "other-device".to_string(),
            DeviceChannels {
                channel_l: 16,
                channel_r: 17,
            },
        );
        let mut s = Settings {
            device_id: Some("qu5".to_string()),
            device_channels: dc,
            input_channel_l: Some(16),
            input_channel_r: Some(17),
            ..Default::default()
        };
        s.validate();
        assert_eq!(s.input_channel_l, None);
        assert_eq!(s.input_channel_r, None);
    }

    #[test]
    fn validate_keeps_flat_channels_when_no_map_exists() {
        // Back-compat: an older exported profile carries only the flat fields.
        let mut s = Settings {
            device_id: Some("qu5".to_string()),
            input_channel_l: Some(2),
            input_channel_r: Some(3),
            ..Default::default()
        };
        s.validate();
        assert_eq!(s.input_channel_l, Some(2));
        assert_eq!(s.input_channel_r, Some(3));
    }

    // ── resolve_save_folder — the canonical rule ─────────────────────────────

    #[test]
    fn resolve_save_folder_configured_wins_verbatim() {
        let docs = std::path::Path::new("/Users/x/Documents");
        assert_eq!(
            resolve_save_folder(Some("/Volumes/Rig/Opptak"), Some(docs)).unwrap(),
            std::path::PathBuf::from("/Volumes/Rig/Opptak")
        );
    }

    #[test]
    fn resolve_save_folder_default_is_the_subfolder_never_bare_documents() {
        // The pre-R3 bug: three callers (trash sweep, recordings_prune, trash
        // commands) used the BARE Documents dir as the recordings root. The
        // canonical rule always appends the subfolder.
        let docs = std::path::Path::new("/Users/x/Documents");
        for unset in [None, Some(""), Some("   ")] {
            assert_eq!(
                resolve_save_folder(unset, Some(docs)).unwrap(),
                std::path::PathBuf::from("/Users/x/Documents/SundayRec")
            );
        }
    }

    #[test]
    fn resolve_save_folder_without_documents_errors_never_dot() {
        // diagnostics/mod.rs pre-R3 could yield a literal "." (the unwrap_or
        // sat OUTSIDE the join). The canonical rule refuses instead.
        for docs in [None, Some(std::path::Path::new(""))] {
            let err = resolve_save_folder(None, docs).unwrap_err();
            assert_eq!(err, NoSaveFolder);
            assert!(err.to_string().starts_with("no_save_folder"));
        }
        // A configured folder needs no Documents dir at all.
        assert_eq!(
            resolve_save_folder(Some("/Volumes/Rig/Opptak"), None).unwrap(),
            std::path::PathBuf::from("/Volumes/Rig/Opptak")
        );
    }
}
