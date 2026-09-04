// The promote gate's manifest check, and the admin-key source order.
//
// The shapes below are REAL: `betaManifest` and `stableManifest` are the
// platform key sets the live feeds served on 2026-08-08 for v0.11.0-beta.1
// (4 entries, NSIS-only Windows) and v0.10.0 (5 entries, MSI + NSIS), and
// `macOnlyManifest` is what release.yml's macOS leg actually uploaded to the
// draft when the Windows leg failed in run 31206918593. Signatures are
// truncated — only their presence/emptiness matters here.
import { afterEach, describe, expect, it, vi } from "vitest";

// Hoisted above the `promote-release.mjs` import below, so the module under
// test resolves this mock instead of the real `node:child_process` — the
// only way to assert "Keychain not touched" is to prove `spawnSync` itself
// was never called.
vi.mock("node:child_process", () => ({ spawnSync: vi.fn() }));

import { spawnSync } from "node:child_process";

import { manifestProblems, readAdminKey } from "./promote-release.mjs";

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

// P2 (F1-D1): the kill-switch used to be reachable from exactly one person's
// Keychain on exactly one Mac. SUNDAYREC_ADMIN_KEY is the documented
// emergency way around that (docs/ROLLBACK.md "Nødprosedyre uten Mac") — an
// env var checked BEFORE the Keychain, and which must short-circuit it
// entirely rather than merely taking priority once both are read.
describe("readAdminKey", () => {
  const ORIGINAL_ENV = process.env.SUNDAYREC_ADMIN_KEY;

  afterEach(() => {
    if (ORIGINAL_ENV === undefined) delete process.env.SUNDAYREC_ADMIN_KEY;
    else process.env.SUNDAYREC_ADMIN_KEY = ORIGINAL_ENV;
    vi.mocked(spawnSync).mockReset();
  });

  it("returns SUNDAYREC_ADMIN_KEY when set, and never spawns `security`", () => {
    process.env.SUNDAYREC_ADMIN_KEY = "test-emergency-key";
    expect(readAdminKey()).toBe("test-emergency-key");
    expect(spawnSync).not.toHaveBeenCalled();
  });

  it("trims the env value", () => {
    process.env.SUNDAYREC_ADMIN_KEY = "  test-emergency-key  \n";
    expect(readAdminKey()).toBe("test-emergency-key");
    expect(spawnSync).not.toHaveBeenCalled();
  });

  it("treats an empty/whitespace-only env var as unset and falls back to the Keychain", () => {
    process.env.SUNDAYREC_ADMIN_KEY = "   ";
    vi.mocked(spawnSync).mockReturnValue({
      status: 0,
      stdout: "keychain-key\n",
      stderr: "",
      error: undefined,
    });
    expect(readAdminKey()).toBe("keychain-key");
    expect(spawnSync).toHaveBeenCalledTimes(1);
  });

  it("falls back to the Keychain, unchanged, when the env var is unset", () => {
    delete process.env.SUNDAYREC_ADMIN_KEY;
    vi.mocked(spawnSync).mockReturnValue({
      status: 0,
      stdout: "keychain-key\n",
      stderr: "",
      error: undefined,
    });
    expect(readAdminKey()).toBe("keychain-key");
    expect(spawnSync).toHaveBeenCalledWith(
      "security",
      ["find-generic-password", "-s", "SundayRec telemetry admin key", "-w"],
      { encoding: "utf8" },
    );
  });
});
