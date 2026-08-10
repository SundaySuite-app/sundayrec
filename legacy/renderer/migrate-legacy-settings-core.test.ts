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
  inputVolume: 80.5, // float — serde would reject an i32; the mapper rounds
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
    { id: "s1", date: "2099-12-24", name: "Julaften", start: "16:00", stop: "17:00" },
  ],
  stopOnSilence: true,
  silenceThreshold: -50,
  silenceTimeoutMinutes: 5,
  splitMinutes: 0,
  reminderMinutes: 15,
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
  webhookUrl: "https://hooks.example.no/x",
  webhookOnWarn: true, // → webhookOnWarning
  webhookAllowLocal: false,
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
    { id: "yt", name: "YouTube", rtmpUrl: "rtmp://a/live2", enabled: true, hasKey: true },
  ],
  streamResolution: "1080p",
  streamFramerate: 25,
  streamVideoBitrate: 4500,
  streamOverlays: [{ id: "o1", type: "image", source: "/logo.png", position: "br" }],
  cloudGoogleDrive: { enabled: true, autoUpload: false, folderName: "Opptak" },
  podcast: {
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

    // The four renames — the mutation-test surface.
    expect(out.webhookOnWarning).toBe(true);
    expect(out).not.toHaveProperty("webhookOnWarn");
    expect(out.outputMode).toBe("separate");
    expect(out).not.toHaveProperty("videoSeparate");
    expect(out.keepSeparateAudio).toBe(false);
    expect(out).not.toHaveProperty("videoKeepAudio");
    expect(out.separateAudioFormat).toBe("flac"); // seeded from the overloaded `format`
    expect(out.format).toBe("flac");

    // Values with backend readers survive verbatim.
    expect(out.updateChannel).toBe("beta");
    expect(out.autoDeleteDays).toBe(90);
    expect(out.autoUpdate).toBe(false);
    expect(out.reminderMinutes).toBe(15);
    expect(out.localAdaptivity).toBe(true);
    expect(out.saveFolder).toBe("/Volumes/Rig/Opptak");
    expect(out.filenamePattern).toBe("church");
    expect(out.churchName).toBe("Domkirken");
    expect(out.wakeFromSleep).toBe(false);
    expect(out.notifyStart).toBe(false);
    expect(out.prerollEnabled).toBe(true);
    expect(out.preRollSeconds).toBe(30);
    expect(out.deviceChannels).toEqual({ "qu5-usb": { channelL: 16, channelR: 17 } });
    expect(out.slots).toEqual([{ days: [6], start: "10:30", stop: "12:30", max: 150 }]);
    expect(out.cloudGoogleDrive).toMatchObject({ enabled: true, autoUpload: false });
    expect(out.podcast).toMatchObject({
      enabled: true,
      title: "Domkirken taler",
      email: "post@kirke.no",
      feedUrl: "https://drive.example/feed.xml",
    });

    // Floats are rounded, not forwarded (a raw 80.5 fails the WHOLE Rust merge).
    expect(out.inputVolume).toBe(81);
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

  // Live streaming was removed in v0.14. Old blobs still carry its fields —
  // they must be DROPPED tolerantly (imports cleanly without them), never fail
  // the migration or leak into the unified store where the Rust merge would
  // choke on unknown keys' shapes.
  it("drops the retired stream fields tolerantly — the rest imports cleanly", () => {
    const out = mapLegacyBlob(JSON.stringify(REALISTIC_BLOB))!;
    expect(out).not.toBeNull();
    for (const gone of [
      "streamDestinations",
      "streamResolution",
      "streamFramerate",
      "streamVideoBitrate",
      "streamOverlays",
    ]) {
      expect(out, `${gone} must not migrate`).not.toHaveProperty(gone);
    }
    // The neighbours still cross intact — dropping stream fields costs nothing else.
    expect(out.churchName).toBe("Domkirken");
    expect(out.videoEnabled).toBe(true);
  });

  it("a partial blob maps only what it has (merge-over-defaults is Rust's job)", () => {
    const out = mapLegacyBlob(JSON.stringify({ churchName: "Betel", autoDeleteDays: 30 }))!;
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
