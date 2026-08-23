/**
 * The ONE-SHOT localStorage → sqlite settings migration, pure half (R4).
 *
 * Until R4 the renderer's full settings lived in `localStorage["sundayrec.settings"]`
 * (the Electron heritage) while the backend read sqlite — bridged by a curated
 * `settings_save` subset that produced the #113/#115 family of silent
 * re-defaulting bugs. R4 makes sqlite the single store; this module translates
 * an existing install's blob into the unified (Rust-named) vocabulary exactly
 * once, so nothing the operator configured is lost in the switch.
 *
 * `mapLegacyBlob` is deliberately a FIELD-BY-FIELD whitelist rather than a
 * pass-through with renames, for the same reason the old bridge was: the
 * backend's `settings_import` merge (`Settings::from_json_merged`) falls back
 * to FULL defaults the moment any field fails to deserialise, and an
 * eleven-months-old blob is exactly where a float-in-an-i32 or a retired enum
 * tag lives. One bad value must cost that value, never the whole migration.
 *
 * Vocabulary translations (old TS name → unified name), applied HERE only —
 * no living compat code elsewhere:
 *   - `videoSeparate: boolean`     → `outputMode: "separate" | "combined"`
 *   - `videoKeepAudio` (abs.=true) → `keepSeparateAudio`
 *   - `format` (was overloaded)    → also seeds `separateAudioFormat`, because
 *     the bridge always sent `separateAudioFormat = format` — a migrated
 *     install keeps the sidecar-audio format it was actually getting.
 *
 * Dropped on the floor, with their real homes:
 *   - `recordingHistory` → the `recording` table (already authoritative)
 *   - `wakeFailureHistory` → the wake engine's own store
 *   - `reviewQueue` → dropped (the review queue was removed)
 *   - `activeRecovery` → the recovery dir
 *   - `nextExpectedRecordingISO` → dead (no Tauri reader; the scheduler owns
 *     missed-recording detection)
 *   - `useUnifiedRecorder` → dead (hard-coded true since the A/V-sync choice
 *     was removed)
 *   - `integrations` → the backend integrations store (`integrations_*`)
 *   - `emailSmtpPass`/`emailSmtpPassEnc`/`emailSmtpPassSet` → the OS keychain;
 *     secrets never enter the settings store (E1.6's purge, subsumed)
 *
 * The impure half (read localStorage, invoke `settings_import`, remove the
 * key, flag) lives in api-shim.ts — see `migrateLegacySettingsOnce`.
 */

/** The localStorage key the pre-R4 renderer persisted settings under. */
export const LEGACY_SETTINGS_KEY = "sundayrec.settings";

/** The once-per-install flag `migrateLegacySettingsOnce` sets when done. */
export const LEGACY_MIGRATED_FLAG = "sundayrec.settings.migratedToSqlite.v1";

type Dict = Record<string, unknown>;

/** `v` rounded and clamped to `[lo, hi]`, or `undefined` when not a finite
 *  number — an absent key lets the backend default win, which is the honest
 *  translation of a value we cannot read. */
function int(v: unknown, lo: number, hi: number): number | undefined {
  if (typeof v !== "number" || !Number.isFinite(v)) return undefined;
  return Math.min(hi, Math.max(lo, Math.round(v)));
}

function num(v: unknown, lo: number, hi: number): number | undefined {
  if (typeof v !== "number" || !Number.isFinite(v)) return undefined;
  return Math.min(hi, Math.max(lo, v));
}

function bool(v: unknown): boolean | undefined {
  return typeof v === "boolean" ? v : undefined;
}

