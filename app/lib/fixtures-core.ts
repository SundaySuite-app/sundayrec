// The fixture seam's precedence rules (E5.1) — pure, no DOM, no Tauri.
//
// ## The problem this exists for
//
// 84 % of the renderer has no test and no way to get one: `vitest.config.ts` is
// node-env-only on purpose, so every DOM shell (`api-shim.ts`, `pages/home.ts`,
// `pages/editor-page.ts`, …) is unreachable from the unit gate. The way IN is
// the fact that the renderer already boots in a plain browser: `call()` wraps
// every `invoke` in try/catch, so outside Tauri each command yields its fallback
// and the UI renders complete EMPTY states.
//
// Empty is not enough. A journey test wants a history list with rows in it, an
// editor with a loaded file, a settings screen with values. That needs a way to
// answer a command with canned data — without a backend, and without the tests
// reaching into the renderer's internals.
//
// So: an injectable fixture source (`window.__SUNDAYREC_FIXTURES__`), consulted
// by the ONE `invoke` wrapper in `api-shim.ts`, keyed by Tauri command name.
//
// ## The precedence, in one sentence
//
//   an HONOURED fixture  >  the real `invoke`  >  the command's fallback.
//
// Spelled out:
//
//   1. A fixture that is honoured (see below) SHORT-CIRCUITS: `invoke` is never
//      attempted. That is what makes it an override, and it is also why a
//      fixture hit is NOT an IPC failure — nothing failed, so E2.4's failure
//      ring and its toast never see it.
//   2. Otherwise the real `invoke` runs, exactly as before.
//   3. If that rejects, the caller's `fallback` wins — E2.4's ring records the
//      failure and may toast, entirely unchanged.
//
// The load-bearing property: **with no fixtures installed, every path through
// this module is a no-op**. `lookupFixture` misses, nothing short-circuits, and
// `call()` behaves byte-for-byte as it did before E5.
//
// ## When a fixture is honoured
//
//   - OUTSIDE Tauri (a plain browser: `npm run dev` in Chrome, the Playwright
//     tier): ALWAYS. There is no backend there, so a fixture shadows nothing —
//     it only decides whether the screen shows canned data or the empty state it
//     would show anyway. No privileged surface is reachable either way.
//   - INSIDE Tauri: only in a DEV build AND only when the page was opened with
//     `?fixtures=1`. Both, deliberately. The query param is the ergonomic switch
//     (manual QA of a rare state — a full disk, a failing device — against the
//     real app); the dev-build flag is the guarantee, because Vite replaces
//     `import.meta.env.DEV` with the literal `false` in a production build, so
//     `FIXTURE_GATE.devBuild` is `false` in the shipped bundle and this
//     function's in-Tauri branch can only ever answer `false` there.
//
//     Note what that is and is not: the CODE below still ships — it is a plain
//     exported function, not something a bundler can prove unreachable — the
//     CONDITION is what is nailed shut. `fixturesHonored` is honest about which
//     one it is because "the branch is eliminated" and "the branch always
//     answers false" fail differently the day someone passes a hand-built gate.
//     Either way a shipped SundayRec CANNOT be driven by fixtures, whatever a
//     page puts on `window`.
//
// ## Simulating failures
//
// A fixture may be a function; if it throws (or returns a rejecting promise),
// the rejection propagates out of the wrapper exactly like a real `invoke`
// rejection — so `call()` records it in the failure ring and falls back. That is
// how a test drives the E2.4 toast without breaking a backend.

/** A fixture that computes its answer from the invoke args. May throw to
 *  simulate a rejected command. */
export type FixtureFn = (args?: Record<string, unknown>) => unknown;

/** A canned answer for one command: either the value itself, or a function of
 *  the invoke args. `undefined` is a legitimate value (plenty of commands return
 *  void), which is why presence is decided by key ownership, never by `!== undefined`. */
export type FixtureValue = unknown | FixtureFn;

/** `Tauri command name` → canned answer. Installed on `window`. */
export type FixtureMap = Record<string, FixtureValue>;

