import { describe, it, expect } from "vitest";
import {
  attemptsInvoke,
  fixtureWins,
  fixturesHonored,
  lookupFixture,
  readFixture,
  resolveSource,
  type FixtureGate,
  type PrecedenceInput,
} from "./fixtures-core";

const gate = (over: Partial<FixtureGate> = {}): FixtureGate => ({
  inTauri: false,
  devBuild: true,
  requested: false,
  ...over,
});

describe("fixturesHonored — when an override is allowed at all", () => {
  it("always honours fixtures outside Tauri, dev build or not, param or not", () => {
    // A plain browser has no backend: a fixture shadows nothing, it only decides
    // whether the screen shows canned data or the empty state it would show
    // anyway. This is the Playwright tier's whole foundation.
    for (const devBuild of [true, false]) {
      for (const requested of [true, false]) {
        expect(fixturesHonored(gate({ inTauri: false, devBuild, requested }))).toBe(true);
      }
    }
  });

  it("inside Tauri requires BOTH a dev build and ?fixtures=1", () => {
    expect(fixturesHonored(gate({ inTauri: true, devBuild: true, requested: true }))).toBe(true);
    expect(fixturesHonored(gate({ inTauri: true, devBuild: true, requested: false }))).toBe(false);
    expect(fixturesHonored(gate({ inTauri: true, devBuild: false, requested: true }))).toBe(false);
    expect(fixturesHonored(gate({ inTauri: true, devBuild: false, requested: false }))).toBe(false);
  });

  it("a shipped in-Tauri build cannot be driven by a page, whatever it asks for", () => {
    // The guarantee E5.1 owes the shipped app: devBuild is the literal `false`
    // Vite inlines in a production build, so this branch is dead code there.
    const shipped = gate({ inTauri: true, devBuild: false, requested: true });
    expect(fixturesHonored(shipped)).toBe(false);
    expect(fixtureWins(shipped, true)).toBe(false);
    expect(attemptsInvoke(shipped, true)).toBe(true);
  });
});

describe("resolveSource — the full precedence table", () => {
  const at = (over: Partial<PrecedenceInput>): PrecedenceInput => ({
    inTauri: false,
    devBuild: true,
    requested: false,
    hasFixture: false,
    invokeSucceeds: true,
    ...over,
  });

  it("with NO fixture installed, behaviour is exactly the pre-E5 behaviour", () => {
    // The load-bearing property of the whole seam: it is inert until used.
    for (const inTauri of [true, false]) {
      for (const requested of [true, false]) {
        expect(resolveSource(at({ inTauri, requested, hasFixture: false, invokeSucceeds: true })))
          .toBe("invoke");
        expect(resolveSource(at({ inTauri, requested, hasFixture: false, invokeSucceeds: false })))
          .toBe("fallback");
      }
    }
  });

  it("outside Tauri a fixture beats the (always-rejecting) invoke", () => {
    expect(resolveSource(at({ inTauri: false, hasFixture: true, invokeSucceeds: false })))
      .toBe("fixture");
  });

  it("outside Tauri, no fixture still means the empty-state fallback", () => {
    expect(resolveSource(at({ inTauri: false, hasFixture: false, invokeSucceeds: false })))
      .toBe("fallback");
  });

  it("inside Tauri the REAL backend wins over an un-opted-in fixture", () => {
    expect(
      resolveSource(
        at({ inTauri: true, requested: false, hasFixture: true, invokeSucceeds: true }),
      ),
    ).toBe("invoke");
  });

  it("inside Tauri an un-opted-in fixture does not rescue a failing command either", () => {
    // Deliberate: with the gate closed the fixture is invisible, so a failing
    // command degrades to the caller's fallback exactly as E2.4 describes.
    expect(
      resolveSource(
        at({ inTauri: true, requested: false, hasFixture: true, invokeSucceeds: false }),
      ),
    ).toBe("fallback");
  });

  it("inside a DEV build with ?fixtures=1 the fixture overrides a WORKING backend", () => {
    // This is the manual-QA case: drive the real app into a rare state.
    expect(
      resolveSource(
        at({ inTauri: true, devBuild: true, requested: true, hasFixture: true, invokeSucceeds: true }),
      ),
    ).toBe("fixture");
  });
});

describe("attemptsInvoke — a fixture hit is not an IPC failure", () => {
  it("short-circuits the invoke when the fixture wins", () => {
    expect(attemptsInvoke(gate({ inTauri: false }), true)).toBe(false);
    expect(
      attemptsInvoke(gate({ inTauri: true, devBuild: true, requested: true }), true),
    ).toBe(false);
  });

  it("still invokes when there is no fixture, or the gate is closed", () => {
    expect(attemptsInvoke(gate({ inTauri: false }), false)).toBe(true);
    expect(attemptsInvoke(gate({ inTauri: true, requested: false }), true)).toBe(true);
  });
});

describe("lookupFixture", () => {
  it("hits on an own key", () => {
    expect(lookupFixture({ settings_get: { a: 1 } }, "settings_get")).toEqual({
      hit: true,
      value: { a: 1 },
    });
  });

  it("treats an explicit `undefined` as a real fixture (void commands)", () => {
    // `settings_save`-shaped commands return nothing; `map[cmd] !== undefined`
    // would have silently fallen through to a live invoke.
    expect(lookupFixture({ stop_vu: undefined }, "stop_vu")).toEqual({ hit: true, value: undefined });
  });

  it("misses on an absent key", () => {
    expect(lookupFixture({ settings_get: 1 }, "history_list").hit).toBe(false);
  });

  it("does not inherit Object.prototype members as fixtures", () => {
    // A command literally named `toString`/`constructor` must not resolve to a
    // prototype member — that would answer a real command with a function.
    expect(lookupFixture({}, "toString").hit).toBe(false);
    expect(lookupFixture({}, "constructor").hit).toBe(false);
    expect(lookupFixture({}, "hasOwnProperty").hit).toBe(false);
  });

  it("misses safely on a missing or non-object map", () => {
    expect(lookupFixture(undefined, "settings_get").hit).toBe(false);
    expect(lookupFixture("nope" as never, "settings_get").hit).toBe(false);
    expect(lookupFixture(null as never, "settings_get").hit).toBe(false);
  });
});

describe("readFixture", () => {
  it("returns a plain value untouched", () => {
    const v = { rows: [1, 2, 3] };
    expect(readFixture(v)).toBe(v);
  });

  it("calls a function fixture with the invoke args", () => {
    const fx = (args?: Record<string, unknown>) => `saw:${String(args?.deviceName)}`;
    expect(readFixture(fx, { deviceName: "Qu-5" })).toBe("saw:Qu-5");
  });

  it("passes undefined args through to an arg-less invoke", () => {
    expect(readFixture((args?: Record<string, unknown>) => args === undefined)).toBe(true);
  });

  it("propagates a throwing fixture — that is how a test simulates a rejection", () => {
    expect(() => readFixture(() => {
      throw new Error("device busy");
    })).toThrow("device busy");
  });
});
