/**
 * `confirmAndStop` — the stop dialog can stay open for minutes; its «stop»
 * answer must not hit a take that ended (or was replaced) while it was open
 * (audit 2026-10-05: «confirmAndStop sjekker ikke tilstanden etter dialogen»).
 */

import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { setLocale } from "../../i18n";
import {
  isRecording,
  markSessionStarted,
  endSessionLocally,
} from "../../state/recording";
import { activeDialog, cancelAllDialogs, resolveDialog } from "../../ui/dialog";
import { confirmAndStop } from "./stop";

let stops = 0;

beforeAll(async () => {
  await setLocale("no");
});

beforeEach(() => {
  stops = 0;
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      stopRecordingNow: async () => {
        stops++;
      },
    },
  };
  markSessionStarted();
});

afterEach(() => {
  cancelAllDialogs();
  endSessionLocally();
  delete (globalThis as unknown as { window?: unknown }).window;
});

function answer(keep: boolean): void {
  const open = activeDialog.value;
  if (!open) throw new Error("no dialog is open");
  resolveDialog(open.id, keep);
}

describe("confirmAndStop", () => {
  it("stopper når brukeren velger «Stopp» og opptaket fortsatt går", async () => {
    const p = confirmAndStop();
    answer(false);
    await p;
    expect(stops).toBe(1);
  });

  it("stopper ikke når brukeren velger å la opptaket gå", async () => {
    const p = confirmAndStop();
    answer(true);
    await p;
    expect(stops).toBe(0);
  });

  it("stopper IKKE hvis opptaket tok slutt mens dialogen sto åpen", async () => {
    const p = confirmAndStop();
    endSessionLocally();
    answer(false);
    await p;
    expect(isRecording.peek()).toBe(false);
    expect(stops).toBe(0);
  });

  it("stopper IKKE et NYTT opptak som startet mens dialogen sto åpen", async () => {
    const p = confirmAndStop();
    endSessionLocally();
    // A different take: a later start timestamp.
    const realNow = Date.now;
    Date.now = () => realNow() + 5000;
    try {
      markSessionStarted();
    } finally {
      Date.now = realNow;
    }
    answer(false);
    await p;
    expect(stops).toBe(0);
  });

  it("et andre trykk mens spørsmålet står åpent kuer ikke en andre dialog", async () => {
    const p = confirmAndStop();
    const first = activeDialog.value;
    void confirmAndStop();
    expect(activeDialog.value).toBe(first);
    answer(true);
    await p;
  });
});
