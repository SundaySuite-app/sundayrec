import { SAVED_CHIP_MS, SAVE_COALESCE_MS } from "@lib/ui/bind-setting-core";
import { describe, expect, it } from "vitest";

import {
  IDLE_COMMIT_QUEUE,
  narrowToStored,
  runCommit,
  runPatchCommit,
  stepCommitQueue,
  type CommitDeps,
  type CommitOutcome,
  type CommitQueue,
  type GuardDescriptor,
  type PatchCommitDeps,
} from "./use-setting-core";

/**
 * One recorder for every effect the sequence can have. The assertions are on
 * the LOG, not on a return value: the order of effects is the contract here —
 * "apply before persist", "revert after a failed persist" — and an outcome
 * alone cannot tell a correct order from a lucky one.
 */
function harness(over: Partial<CommitDeps<unknown>> = {}) {
  const log: string[] = [];
  const deps: CommitDeps<unknown> = {
    previous: "before",
    next: "after",
    confirm: async (g: GuardDescriptor) => {
      log.push(`confirm:${g.title}`);
      return true;
    },
    apply: (v) => log.push(`apply:${String(v)}`),
    persist: async () => {
      log.push("persist");
      return true;
    },
    revert: (p) => log.push(`revert:${String(p)}`),
    toast: (kind, msg) => log.push(`toast:${kind}:${msg}`),
    saveFailedMessage: () => "Kunne ikke lagre innstillingen",
    after: (v) => log.push(`after:${String(v)}`),
    onReceipt: (r) => log.push(`receipt:${r}`),
    onError: (m) => log.push(`error:${m ?? "none"}`),
    ...over,
  };
  return { deps, log };
}

const run = async (
  over: Partial<CommitDeps<unknown>> = {},
): Promise<{ outcome: CommitOutcome; log: string[]; error: string | null }> => {
  const { deps, log } = harness(over);
  const result = await runCommit(deps);
  return { outcome: result.outcome, error: result.error, log };
};

describe("the happy path", () => {
  it("validates, applies, persists, then receipts — in that order", async () => {
    const { outcome, log } = await run();
    expect(outcome).toBe("saved");
    expect(log).toEqual([
      "error:none",
      "apply:after",
      "receipt:saving",
      "persist",
      "receipt:saved",
      "after:after",
    ]);
  });

  it("applies BEFORE it persists", async () => {
    // The user must see the change land while the write is in flight; a UI
    // that waits for the disk feels broken on a slow machine.
    const { log } = await run();
    expect(log.indexOf("apply:after")).toBeLessThan(log.indexOf("persist"));
  });

  it("coerces the raw value before anything else touches it", async () => {
    const { outcome, log } = await run({
      next: " 250 ",
      coerce: (raw) => Number(String(raw).trim()),
      validate: (v) => (typeof v === "number" ? null : "ikke et tall"),
    });
    expect(outcome).toBe("saved");
    expect(log).toContain("apply:250");
  });
});

describe("a change that is not a change", () => {
  it("does nothing at all when the value is unchanged", async () => {
    // Not merely "does not write": no receipt either. A «Lagret ✓» for a
    // non-change teaches the user to ignore the chip.
    const { outcome, log } = await run({ previous: "same", next: "same" });
    expect(outcome).toBe("skipped");
    expect(log).toEqual([]);
  });

  it("treats a cleared number field as different from zero", async () => {
    // `coerceValue` answers null (not NaN) for an empty field precisely so
    // this distinction survives; the sequence must not collapse it.
    const { outcome } = await run({ previous: 0, next: null });
    expect(outcome).toBe("saved");
  });

  it.each([
    ["false → true", false, true],
    ["empty → text", "", "Alta Frikirke"],
    ["null → number", null, 5],
  ])("commits %s", async (_name, previous, next) => {
    const { outcome } = await run({ previous, next });
    expect(outcome).toBe("saved");
  });
});

describe("validation", () => {
  it("rejects without writing, and leaves the typed value alone", async () => {
    const { outcome, error, log } = await run({
      validate: () => "Minimum er 500",
    });
    expect(outcome).toBe("invalid");
    expect(error).toBe("Minimum er 500");
    expect(log).toEqual(["error:Minimum er 500"]);
    // Reverting here would delete what the user was typing — the only way
    // they have of fixing the error.
    expect(log.some((l) => l.startsWith("revert"))).toBe(false);
    expect(log).not.toContain("persist");
  });

  it("clears a previous error when the value becomes valid", async () => {
    const { log } = await run({ validate: () => null });
    expect(log[0]).toBe("error:none");
  });

  it("never asks the guard about a value it is going to reject", async () => {
    const { log } = await run({
      validate: () => "nei",
      confirmIf: () => ({ title: "Bytte lydenhet nå?" }),
    });
    expect(log.some((l) => l.startsWith("confirm"))).toBe(false);
  });
});

