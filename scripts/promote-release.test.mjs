// The promote gate's manifest check.
//
// The shapes below are REAL: `betaManifest` and `stableManifest` are the
// platform key sets the live feeds served on 2026-08-08 for v0.11.0-beta.1
// (4 entries, NSIS-only Windows) and v0.10.0 (5 entries, MSI + NSIS), and
// `macOnlyManifest` is what release.yml's macOS leg actually uploaded to the
// draft when the Windows leg failed in run 31206918593. Signatures are
// truncated — only their presence/emptiness matters here.
import { describe, expect, it } from "vitest";

import { manifestProblems } from "./promote-release.mjs";

const entry = (sig = "dW50cnVzdGVk…") => ({
  signature: sig,
  url: "https://api.github.com/repos/SundaySuite-app/sundayrec/releases/assets/1",
});

const betaManifest = {
  version: "0.11.0-beta.1",
  platforms: {
    "darwin-aarch64": entry(),
    "darwin-aarch64-app": entry(),
    "windows-x86_64": entry(),
    "windows-x86_64-nsis": entry(),
  },
};

const stableManifest = {
  version: "0.10.0",
  platforms: {
    "darwin-aarch64": entry(),
    "darwin-aarch64-app": entry(),
    "windows-x86_64": entry(),
    "windows-x86_64-msi": entry(),
    "windows-x86_64-nsis": entry(),
  },
};

describe("manifestProblems", () => {
  it("accepts the NSIS-only beta manifest (4 entries, no msi key)", () => {
    // The whole point of the --bundles nsis change: a beta legitimately has no
    // windows-x86_64-msi entry, and that must not read as a broken release.
    expect(manifestProblems(betaManifest, "v0.11.0-beta.1")).toEqual([]);
  });

  it("accepts the 5-entry stable manifest", () => {
    expect(manifestProblems(stableManifest, "v0.10.0")).toEqual([]);
  });

  it("rejects the mac-only manifest a half-failed matrix leaves behind", () => {
    const macOnly = {
      version: "0.11.0-beta.1",
      platforms: {
        "darwin-aarch64": entry(),
        "darwin-aarch64-app": entry(),
      },
    };
    const problems = manifestProblems(macOnly, "v0.11.0-beta.1");
    expect(problems).toHaveLength(2);
    expect(problems.join("\n")).toContain("windows-x86_64");
    expect(problems.join("\n")).toContain("windows-x86_64-nsis");
  });

  it("rejects a Windows-only manifest too (the mirror-image failure)", () => {
    const winOnly = {
      version: "0.10.0",
      platforms: {
        "windows-x86_64": entry(),
        "windows-x86_64-nsis": entry(),
        "windows-x86_64-msi": entry(),
      },
    };
    const problems = manifestProblems(winOnly, "v0.10.0");
    expect(problems).toHaveLength(2);
    expect(problems.join("\n")).toContain("darwin-aarch64");
  });

  it("rejects an unsigned entry — the build succeeds, every client refuses it", () => {
    const unsigned = {
      ...betaManifest,
      platforms: {
        ...betaManifest.platforms,
        "windows-x86_64-nsis": entry(""),
      },
    };
    const problems = manifestProblems(unsigned, "v0.11.0-beta.1");
    expect(problems).toEqual([expect.stringContaining("no signature")]);
  });

  it("rejects an entry with no download url", () => {
    const noUrl = {
      ...betaManifest,
      platforms: {
        ...betaManifest.platforms,
        "darwin-aarch64": { signature: "sig", url: "" },
      },
    };
    expect(manifestProblems(noUrl, "v0.11.0-beta.1")).toEqual([
      expect.stringContaining("no download url"),
    ]);
  });

  it("rejects a manifest whose version disagrees with the tag", () => {
    const problems = manifestProblems(betaManifest, "v0.11.0-beta.2");
    expect(problems).toEqual([
      expect.stringContaining('says version "0.11.0-beta.1"'),
    ]);
  });

  it("strips the leading v when comparing tag to version", () => {
    expect(manifestProblems(stableManifest, "v0.10.0")).toEqual([]);
  });

  it("rejects a manifest with no platforms object at all", () => {
    const problems = manifestProblems({ version: "0.10.0" }, "v0.10.0");
    expect(problems).toEqual([expect.stringContaining("no `platforms`")]);
  });

  it("rejects a non-object manifest instead of throwing", () => {
    expect(manifestProblems(null, "v0.10.0")).toEqual([
      expect.stringContaining("not a JSON object"),
    ]);
    expect(manifestProblems("nope", "v0.10.0")).toEqual([
      expect.stringContaining("not a JSON object"),
    ]);
  });
});
