import { describe, expect, it } from "vitest";

import { detectOs, type Os, type PlatformFacts } from "./platform-core";

/**
 * Én rad per kilde-kombinasjon. Tabellen finnes fordi svaret bestemmer om en
 * kontroll RENDRES: «Klassisk lyd-motor (DirectShow)» er meningsløs på macOS
 * og er den ene nødutgangen på en hakkete Windows-rigg.
 */
const CASES: Array<[string, PlatformFacts, Os]> = [
  [
    "userAgentData wins over everything else",
    { uaDataPlatform: "Windows", platform: "MacIntel", userAgent: "Linux" },
    "win",
  ],
  [
    "navigator.platform when there is no userAgentData (WKWebView)",
    { uaDataPlatform: null, platform: "MacIntel", userAgent: "" },
    "mac",
  ],
  ["Win32", { platform: "Win32" }, "win"],
  ["Linux x86_64", { platform: "Linux x86_64" }, "linux"],
  [
    "the real WKWebView UA, with no platform at all",
    {
      userAgent:
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)",
    },
    "mac",
  ],
  [
    "a Windows UA",
    {
      userAgent:
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126 Safari/537.36",
    },
    "win",
  ],
  ["nothing at all", {}, "other"],
  ["empty strings say nothing", { platform: "", userAgent: "" }, "other"],
  [
    "a product name is not an operating system",
    { userAgent: "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) Winamp/5" },
    "linux",
  ],
];

describe("detectOs", () => {
  for (const [name, facts, expected] of CASES) {
    it(name, () => {
      expect(detectOs(facts)).toBe(expected);
    });
  }

  it("prefers navigator.platform over the UA string", () => {
    // The one that matters for the DirectShow row: a UA that mentions Windows
    // (a compatibility token, a product name) must not outvote the structured
    // value the engine actually set.
    expect(
      detectOs({
        platform: "MacIntel",
        userAgent: "Mozilla/5.0 (Macintosh) SomethingWindowsLike/1.0",
      }),
    ).toBe("mac");
  });
});