describe("the guard", () => {
  const guard: GuardDescriptor = { title: "Bytte lydenhet nå?" };

  it("applies without asking when the guard has no opinion", async () => {
    const { outcome, log } = await run({ confirmIf: () => null });
    expect(outcome).toBe("saved");
    expect(log.some((l) => l.startsWith("confirm"))).toBe(false);
  });

  it("asks first, then applies when confirmed", async () => {
    const { outcome, log } = await run({ confirmIf: () => guard });
    expect(outcome).toBe("saved");
    expect(log.indexOf("confirm:Bytte lydenhet nå?")).toBeLessThan(
      log.indexOf("apply:after"),
    );
  });

  it("reverts and writes NOTHING when the user says no", async () => {
    const { outcome, log } = await run({
      confirmIf: () => guard,
      confirm: async () => false,
    });
    expect(outcome).toBe("declined");
    expect(log).toEqual(["error:none", "revert:before"]);
    expect(log).not.toContain("persist");
    // No receipt: nothing was saved, so nothing should say it was.
    expect(log.some((l) => l.startsWith("receipt"))).toBe(false);
  });
});

describe("a failed write — the one place app/ is stricter than legacy", () => {
  it("reverts the value, toasts, and reports failure", async () => {
    const { outcome, log } = await run({ persist: async () => false });
    expect(outcome).toBe("failed");
    expect(log).toEqual([
      "error:none",
      "apply:after",
      "receipt:saving",
      "revert:before",
      "toast:error:Kunne ikke lagre innstillingen",
      "receipt:failed",
    ]);
  });

  it("reverts to the value that is actually stored, not to a guess", async () => {
    const { log } = await run({
      previous: 128,
      next: 320,
      persist: async () => false,
    });
    expect(log).toContain("revert:128");
  });

  it("does not run `after` when the write failed", async () => {
    // `after` refreshes Home, re-derives the schedule… all of it would be
    // built on a value that is not stored.
    const { log } = await run({ persist: async () => false });
    expect(log.some((l) => l.startsWith("after"))).toBe(false);
  });

  it("treats a rejected persist as a failure, not as an exception to leak", async () => {
    // `saveSettingsDebounced` resolves false rather than rejecting; this
    // pins that the sequence would still be sane if that ever changed.
    await expect(
      runCommit(
        harness({
          persist: async () => false,
          toast: () => {},
        }).deps,
      ),
    ).resolves.toMatchObject({ outcome: "failed" });
  });
});

describe("the whole sequence, one row per path", () => {
  it.each([
    ["no change", { previous: "x", next: "x" }, "skipped"],
    ["invalid", { validate: () => "nei" }, "invalid"],
    [
      "declined",
      { confirmIf: () => ({ title: "?" }), confirm: async () => false },
      "declined",
    ],
    ["write failed", { persist: async () => false }, "failed"],
    ["saved", {}, "saved"],
  ] as Array<[string, Partial<CommitDeps<unknown>>, CommitOutcome]>)(
    "%s → %s",
    async (_name, over, expected) => {
      expect((await run(over)).outcome).toBe(expected);
    },
  );
});

describe("the timings come from bind-setting-core, not from a second copy", () => {
  it("uses the shared coalescing window and chip duration", () => {
    // Not a behaviour test — a pin. These two numbers exist once, and the day
    // someone tunes them the new shell must move with the old one.
    expect(SAVE_COALESCE_MS).toBeGreaterThan(0);
    expect(SAVED_CHIP_MS).toBeGreaterThan(SAVE_COALESCE_MS);
  });
});

describe("narrowToStored", () => {
  // A `<select>` always hands back a string. Half the settings behind one are
  // `i32` in Rust, and `Settings` deserialises strictly: `"30"` where serde
  // wants a number rejects the WHOLE save — the screen would say «Lagret ✓»
  // for a write that never landed, taking everything else in the same burst
  // with it.
  it.each([
    ["a number key takes the parsed value", 0, "30", 30],
    ["…including a negative one", -50, "-60", -60],
    ["…and zero, which is not «empty»", 15, "0", 0],
    ["a string key keeps the string", "256", "320", "320"],
    ["a boolean key is untouched", true, false, false],
    ["a number that is not a number stays for validate to reject", 0, "x", "x"],
    ["an empty field stays empty — that is «cleared», not 0", 0, "", ""],
    ["a number in, a number out", 0, 42, 42],
  ] as Array<[string, unknown, unknown, unknown]>)(
    "%s",
    (_name, previous, next, expected) => {
      expect(
        narrowToStored(
          previous as Parameters<typeof narrowToStored>[0],
          next as Parameters<typeof narrowToStored>[1],
        ),
      ).toBe(expected);
    },
  );
});