/** The `window` property the renderer reads fixtures from. */
export const FIXTURE_GLOBAL = "__SUNDAYREC_FIXTURES__";

/** The query param that opts an in-Tauri DEV build into honouring fixtures. */
export const FIXTURE_QUERY_PARAM = "fixtures";

/** The three inputs the honour decision is made from. */
export interface FixtureGate {
  /** `isTauri()` — is there a real backend behind `invoke`? */
  inTauri: boolean;
  /** `import.meta.env.DEV` — a dev build, not a shipped bundle. */
  devBuild: boolean;
  /** Was the page opened with `?fixtures=1`? */
  requested: boolean;
}

/**
 * Whether fixtures are allowed to override at all in this boot.
 *
 * See the module header: unconditional outside Tauri (nothing to shadow),
 * dev-build + explicit opt-in inside it.
 */
export function fixturesHonored(gate: FixtureGate): boolean {
  if (!gate.inTauri) return true;
  return gate.devBuild && gate.requested;
}

/** Where a command's answer came from. */
export type CallSource = "invoke" | "fixture" | "fallback";

/** Everything the precedence decision depends on. */
export interface PrecedenceInput extends FixtureGate {
  /** Does the installed fixture map own a key for this command? */
  hasFixture: boolean;
  /** Would the real backend answer this command? (Only consulted when the
   *  fixture does not short-circuit.) */
  invokeSucceeds: boolean;
}

/**
 * Whether a fixture short-circuits `invoke` for this command.
 *
 * This is the whole seam in one predicate: `true` means the wrapper returns the
 * canned answer and never touches Tauri.
 */
export function fixtureWins(gate: FixtureGate, hasFixture: boolean): boolean {
  return hasFixture && fixturesHonored(gate);
}

/**
 * The full precedence table, as one pure function — the thing the unit test
 * pins. Given the gate, whether a fixture exists, and whether the real command
 * would succeed: what does the caller actually get?
 */
export function resolveSource(input: PrecedenceInput): CallSource {
  if (fixtureWins(input, input.hasFixture)) return "fixture";
  return input.invokeSucceeds ? "invoke" : "fallback";
}

/**
 * Whether the real `invoke` is attempted at all.
 *
 * Matters beyond tidiness: an attempted-and-rejected invoke is what feeds E2.4's
 * failure ring. A short-circuited one must not, or a fixtured browser boot would
 * fill the diagnostics panel with failures that never happened.
 */
export function attemptsInvoke(
  gate: FixtureGate,
  hasFixture: boolean,
): boolean {
  return !fixtureWins(gate, hasFixture);
}

/** A fixture lookup result. Split from the value because `undefined` is a
 *  perfectly good canned answer for the many void commands. */
export interface FixtureLookup {
  hit: boolean;
  value: FixtureValue;
}

/**
 * Look `cmd` up in the installed map.
 *
 * Uses own-key ownership rather than `map[cmd] !== undefined` for two reasons:
 * a fixture may legitimately BE `undefined`, and a map made with `{}` inherits
 * `constructor`/`toString` from `Object.prototype` — without the own check,
 * a command called `toString` would "have a fixture".
 */
export function lookupFixture(
  map: FixtureMap | undefined,
  cmd: string,
): FixtureLookup {
  if (!map || typeof map !== "object") return { hit: false, value: undefined };
  if (!Object.prototype.hasOwnProperty.call(map, cmd))
    return { hit: false, value: undefined };
  return { hit: true, value: map[cmd] };
}

/**
 * Turn a fixture into its answer: call it with the invoke args if it is a
 * function, otherwise hand back the value as-is.
 *
 * Throwing is deliberate and load-bearing (see the module header): a fixture
 * function that throws simulates a rejected command, which is the only way to
 * drive the E2.4 failure path from a test.
 */
export function readFixture(
  value: FixtureValue,
  args?: Record<string, unknown>,
): unknown {
  return typeof value === "function" ? (value as FixtureFn)(args) : value;
}