function str(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

function strOrNull(v: unknown): string | null | undefined {
  if (v === null) return null;
  return typeof v === "string" ? v : undefined;
}

/** A member of `allowed`, or `undefined` (→ backend default). */
function oneOf<T extends string>(v: unknown, allowed: readonly T[]): T | undefined {
  return allowed.includes(v as T) ? (v as T) : undefined;
}

const FORMATS = ["mp3", "wav", "flac", "aac"] as const;

function sanitizeSlots(v: unknown): Dict[] {
  if (!Array.isArray(v)) return [];
  return v
    .filter((sl) => sl && Array.isArray((sl as { days?: unknown }).days))
    .map((sl) => {
      const o = sl as Dict;
      return {
        days: (o.days as unknown[]).filter((d) => Number.isInteger(d)),
        start: typeof o.start === "string" ? o.start : "10:00",
        stop: typeof o.stop === "string" ? o.stop : "12:00",
        max: typeof o.max === "number" ? Math.round(o.max) : null,
      };
    });
}

function sanitizeSpecials(v: unknown): Dict[] {
  if (!Array.isArray(v)) return [];
  return v
    .filter((r) => r && typeof (r as { date?: unknown }).date === "string")
    .map((r) => {
      const o = r as Dict;
      return {
        id: typeof o.id === "string" ? o.id : null,
        date: o.date as string,
        name: typeof o.name === "string" ? o.name : "",
        start: typeof o.start === "string" ? o.start : "10:00",
        stop: typeof o.stop === "string" ? o.stop : "12:00",
      };
    });
}

function sanitizeDeviceChannels(v: unknown): Dict | undefined {
  if (!v || typeof v !== "object" || Array.isArray(v)) return undefined;
  const out: Dict = {};
  for (const [id, pair] of Object.entries(v as Dict)) {
    const p = (pair ?? {}) as { channelL?: unknown; channelR?: unknown };
    const l = int(p.channelL, 0, 31);
    const r = int(p.channelR, 0, 31);
    if (l !== undefined && r !== undefined) out[id] = { channelL: l, channelR: r };
  }
  return out;
}

function sanitizePodcast(v: unknown): Dict | undefined {
  if (!v || typeof v !== "object" || Array.isArray(v)) return undefined;
  const o = v as Dict;
  return {
    enabled: o.enabled === true,
    service: oneOf(o.service, ["google-drive", "dropbox", "onedrive"]) ?? "google-drive",
    title: str(o.title) ?? "",
    description: str(o.description) ?? "",
    author: str(o.author) ?? "",
    language: str(o.language) ?? "no",
    category: str(o.category) ?? "Religion & Spirituality",
    explicit: o.explicit === true,
    link: str(o.link) ?? null,
    imageUrl: str(o.imageUrl) ?? null,
    email: str(o.email) ?? null,
    feedUrl: str(o.feedUrl) ?? null,
    autoPrepEnabled: o.autoPrepEnabled !== false,
    defaultIntroPath: str(o.defaultIntroPath) ?? null,
    defaultOutroPath: str(o.defaultOutroPath) ?? null,
    defaultMasterPreset: str(o.defaultMasterPreset) ?? "speech-clear",
  };
}

/**
 * Translate a raw pre-R4 localStorage blob into unified-vocabulary settings
 * JSON, ready for `settings_import` (whose merge-over-defaults + `validate`
 * fills everything this whitelist leaves out).
 *
 * Returns `null` when the blob is not a JSON object at all — nothing worth
 * importing, the store starts from defaults (the caller still removes the
 * localStorage key so the corrupt blob is not re-tried forever).
 */
export function mapLegacyBlob(raw: string): Dict | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  const s = parsed as Dict;

  const out: Dict = {};
  const put = (key: string, value: unknown): void => {
    if (value !== undefined) out[key] = value;
  };

  // System
  put("language", strOrNull(s.language));
  put("hasLaunched", bool(s.hasLaunched));
  put("onboardingDone", bool(s.onboardingDone));

  // Audio device + per-device channel map
  put("deviceId", strOrNull(s.deviceId));
  put("deviceName", strOrNull(s.deviceName));
  put("deviceChannels", sanitizeDeviceChannels(s.deviceChannels));

  // Video device
  put("videoEnabled", bool(s.videoEnabled));
  put("videoDeviceName", strOrNull(s.videoDeviceName));
  put("videoDeviceIndex", s.videoDeviceIndex === null ? null : int(s.videoDeviceIndex, -1, 999));
  put("videoResolution", oneOf(s.videoResolution, ["480p", "720p", "1080p", "2160p"]));
  put("videoFramerate", int(s.videoFramerate, 1, 120));
  put("videoContainer", oneOf(s.videoContainer, ["mp4", "mov"]));
  put("videoCodec", oneOf(s.videoCodec, ["h264", "h265"]));
  put("videoEncoder", oneOf(s.videoEncoder, ["software", "hardware"]));
  put("videoFlip", bool(s.videoFlip));
  put("videoBitrate", int(s.videoBitrate, 0, 50_000));
  // Renames: the boolean pair → the Rust vocabulary. Absent means default.
  if (typeof s.videoSeparate === "boolean") {
    put("outputMode", s.videoSeparate ? "separate" : "combined");
  }
  if (s.videoKeepAudio !== undefined) {
    put("keepSeparateAudio", s.videoKeepAudio !== false);
  }
  put("classicDirectshow", bool(s.classicDirectshow));
  put("classicFfmpegAudio", bool(s.classicFfmpegAudio));
  put("classicFfmpegPreroll", bool(s.classicFfmpegPreroll));

  // Audio processing
  put("channels", oneOf(s.channels, ["stereo", "monoL", "monoR", "monoMix"]));
  put("sampleRate", int(s.sampleRate, 8_000, 192_000));
  put("sampleRateMode", oneOf(s.sampleRateMode, ["auto", "r44100", "r48000", "r96000"]));
  put("inputVolume", int(s.inputVolume, 0, 200));
  put("eqBass", int(s.eqBass, -24, 24));
  put("eqMid", int(s.eqMid, -24, 24));
  put("eqTreble", int(s.eqTreble, -24, 24));
  put("compEnabled", bool(s.compEnabled));
  put("compThreshold", num(s.compThreshold, -60, 0));
  put("compRatio", num(s.compRatio, 1, 100));
  put("compAttack", num(s.compAttack, 0.1, 2000));
  put("compRelease", num(s.compRelease, 1, 9000));
  put("limiterEnabled", bool(s.limiterEnabled));
  put("limiterCeiling", num(s.limiterCeiling, -10, 0));

  // Output. `format` seeds BOTH fields — see the module header.
  const format = oneOf(s.format, FORMATS);
  put("format", format);
  put("separateAudioFormat", format);
  put("bitrate", s.bitrate === undefined ? undefined : String(s.bitrate));
  put("filenamePattern", oneOf(s.filenamePattern, ["date", "church", "plain", "datetime"]));
  put("saveFolder", strOrNull(s.saveFolder));
  put("autoDeleteDays", int(s.autoDeleteDays, 0, 3650));

  // Schedule + recording behaviour
  put("slots", Array.isArray(s.slots) ? sanitizeSlots(s.slots) : undefined);
  put(
    "specialRecordings",
    Array.isArray(s.specialRecordings) ? sanitizeSpecials(s.specialRecordings) : undefined,
  );
  put("stopOnSilence", bool(s.stopOnSilence));
  put("silenceThreshold", int(s.silenceThreshold, -90, 0));
  put("silenceTimeoutMinutes", int(s.silenceTimeoutMinutes, 1, 120));
  put("splitMinutes", int(s.splitMinutes, 0, 480));
  put("trimSilence", bool(s.trimSilence));
  put("manualMaxMinutes", int(s.manualMaxMinutes, 0, 1440));
  put("preRollSeconds", int(s.preRollSeconds, 0, 60));
  put("prerollEnabled", bool(s.prerollEnabled));
  put("showLiveLevels", bool(s.showLiveLevels));
  put("reminderMinutes", int(s.reminderMinutes, 0, 60));

  // System behaviour
  put("launchAtLogin", bool(s.launchAtLogin));
  put("minimizeToTray", bool(s.minimizeToTray));
  put("wakeFromSleep", bool(s.wakeFromSleep));
  put("protectRecording", bool(s.protectRecording));

  // Church profile + notifications
  put("churchName", str(s.churchName));
  put("responsiblePerson", str(s.responsiblePerson));
  put("notifyStart", bool(s.notifyStart));
  put("notifyStop", bool(s.notifyStop));
  // (webhookUrl / webhookOnWarn / webhookAllowLocal: the chat webhook was
  // removed with the sharing cluster — the whitelist mapper never copies
  // them, so an old blob imports cleanly without them.)

  // Email (never the password — keychain only).
  put("emailOnError", bool(s.emailOnError));
  put("emailAddress", str(s.emailAddress));
  put("emailSmtp", str(s.emailSmtp));
  put("emailSmtpPort", int(s.emailSmtpPort, 1, 65_535));
  put("emailSmtpUser", str(s.emailSmtpUser));
  put("emailSmtpFrom", str(s.emailSmtpFrom));

  // Editor
  put("askOpenEditor", bool(s.askOpenEditor));
  put("editorIntroPath", strOrNull(s.editorIntroPath));
  put("editorOutroPath", strOrNull(s.editorOutroPath));
  put("editorHwEncode", bool(s.editorHwEncode));

  // Live streaming (removed in v0.14) and cloud backup (removed with the
  // sharing cluster): old blobs may still carry streamDestinations/
  // streamResolution/streamFramerate/streamVideoBitrate/streamOverlays and
  // cloudGoogleDrive/cloudDropbox/cloudOneDrive — the whitelist mapper simply
  // never copies them, so a legacy config imports cleanly without them.

  // Podcast
  put("podcast", sanitizePodcast(s.podcast));

  // Misc
  put("autoUpdate", bool(s.autoUpdate));
  put("updateChannel", oneOf(s.updateChannel, ["stable", "beta"]));
  put("localAdaptivity", bool(s.localAdaptivity));

  return out;
}
