// The one-shot migration's pure half, held to its two promises:
//
//   1. NOTHING an operator configured is lost in the vocabulary switch — every
//      old key with a reader lands under its unified name, value intact.
//   2. One unreadable value costs that value, never the migration: the mapper
//      whitelists field-by-field precisely because `Settings::from_json_merged`
//      backend-side falls to FULL defaults on any type error.
//
// The realistic-blob fixture below is shaped like a real pre-R4 install's
// localStorage (the old DEFAULT_SETTINGS surface plus the shadow fields),
// including the four renamed keys — breaking any name mapping in the mapper
// turns at least one assertion here red (the R4 gate's mutation test).

import { describe, expect, it } from "vitest";
import {
  LEGACY_MIGRATED_FLAG,
  LEGACY_SETTINGS_KEY,
  mapLegacyBlob,
} from "./migrate-legacy-settings-core";

/** A realistic pre-R4 blob: old names, shadow fields, one float, one stale tag. */
const REALISTIC_BLOB = {
  language: "no",
  hasLaunched: true,
  onboardingDone: true,
  deviceId: "qu5-usb",
  deviceName: "Allen & Heath Qu-5",
  deviceChannels: { "qu5-usb": { channelL: 16, channelR: 17 } },
  channels: "stereo",
  sampleRate: 48000,
  sampleRateMode: "auto",
  inputVolume: 80.5, // float (the mapper used to round it; dead since v0.15)
  eqBass: 0,
  eqMid: 0,
  eqTreble: 0,
  compEnabled: false,
  compThreshold: -24,
  compRatio: 4,
  compAttack: 10,
  compRelease: 200,
  limiterEnabled: true,
  limiterCeiling: -1,
  format: "flac",
  bitrate: "256",
  filenamePattern: "church",
  saveFolder: "/Volumes/Rig/Opptak",
  autoDeleteDays: 90,
  slots: [{ days: [6], start: "10:30", stop: "12:30", max: 150 }],
  specialRecordings: [
    {
      id: "s1",
      date: "2099-12-24",
      name: "Julaften",
      start: "16:00",
      stop: "17:00",
    },
  ],
  stopOnSilence: true,
  silenceThreshold: -50,
  silenceTimeoutMinutes: 5,
  splitMinutes: 0,
  reminderMinutes: 15.4, // float — serde would reject an i32; the mapper rounds
  manualMaxMinutes: 0,
  preRollSeconds: 30,
  prerollEnabled: true,
  launchAtLogin: true,
  minimizeToTray: true,
  wakeFromSleep: false,
  protectRecording: true,
  notifyStart: false,
  notifyStop: true,
  emailOnError: true,
  emailAddress: "lydansvarlig@kirke.no",
  emailSmtp: "smtp.kirke.no",
  emailSmtpPort: 587,
  emailSmtpUser: "opptak@kirke.no",
  emailSmtpFrom: "",
  emailSmtpPass: "SUPERSECRET", // must never cross
  autoUpdate: false,
  updateChannel: "beta",
  askOpenEditor: true,
  editorIntroPath: "/jingler/intro.mp3",
  editorOutroPath: null,
  editorHwEncode: true,
  churchName: "Domkirken",
  responsiblePerson: "Kari Nordmann",
  webhookUrl: "https://hooks.example.no/x", // webhook removed — dropped
  webhookOnWarn: true, // dropped
  webhookAllowLocal: false, // dropped
  videoEnabled: true,
  videoDeviceName: "FaceTime HD",
  videoDeviceIndex: 0,
  videoResolution: "1080p",
  videoBitrate: 8000,
  videoFramerate: 30,
  videoContainer: "mp4",
  videoCodec: "h264",
  videoEncoder: "hardware",
  videoSeparate: true, // → outputMode "separate"
  videoKeepAudio: false, // → keepSeparateAudio false
  videoFlip: false,
  useUnifiedRecorder: true, // dead — dropped
  localAdaptivity: true,
  trimSilence: false,
  showLiveLevels: true,
  streamDestinations: [
    {
      id: "yt",
      name: "YouTube",
      rtmpUrl: "rtmp://a/live2",
      enabled: true,
      hasKey: true,
    },
  ],
  streamResolution: "1080p",
  streamFramerate: 25,
  streamVideoBitrate: 4500,
  streamOverlays: [
    { id: "o1", type: "image", source: "/logo.png", position: "br" },
  ],
  cloudGoogleDrive: { enabled: true, autoUpload: false, folderName: "Opptak" }, // cloud removed — dropped
  podcast: {
    // podcast removed — dropped
    enabled: true,
    service: "google-drive",
    title: "Domkirken taler",
    description: "Ukens preken",
    author: "Domkirken",
    language: "no",
    category: "Religion & Spirituality",
    explicit: false,
    email: "post@kirke.no",
    feedUrl: "https://drive.example/feed.xml",
  },
  integrations: { enabled: true }, // own backend store — dropped
  activeRecovery: null, // recovery dir — dropped
  nextExpectedRecordingISO: "2026-08-16T10:30:00", // dead — dropped
  recordingHistory: [{ path: "/rec/a.mp3", status: "ok" }], // recordings table
  wakeFailureHistory: [{ timestamp: 1 }], // wake store
  reviewQueue: [{ id: "q1" }], // review-queue store
} as const;

