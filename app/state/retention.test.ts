/**
 * Retensjonspasset — kvitteringen og stillheten.
 *
 * Det som bevises: et pass som flyttet noe SIER det (riktig flertall, med
 * veien til papirkurven), leser butikkene på nytt FØR det sier det, og et pass
 * som ikke flyttet noe — eller er slått av, eller feilet (shimmens fallback er
 * `disabled`) — er stille. Stillheten er halve kontrakten: passet kjører
 * uspurt ved hver oppstart, og en toast per oppstart uten innhold ville lært
 * folk å lukke toasts uten å lese dem.
 */

import { afterEach, beforeAll, describe, expect, it } from "vitest";

import type { PruneSummary } from "@legacy/bindings/PruneSummary";
import { setLocale } from "../i18n";
import { route } from "../router/router";
import { clearToasts, toasts } from "../ui/toast";
import { recordings } from "./recordings";
import { runRetentionPass, TOAST_MS } from "./retention";
import { trashEntries } from "./trash";

/** Et minimalt `window.api` for passet: kvitteringen + de to butikklesningene. */
function withFakeApi(summary: PruneSummary): void {
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      recordingsPrune: () => Promise.resolve(summary),
      trashList: () =>
        Promise.resolve([{ id: "t1" }, { id: "t2" }, { id: "t3" }]),
      getHistory: () => Promise.resolve([{ id: "rec-a" }]),
    },
  };
}

beforeAll(async () => {
  await setLocale("no");
});

afterEach(() => {
  clearToasts();
  trashEntries.value = null;
  recordings.value = null;
  route.value = { page: "record" };
  delete (globalThis as unknown as { window?: unknown }).window;
});

describe("retensjonspasset", () => {
  it("et pass som flyttet flere sier det, i flertall, og butikkene er alt lest", async () => {
    withFakeApi({ moved: 3, disabled: false });
    await runRetentionPass();

    expect(toasts.value).toHaveLength(1);
    const item = toasts.value[0];
    expect(item.msg).toBe("3 gamle opptak ble flyttet til papirkurven");
    expect(item.kind).toBe("info");
    // Lang nok til å bli sett, men ALDRI `0`: «blir stående» er reservert feil.
    expect(item.durationMs).toBe(TOAST_MS);
    // Butikkene FØR toasten — tallene bak meldingen stemmer idet den kan leses.
    expect(trashEntries.value).toHaveLength(3);
    expect(recordings.value).toHaveLength(1);
  });

  it("ett opptak får entallsformen", async () => {
    withFakeApi({ moved: 1, disabled: false });
    await runRetentionPass();
    expect(toasts.value[0]?.msg).toBe(
      "1 gammelt opptak ble flyttet til papirkurven",
    );
  });

  it("handlingen på toasten går til papirkurven", async () => {
    withFakeApi({ moved: 2, disabled: false });
    await runRetentionPass();
    const action = toasts.value[0]?.action;
    expect(action?.label).toBe("Vis papirkurven");
    action?.onClick();
    expect(route.value).toMatchObject({ page: "edit", tab: "trash" });
  });

  it("et pass uten flytting er stille", async () => {
    withFakeApi({ moved: 0, disabled: false });
    await runRetentionPass();
    expect(toasts.value).toHaveLength(0);
    // Og butikkene er ikke rørt — det finnes ingenting nytt å lese.
    expect(trashEntries.value).toBeNull();
  });

  it("avslått retensjon — og dermed også shimmens feil-fallback — er stille", async () => {
    // `disabled: true` er BÅDE «autoDeleteDays er 0» og shimmens svar når
    // selve IPC-en feilet. Begge skal se like ut herfra: ingen toast.
    withFakeApi({ moved: 0, disabled: true });
    await runRetentionPass();
    expect(toasts.value).toHaveLength(0);
  });
});
