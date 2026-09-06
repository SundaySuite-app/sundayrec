import { describe, expect, it } from "vitest";

import { isMissingFileFailure } from "./loader-core";

describe("isMissingFileFailure", () => {
  it("kjenner path_guard sin prosa for en fil som ikke lenger kan løses opp", () => {
    expect(
      isMissingFileFailure(
        "validation: cannot resolve path /Opptak/2026-08-23.flac: No such file or directory (os error 2)",
      ),
    ).toBe(true);
  });

  it("kjenner den stabile koden fra load_recording sin egen TOCTOU-sjekk", () => {
    expect(isMissingFileFailure("validation: file_not_found")).toBe(true);
  });

  it("en genuint ulesbar/ustøttet fil er IKKE en manglende fil", () => {
    expect(
      isMissingFileFailure(
        "recording error: ffprobe found no audio or video stream",
      ),
    ).toBe(false);
  });

  it("prosa som bare NEVNER ordet et sted matcher ikke — kun de to kjente formene", () => {
    expect(
      isMissingFileFailure(
        "internal: could not cannot resolve the mixer graph",
      ),
    ).toBe(false);
  });
});
