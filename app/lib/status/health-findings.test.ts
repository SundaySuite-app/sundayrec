import { describe, expect, it } from "vitest";

import { buildHealthFindings, isBlocked } from "./health-findings";

const t = (_key: string, fallback: string): string => fallback;

describe("isBlocked", () => {
  it("is true only for the two states the OS is actively refusing", () => {
    expect(isBlocked("denied")).toBe(true);
    expect(isBlocked("restricted")).toBe(true);
  });

  it('does not treat "never asked" as a problem — opening the device IS the prompt', () => {
    expect(isBlocked("notDetermined")).toBe(false);
  });

  it("does not report what it could not determine", () => {
    expect(isBlocked("unknown")).toBe(false);
    expect(isBlocked("authorized")).toBe(false);
    expect(isBlocked(undefined)).toBe(false);
  });
});

describe("buildHealthFindings", () => {
  it("says nothing when everything is fine", () => {
    expect(
      buildHealthFindings({
        permissions: { microphone: "authorized", camera: "authorized" },
        ffmpeg: { available: true, path: "/x/ffmpeg" },
        videoEnabled: true,
        t,
      }),
    ).toEqual([]);
  });

  it("reports a denied microphone as an error with a way to fix it", () => {
    const [f, ...rest] = buildHealthFindings({
      permissions: { microphone: "denied" },
      t,
    });
    expect(rest).toEqual([]);
    expect(f.severity).toBe("error");
    expect(f.category).toBe("device");
    expect(f.message).toMatch(/Systeminnstillinger/);
  });

  it("words a restricted microphone differently — the user cannot grant it themselves", () => {
    const denied = buildHealthFindings({
      permissions: { microphone: "denied" },
      t,
    })[0];
    const restricted = buildHealthFindings({
      permissions: { microphone: "restricted" },
      t,
    })[0];
    expect(restricted.message).not.toEqual(denied.message);
    expect(restricted.message).toMatch(/systemadministrator/);
  });

  it("only mentions the camera when video recording is on", () => {
    expect(
      buildHealthFindings({
        permissions: { camera: "denied" },
        videoEnabled: false,
        t,
      }),
    ).toEqual([]);
    expect(
      buildHealthFindings({
        permissions: { camera: "denied" },
        videoEnabled: true,
        t,
      }),
    ).toHaveLength(1);
  });

  it("reports a missing ffmpeg sidecar", () => {
    const found = buildHealthFindings({
      ffmpeg: { available: false, path: "ffmpeg" },
      t,
    });
    expect(found).toHaveLength(1);
    expect(found[0].message).toMatch(/ffmpeg/);
  });

  it("stays silent when a probe did not answer at all", () => {
    expect(buildHealthFindings({ t })).toEqual([]);
    expect(buildHealthFindings({ permissions: null, ffmpeg: null, t })).toEqual(
      [],
    );
    // An absent `available` is "we do not know", not "missing".
    expect(buildHealthFindings({ ffmpeg: {}, t })).toEqual([]);
  });

  it("can report several problems at once, mic first", () => {
    const found = buildHealthFindings({
      permissions: { microphone: "denied", camera: "denied" },
      ffmpeg: { available: false },
      videoEnabled: true,
      t,
    });
    expect(found).toHaveLength(3);
    expect(found[0].message).toMatch(/Mikrofon/);
  });
});
