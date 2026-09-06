import { describe, it, expect } from "vitest";
import {
  buildExportRequest,
  exportLevelSummary,
  jinglesSupportedForFile,
  EXPORT_PHASES,
  EXPORT_PHASE_ENCODING,
  EXPORT_PHASE_MEASURING,
  type ExportRequestInput,
} from "./export-params";

const base: ExportRequestInput = {
  kind: "audio",
  inputPath: "/Users/x/Opptak/gudstjeneste.mp4",
  cutRegions: [{ start: 10, end: 20 }],
  duration: 3600,
  outputFolder: "",
  format: "mp3",
  bitrate: 256,
  bitDepth: 16,
  metadata: { title: "Søndag" },
};

describe("buildExportRequest", () => {
  it("passes the empty default destination through as an empty string", () => {
    // "Samme mappe" is the default pill and never picks a folder. '' is a real
    // instruction to the backend ("next to the source"), not a missing value —
    // sending undefined would leave the decision to the shim's fallback.
    const params = buildExportRequest(base);
    expect(params.outputFolder).toBe("");
    expect("outputFolder" in params).toBe(true);
  });

  it("passes a picked folder through untouched", () => {
    const params = buildExportRequest({
      ...base,
      outputFolder: "/Users/x/Skrivebord",
    });
    expect(params.outputFolder).toBe("/Users/x/Skrivebord");
  });

  it("never sends a mode field", () => {
    // 'new' | 'replace' | 'folder' was dropped by the shim and unimplemented in
    // Rust, so "Erstatt original" quietly wrote a new file instead. Pill gone,
    // field gone.
    expect(Object.keys(buildExportRequest(base))).not.toContain("mode");
    expect(
      Object.keys(buildExportRequest({ ...base, kind: "video" })),
    ).not.toContain("mode");
  });

  it("maps the audio format and bitrate onto the backend field names", () => {
    const params = buildExportRequest({
      ...base,
      format: "aac",
      bitrate: 192,
      bitDepth: 24,
    });
    expect(params.outputFormat).toBe("aac");
    expect(params.outputBitrate).toBe(192);
    expect(params.outputBitDepth).toBe(24);
  });

  it("defaults the audio format to mp3 when none is given", () => {
    const { format: _dropped, ...noFormat } = base;
    expect(buildExportRequest(noFormat).outputFormat).toBe("mp3");
  });

  it("sends the video container and codec instead of audio fields", () => {
    const params = buildExportRequest({
      ...base,
      kind: "video",
      videoFormat: "mov",
      videoCodec: "h265",
    });
    expect(params.videoFormat).toBe("mov");
    expect(params.videoCodec).toBe("h265");
    expect(params.outputFormat).toBeUndefined();
    expect(params.outputBitrate).toBeUndefined();
  });

  it("defaults the video container and codec", () => {
    const params = buildExportRequest({ ...base, kind: "video" });
    expect(params.videoFormat).toBe("mp4");
    expect(params.videoCodec).toBe("h264");
  });

  it("passes a normalize gain through and omits a zero one", () => {
    expect(buildExportRequest({ ...base, gainDb: -3.5 }).gainDb).toBe(-3.5);
    expect(buildExportRequest({ ...base, gainDb: 0 }).gainDb).toBeUndefined();
    expect(buildExportRequest(base).gainDb).toBeUndefined();
  });

  it("sends the normalize gain on the VIDEO path too", () => {
    // The shim's video branch hard-coded `gainDb: null`, so "Normaliser" was a
    // silent no-op for video exports while working for audio ones.
    expect(
      buildExportRequest({ ...base, kind: "video", gainDb: -3.5 }).gainDb,
    ).toBe(-3.5);
  });

  it('omits empty optional strings so the backend sees null, not ""', () => {
    const params = buildExportRequest({
      ...base,
      masterPreset: "",
      vocalChainPreset: "",
      introPath: "",
      outroPath: "",
    });
    expect(params.masterPreset).toBeUndefined();
    expect(params.vocalChainPreset).toBeUndefined();
    expect(params.introPath).toBeUndefined();
    expect(params.outroPath).toBeUndefined();
  });

  it("carries the enhancement settings and jingles when set", () => {
    const channelRepair = { mode: "duplicateLeft", leftDb: 0, rightDb: 0 };
    const processing = { highpass: { enabled: true } };
    const params = buildExportRequest({
      ...base,
      masterPreset: "speech-clear",
      vocalChainPreset: "voice-podcast",
      channelRepair,
      processing,
      introPath: "/Users/x/intro.mp3",
      outroPath: "/Users/x/outro.mp3",
    });
    expect(params.masterPreset).toBe("speech-clear");
    expect(params.vocalChainPreset).toBe("voice-podcast");
    expect(params.channelRepair).toEqual(channelRepair);
    expect(params.processing).toEqual(processing);
    expect(params.introPath).toBe("/Users/x/intro.mp3");
    expect(params.outroPath).toBe("/Users/x/outro.mp3");
  });

  it("passes the cut plan and duration through unchanged", () => {
    const params = buildExportRequest(base);
    expect(params.inputPath).toBe(base.inputPath);
    expect(params.cutRegions).toEqual([{ start: 10, end: 20 }]);
    expect(params.duration).toBe(3600);
    expect(params.metadata).toEqual({ title: "Søndag" });
  });
});

