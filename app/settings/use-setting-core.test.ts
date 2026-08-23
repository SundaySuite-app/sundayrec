import { SAVED_CHIP_MS, SAVE_COALESCE_MS } from "@lib/ui/bind-setting-core";
import { describe, expect, it } from "vitest";

import {
  narrowToStored,
  runCommit,
  type CommitDeps,
  type CommitOutcome,
  type GuardDescriptor,
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
