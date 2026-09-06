import { afterEach, describe, expect, it } from "vitest";

import { banners, clearBanners, dismissBanner, raiseBanner } from "./banners";

afterEach(() => {
  clearBanners();
});

// `state/banners.ts`'s general queue mechanics (nøklet, erstatning PÅ PLASS,
// idempotent dismiss) exercised through their OWN kinds elsewhere
// (`recording.test.ts`, `backend-warning.test.ts`). This file is the "update"
// entry's own contract — in particular F1-P1's `notes` field, which is DATA
// on the queue, never a translated string (see the filhead).
describe("the «update» banner", () => {
  it("carries the release note as data, alongside the version", () => {
    raiseBanner({
      key: "update",
      state: "available",
      version: "0.17.2-beta.1",
      percent: 0,
      notes: "Papirkurven tåler strømbrudd.",
    });

    const entry = banners.value.find((b) => b.key === "update");
    expect(entry?.key).toBe("update");
    if (entry?.key !== "update") return;
    expect(entry.notes).toBe("Papirkurven tåler strømbrudd.");
    expect(entry.version).toBe("0.17.2-beta.1");
  });

  it("a feed with no note raises the SAME banner with notes: null — never a missing field", () => {
    raiseBanner({
      key: "update",
      state: "available",
      version: "0.17.2-beta.1",
      percent: 0,
      notes: null,
    });

    const entry = banners.value.find((b) => b.key === "update");
    expect(entry?.key).toBe("update");
    if (entry?.key !== "update") return;
    // A caller that forgot the field entirely is a TypeScript error, not a
    // runtime `undefined` this test would otherwise miss — `toStrictEqual`
    // over the full entry pins that `notes` is a real, present key.
    expect(entry).toStrictEqual({
      key: "update",
      state: "available",
      version: "0.17.2-beta.1",
      percent: 0,
      notes: null,
    });
  });

  it("replaces IN PLACE across phases — the note the reader saw on «available» is still there on «ready»", () => {
    raiseBanner({
      key: "update",
      state: "available",
      version: "0.17.2-beta.1",
      percent: 0,
      notes: "Papirkurven tåler strømbrudd.",
    });
    const before = banners.value.length;

    // The transient download phase carries no note (see `state/auto-update.ts`).
    raiseBanner({
      key: "update",
      state: "downloading",
      version: "",
      percent: 40,
      notes: null,
    });
    expect(banners.value).toHaveLength(before); // updated, not stacked
    expect(banners.value.find((b) => b.key === "update")).toMatchObject({
      state: "downloading",
      notes: null,
    });

    // …and it comes back once the download finishes.
    raiseBanner({
      key: "update",
      state: "ready",
      version: "0.17.2-beta.1",
      percent: 100,
      notes: "Papirkurven tåler strømbrudd.",
    });
    expect(banners.value).toHaveLength(before);
    expect(banners.value.find((b) => b.key === "update")).toMatchObject({
      state: "ready",
      notes: "Papirkurven tåler strømbrudd.",
    });
  });

  it("dismiss removes it — idempotent, and does not touch other banners", () => {
    raiseBanner({
      key: "update",
      state: "available",
      version: "1.0.0",
      percent: 0,
      notes: null,
    });
    raiseBanner({
      key: "backend-disk-low",
      code: "disk_low",
      msg: null,
      severity: "warn",
      params: {},
    });

    dismissBanner("update");
    expect(banners.value.map((b) => b.key)).toEqual(["backend-disk-low"]);

    // A second dismiss of the same key is a no-op, not an error.
    dismissBanner("update");
    expect(banners.value.map((b) => b.key)).toEqual(["backend-disk-low"]);
  });
});