describe("mapLegacyBlob", () => {
  it("translates a realistic blob: renames applied, values intact", () => {
    const out = mapLegacyBlob(JSON.stringify(REALISTIC_BLOB))!;
    expect(out).not.toBeNull();

    // The rename — the mutation-test surface. (`videoSeparate` → `outputMode`
    // and `format` → `separateAudioFormat` were renames until v0.15; both
    // targets are dead fields now and must NOT be produced.)
    expect(out.keepSeparateAudio).toBe(false);
    expect(out).not.toHaveProperty("videoKeepAudio");
    expect(out).not.toHaveProperty("videoSeparate");
    expect(out).not.toHaveProperty("outputMode");
    expect(out).not.toHaveProperty("separateAudioFormat");
    expect(out.format).toBe("flac"); // …and the sidecar follows THIS in the backend

    // Values with backend readers survive verbatim.
    expect(out.updateChannel).toBe("beta");
    expect(out.autoDeleteDays).toBe(90);
    expect(out.autoUpdate).toBe(false);
    expect(out.saveFolder).toBe("/Volumes/Rig/Opptak");
    expect(out.filenamePattern).toBe("church");
    expect(out.churchName).toBe("Domkirken");
    expect(out.wakeFromSleep).toBe(false);
    expect(out.notifyStart).toBe(false);
    expect(out.prerollEnabled).toBe(true);
    expect(out.preRollSeconds).toBe(30);
    expect(out.deviceChannels).toEqual({
      "qu5-usb": { channelL: 16, channelR: 17 },
    });
    expect(out.slots).toEqual([
      { days: [6], start: "10:30", stop: "12:30", max: 150 },
    ]);

    // Floats are rounded, not forwarded (a raw 15.4 fails the WHOLE Rust merge).
    expect(out.reminderMinutes).toBe(15);
  });

  // v0.15 («Frivilligen først» R2) removed the dead settings fields. An old
  // blob carries every one of them; the mapper must copy NONE — a key the Rust
  // struct no longer has would be dropped by serde anyway, but the whitelist
  // is the contract, and the neighbours must cross intact.
  it("drops the v0.15 dead settings fields tolerantly — the rest imports cleanly", () => {
    const out = mapLegacyBlob(
      JSON.stringify({
        ...REALISTIC_BLOB,
        avSync: false,
        minimizeToTray: false,
        videoBitrate: 8000,
        trimSilence: true,
        showLiveLevels: false,
        separateAudioFormat: "wav",
        localAdaptivity: true,
      }),
    )!;
    for (const gone of [
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
      "videoSeparate",
      "trimSilence",
      "showLiveLevels",
      "separateAudioFormat",
      "localAdaptivity",
    ]) {
      expect(out, `${gone} must not migrate`).not.toHaveProperty(gone);
    }
    expect(out.format).toBe("flac");
    expect(out.sampleRateMode).toBe("auto");
    expect(out.churchName).toBe("Domkirken");
    expect(out.keepSeparateAudio).toBe(false);
  });

  it("never lets a secret cross, and drops the shadow fields", () => {
    const out = mapLegacyBlob(JSON.stringify(REALISTIC_BLOB))!;
    expect(JSON.stringify(out)).not.toContain("SUPERSECRET");
    for (const gone of [
      "emailSmtpPass",
      "emailSmtpPassEnc",
      "emailSmtpPassSet",
      "recordingHistory",
      "wakeFailureHistory",
      "reviewQueue",
      "activeRecovery",
      "nextExpectedRecordingISO",
      "useUnifiedRecorder",
      "integrations",
    ]) {
      expect(out, `${gone} must not migrate`).not.toHaveProperty(gone);
    }
  });

  // Live streaming was removed in v0.14, cloud backup + the chat webhook with
  // the sharing cluster after it. Old blobs still carry their fields — they
  // must be DROPPED tolerantly (imports cleanly without them), never fail the
  // migration or leak into the unified store where the Rust merge would
  // choke on unknown keys' shapes.
  it("drops the retired stream/cloud/webhook/podcast fields tolerantly — the rest imports cleanly", () => {
    const out = mapLegacyBlob(JSON.stringify(REALISTIC_BLOB))!;
    expect(out).not.toBeNull();
    for (const gone of [
      "streamDestinations",
      "streamResolution",
      "streamFramerate",
      "streamVideoBitrate",
      "streamOverlays",
      "cloudGoogleDrive",
      "podcast",
      "webhookUrl",
      "webhookOnWarn",
      "webhookOnWarning",
      "webhookAllowLocal",
    ]) {
      expect(out, `${gone} must not migrate`).not.toHaveProperty(gone);
    }
    // The neighbours still cross intact — dropping stream fields costs nothing else.
    expect(out.churchName).toBe("Domkirken");
    expect(out.videoEnabled).toBe(true);
  });

  it("a partial blob maps only what it has (merge-over-defaults is Rust's job)", () => {
    const out = mapLegacyBlob(
      JSON.stringify({ churchName: "Betel", autoDeleteDays: 30 }),
    )!;
    expect(out).toEqual({ churchName: "Betel", autoDeleteDays: 30 });
  });

  it("one unreadable value costs that value, not the migration", () => {
    const out = mapLegacyBlob(
      JSON.stringify({
        churchName: "Betel",
        filenamePattern: "yyyy-mm-dd", // retired/unknown tag
        autoDeleteDays: "ninety", // wrong type
        slots: "not-an-array",
        deviceChannels: { bad: { channelL: "left" } },
        updateChannel: "canary",
      }),
    )!;
    expect(out.churchName).toBe("Betel");
    expect(out).not.toHaveProperty("filenamePattern");
    expect(out).not.toHaveProperty("autoDeleteDays");
    // Wrong-typed slots → absent → the backend default (empty) wins.
    expect(out).not.toHaveProperty("slots");
    expect(out.deviceChannels).toEqual({});
    expect(out).not.toHaveProperty("updateChannel");
  });

  it("corrupt or non-object JSON yields null — defaults, no crash", () => {
    expect(mapLegacyBlob("{ not json ]]]")).toBeNull();
    expect(mapLegacyBlob("42")).toBeNull();
    expect(mapLegacyBlob("[1,2]")).toBeNull();
    expect(mapLegacyBlob("null")).toBeNull();
    expect(mapLegacyBlob("")).toBeNull();
  });

  it("exports the exact key/flag names the impure half and the pin test share", () => {
    expect(LEGACY_SETTINGS_KEY).toBe("sundayrec.settings");
    expect(LEGACY_MIGRATED_FLAG).toBe("sundayrec.settings.migratedToSqlite.v1");
  });
});