describe("stepCommitQueue — ingen commit forsvinner stille", () => {
  const busy: CommitQueue = { busy: true, queued: false };
  const busyQueued: CommitQueue = { busy: true, queued: true };

  it.each([
    ["ledig + forespørsel ⇒ kjør", IDLE_COMMIT_QUEUE, "request", "run", busy],
    [
      "opptatt + forespørsel ⇒ køes, ikke kastes",
      busy,
      "request",
      "queue",
      busyQueued,
    ],
    [
      "opptatt+kø + enda en forespørsel ⇒ fortsatt ÉN plass",
      busyQueued,
      "request",
      "queue",
      busyQueued,
    ],
    ["ferdig uten kø ⇒ ledig", busy, "settled", "idle", IDLE_COMMIT_QUEUE],
    ["ferdig MED kø ⇒ kjør en gang til", busyQueued, "settled", "run", busy],
  ] as Array<
    [string, CommitQueue, "request" | "settled", string, CommitQueue]
  >)("%s", (_name, state, event, action, next) => {
    const step = stepCommitQueue(state, event);
    expect(step.action).toBe(action);
    expect(step.next).toEqual(next);
  });

  it("en byge redigeringer under ÉN skrivning blir én ekstra kjøring", () => {
    // «900 dager på skjermen, 90 i basen»: den gamle hooken ryddet den ventende
    // timeren og returnerte så tomhendt fordi den var opptatt. Sekvensen under
    // er nøyaktig den — skriv, la commiten starte, skriv to ganger til — og
    // den MÅ ende i en kjøring som leser utkastet på nytt.
    let q = IDLE_COMMIT_QUEUE;
    const actions: string[] = [];
    for (const event of [
      "request",
      "request",
      "request",
      "settled",
      "settled",
    ] as const) {
      const step = stepCommitQueue(q, event);
      q = step.next;
      actions.push(step.action);
    }
    expect(actions).toEqual(["run", "queue", "queue", "run", "idle"]);
    expect(q).toEqual(IDLE_COMMIT_QUEUE);
  });
});

describe("runPatchCommit — den samme sekvensen for flere nøkler", () => {
  function patchHarness(over: Partial<PatchCommitDeps> = {}) {
    const log: string[] = [];
    const deps: PatchCommitDeps = {
      changed: true,
      apply: () => log.push("apply"),
      persist: async () => {
        log.push("persist");
        return true;
      },
      revert: () => log.push("revert"),
      toast: (kind, msg) => log.push(`toast:${kind}:${msg}`),
      saveFailedMessage: () => "Kunne ikke lagre innstillingen",
      after: () => {
        log.push("after");
      },
      onReceipt: (r) => log.push(`receipt:${r}`),
      ...over,
    };
    return { deps, log };
  }

  it("anvender, skriver, kvitterer — i den rekkefølgen", async () => {
    const { deps, log } = patchHarness();
    expect(await runPatchCommit(deps)).toBe("saved");
    expect(log).toEqual([
      "apply",
      "receipt:saving",
      "persist",
      "receipt:saved",
      "after",
    ]);
  });

  it("en ikke-endring skriver ingenting og kvitterer ikke", async () => {
    // En «Lagret ✓» for noe som ikke endret seg lærer brukeren å ignorere
    // kvitteringen — den samme regelen `isRealChange` gir `runCommit`.
    const { deps, log } = patchHarness({ changed: false });
    expect(await runPatchCommit(deps)).toBe("skipped");
    expect(log).toEqual([]);
  });

  it("vakten spør FØR noe anvendes, og et nei ruller tilbake", async () => {
    const { deps, log } = patchHarness({
      confirm: async () => {
        log.push("confirm");
        return false;
      },
    });
    expect(await runPatchCommit(deps)).toBe("declined");
    expect(log).toEqual(["confirm", "revert"]);
  });

  it("en feilet skrivning ruller tilbake, sier fra, og kvitteringen blir stående", async () => {
    // Strengere enn legacy, og av samme grunn som `runCommit`: skjermen skal
    // ikke stå og påstå noe basen ikke har.
    const { deps, log } = patchHarness({
      persist: async () => {
        log.push("persist");
        return false;
      },
    });
    expect(await runPatchCommit(deps)).toBe("failed");
    expect(log).toEqual([
      "apply",
      "receipt:saving",
      "persist",
      "revert",
      "toast:error:Kunne ikke lagre innstillingen",
      "receipt:failed",
    ]);
  });

  it("`after` kjøres bare når skrivningen faktisk landet", async () => {
    const { deps, log } = patchHarness({
      persist: async () => false,
    });
    await runPatchCommit(deps);
    expect(log).not.toContain("after");
  });
});