describe("jinglesSupportedForFile", () => {
  it("says no on sight for the unambiguous video extensions", () => {
    expect(jinglesSupportedForFile("/Users/x/Opptak/gudstjeneste.mp4")).toBe(
      false,
    );
    expect(jinglesSupportedForFile("/Users/x/Opptak/gudstjeneste.MOV")).toBe(
      false,
    );
    expect(jinglesSupportedForFile("/Users/x/gudstjeneste.m2ts")).toBe(false);
  });

  it("says yes for audio files", () => {
    expect(jinglesSupportedForFile("/Users/x/Opptak/gudstjeneste.mp3")).toBe(
      true,
    );
    expect(jinglesSupportedForFile("/Users/x/Opptak/gudstjeneste.flac")).toBe(
      true,
    );
  });

  it("honours the resolved probe for the ambiguous containers", () => {
    // .mkv carries video but is NOT in VIDEO_EXTS — the loader decides it by
    // probing the streams, and the seam still strips jingles from an mkv export.
    expect(jinglesSupportedForFile("C:\\Opptak\\gudstjeneste.mkv")).toBe(true);
    expect(jinglesSupportedForFile("C:\\Opptak\\gudstjeneste.mkv", true)).toBe(
      false,
    );
    // An audio-only .mka probes as audio and keeps its jingles.
    expect(jinglesSupportedForFile("/Users/x/gudstjeneste.mka", false)).toBe(
      true,
    );
  });

  it("does not read a dotted FOLDER name as the extension", () => {
    // '/Users/x/Sunday v1.0/gudstjeneste' has a dot in the directory only.
    expect(jinglesSupportedForFile("/Users/x/Sunday v1.0/gudstjeneste")).toBe(
      true,
    );
    expect(jinglesSupportedForFile("/Users/x/Sunday v1.0/opptak.mp4")).toBe(
      false,
    );
  });
});

describe("exportLevelSummary", () => {
  it("says mastering owns the level whenever a preset is active", () => {
    // The backend SKIPS the normalize `volume=` filter under a mastering preset
    // (loudnorm sets the delivery level), so the modal must not promise a gain
    // the export never applies — even when one was computed.
    expect(exportLevelSummary("speech-clear", 4.2)).toEqual({
      kind: "masterOwnsLevel",
    });
    expect(exportLevelSummary("music-speech", 0)).toEqual({
      kind: "masterOwnsLevel",
    });
  });

  it("reports the normalize gain when no preset is active", () => {
    expect(exportLevelSummary("", 4.2)).toEqual({
      kind: "normalized",
      gainDb: 4.2,
    });
    expect(exportLevelSummary(undefined, -1.5)).toEqual({
      kind: "normalized",
      gainDb: -1.5,
    });
  });

  it("hides the row when there is nothing to say", () => {
    expect(exportLevelSummary("", 0)).toEqual({ kind: "hidden" });
    expect(exportLevelSummary(undefined, 0)).toEqual({ kind: "hidden" });
    // A NaN gain (a failed peak probe) is "nothing to say", not "+NaN dB".
    expect(exportLevelSummary("", Number.NaN)).toEqual({ kind: "hidden" });
  });
});

describe("export progress phases", () => {
  it("are the exact wire codes the Rust seam emits", () => {
    // These strings cross the IPC boundary untyped. The Rust side pins the same
    // two values in `export_phase_codes_match_the_renderer_literals`
    // (src-tauri/src/editor/mod.rs), so renaming either side alone fails a test
    // instead of silently mislabelling the mastering measure pass as "encoding".
    expect(EXPORT_PHASE_MEASURING).toBe("measuring");
    expect(EXPORT_PHASE_ENCODING).toBe("encoding");
    expect(EXPORT_PHASES).toEqual(["measuring", "encoding"]);
  });
});
