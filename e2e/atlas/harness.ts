import type { Page, ConsoleMessage } from "@playwright/test";
import { boot, type BootOptions } from "../harness";
import { emit, emitEvent, spyEvents } from "../events";
import type { ConsoleFinding } from "./report";

// What the atlas needs on top of the browser tier's `boot()`.
//
// Three things, and none of them are new machinery: the event bridge and the
// fixture seam are the ones every other spec already uses. What is added here
// is the part a PHOTOGRAPH needs and a test does not — a clock that does not
// move, meters that have stopped moving, and a shutter that waits for the last
// frame instead of the first assertion.
//
//  1. **Backend EVENTS.** A large share of the app's states are painted by a
//     Tauri event, not by an invoke: the recording overlay's meter
//     (`recording://levels`), the missed-recording card (`scheduler://missed`),
//     the pre-start check (`scheduler://preflight`), the update banner.
//     `e2e/harness.ts` resolves `plugin:event|listen` with an id and then never
//     emits — correct for a test tier ("nothing is emitting" is the truth), but
//     it means those screens can only ever be photographed empty.
//
//     The bridge is `e2e/events.ts` — the SAME one the ordinary specs use, not
//     a second copy. Fase A's atlas carried its own `installEventBridge`
//     because `e2e/events.ts` did not exist yet; keeping both would be two
//     bridges that drift, which is the one bug this repo has spent the most
//     nights on. It must be installed BEFORE `boot()`: an init script only
//     applies to navigations added after it.
//
//  2. **A fixed clock.** Every relative label in the app ("om 3 dager",
//     "for 2 timer siden", the recording timer) is derived from `Date.now()`.
//     `page.clock.setFixedTime` pins it so two runs of the atlas produce the
//     same pixels, while leaving timers and rAF running so the UI still paints.
//
//  3. **A loud emit.** `emitAt`/`emitEventAt` fail the scene when NOBODY was
//     listening. A silent zero means the screen this scene wanted is not driven
//     by that event name any more — a finding, and one that would otherwise
//     show up as a perfectly ordinary photograph of the wrong state.

/**
 * The moment every atlas screenshot is taken at: Sunday 23 August 2026, 10:55
 * — five minutes before a service.
 *
 * ⚠️ Written as an ABSOLUTE instant with an explicit offset, not as
 * `new Date(2026, 7, 23, 10, 55)`. The local-parts constructor is evaluated in
 * NODE's timezone, while the page renders in the one the config pins
 * (`timezoneId: "Europe/Oslo"`). On a runner set to anything else the two
 * would disagree, and every date label in the atlas would move by that many
 * hours — a diff that looks exactly like a code change. `+02:00` is CEST,
 * which is what Europe/Oslo is in August.
 */
export const ATLAS_NOW = new Date("2026-08-23T10:55:00+02:00");

/** A recent-but-past Sunday, for recordings that already happened. */
export const LAST_SUNDAY_MS = Date.parse("2026-08-16T11:00:00+02:00");

/** The Sunday before that — so a list of recordings has more than one date. */
export const PREV_SUNDAY_MS = Date.parse("2026-08-09T11:00:00+02:00");

/** An older one still, for the trash (its «deletes in N days» counts from here). */
export const OLD_SUNDAY_MS = Date.parse("2026-07-26T11:00:00+02:00");

/** The next scheduled start the scheduler would answer with. Zone-less local
 *  ISO, the shape `scheduler://next` and `scheduler_status.next` both carry. */
export const NEXT_SERVICE_ISO = "2026-08-30T11:00:00";

/** A special recording far enough out that `specialRows`' «date >= today»
 *  filter (in LOCAL time) can never drop it. */
export const SPECIAL_DATE = "2027-12-24";

/** Boot a scene: fixed clock, event bridge, then the ordinary browser-tier
 *  boot with the scene's fixtures and the locale under the `language` key. */
export async function bootScene(
  page: Page,
  locale: string,
  opts: BootOptions,
): Promise<void> {
  await page.clock.setFixedTime(ATLAS_NOW);
  await spyEvents(page);
  await boot(page, {
    ...opts,
    settings: { ...(opts.settings ?? {}), language: locale },
  });
}

/**
 * Fire a `window.api.on` channel and INSIST somebody took it.
 *
 * The bare `emit` returns a count and lets a caller ignore it. A photographer
 * must not: a zero here means the scene photographs the state it booted into
 * rather than the state it asked for, and the picture is indistinguishable from
 * a correct one.
 */
export async function emitAt(
  page: Page,
  channel: string,
  payload: unknown = null,
): Promise<void> {
  const n = await emit(page, channel, payload);
  if (n === 0)
    throw new Error(
      `ingen lytter på kanalen «${channel}» — scenen ville fotografert feil skjerm`,
    );
}

/** The same, for the backend's own event names (the `listen()` route). */
export async function emitEventAt(
  page: Page,
  event: string,
  payload: unknown = null,
): Promise<void> {
  const n = await emitEvent(page, event, payload);
  if (n === 0)
    throw new Error(
      `ingen lytter på eventet «${event}» — scenen ville fotografert feil skjerm`,
    );
}

/**
 * One VU packet. Steady values on purpose: a drifting meter is a screenshot
 * that differs every run.
 *
 * ⚠️ The two meters in the app read DIFFERENT payloads, and `emit` hands the
 * payload straight to the handler without going through api-shim's
 * `EVENT_ADAPTERS`, so the shape has to be right here:
 *
 *   `vu-levels`        `VuLevels`       — `{ rms_dbfs[], peak_dbfs[] }`, one
 *                                         entry per NATIVE channel.
 *   `recording-levels` `RecordingLevels`— `{ peak_db_left, peak_db_right }`,
 *                                         peaks only, and `null` on the right
 *                                         means MONO rather than "silent".
 *
 * Getting this wrong is quiet: the meter subscribes, the emit reports a
 * listener, and the bars stay at the floor — a photograph of a recording that
 * looks like a dead microphone.
 */
export function vuPacket(
  channel: string,
  rms = -18,
  peak = -9,
  channels = 2,
): unknown {
  if (channel === "recording-levels")
    return { peak_db_left: peak, peak_db_right: peak };
  return {
    rms_dbfs: Array.from({ length: channels }, () => rms),
    peak_dbfs: Array.from({ length: channels }, () => peak),
  };
}

/**
 * Push enough identical VU packets that the meter's smoothing has converged,
 * then stop — the bars hold their last painted position because painting is
 * driven by packets.
 *
 * Converged and not merely "a few packets in" is the whole point: the smoothing
 * is exponential, so an early frame is a bar somewhere between silence and the
 * value, and «somewhere between» is a different number of pixels on every run.
 */
export async function settleVu(
  page: Page,
  channel = "vu-levels",
  rms = -18,
  peak = -9,
): Promise<void> {
  for (let i = 0; i < 40; i++) {
    await emitAt(page, channel, vuPacket(channel, rms, peak));
    await page.evaluate(
      () => new Promise((r) => requestAnimationFrame(() => r(null))),
    );
  }
}

/**
 * Move the frozen clock forward.
 *
 * The app's own second-ticker then picks the new time up on its next `Date.now()`
 * read, so a recording timer can be photographed reading 42 minutes instead of
 * the 00:00:00 a clock that never moves would always show — and it still reads
 * exactly 42 minutes on every run.
 */
export async function advanceClock(page: Page, minutes: number): Promise<void> {
  await page.clock.setFixedTime(
    new Date(ATLAS_NOW.getTime() + minutes * 60_000),
  );
}

/**
 * A camera in the webview, without hardware.
 *
 * `navigator.mediaDevices` is replaced with a stand-in that answers with a
 * canvas stream — a REAL `MediaStream`, so `<video>.srcObject` and `play()`
 * take the ordinary route. Lifted from `e2e/record.spec.ts`, where the same
 * stub is what makes the ownership assertions possible.
 *
 * Not `--use-fake-device-for-media-stream`: a browser flag would give a real
 * camera pipeline with a real start-up time, which is exactly the timing that
 * makes media tests flaky. This is deterministic, and a photographer needs that
 * more than a test does.
 */
export async function stubCamera(
  page: Page,
  fail?: "NotAllowedError" | "NotFoundError",
): Promise<void> {
  await page.addInitScript((mode: string) => {
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: {
        enumerateDevices: async () => [
          {
            kind: "videoinput",
            label: "Logitech BRIO (046d:085e)",
            deviceId: "brio-id",
            groupId: "g1",
          },
        ],
        getUserMedia: async () => {
          if (mode) {
            const err = new Error("stub");
            err.name = mode;
            throw err;
          }
          const canvas = document.createElement("canvas");
          canvas.width = 320;
          canvas.height = 180;
          const ctx = canvas.getContext("2d");
          if (ctx) {
            // A flat, dark rectangle — not noise and not a gradient. The point
            // of the picture is that there IS one; anything with detail would
            // be detail that has to be identical on the next run.
            ctx.fillStyle = "#16202f";
            ctx.fillRect(0, 0, 320, 180);
          }
          return (
            canvas as HTMLCanvasElement & {
              captureStream: (fps?: number) => MediaStream;
            }
          ).captureStream(5);
        },
      },
    });
  }, fail ?? "");
}

/**
 * A clipboard that records instead of writing.
 *
 * Only needed so «Kopier full rapport» can be photographed with its toast: the
 * real `navigator.clipboard` is permission-gated and would reject, which paints
 * the failure toast instead of the receipt.
 */
export async function stubClipboard(page: Page): Promise<void> {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: () => Promise.resolve() },
    });
  });
}

// ── The console guard ────────────────────────────────────────────────────────

/**
 * Collect `console.error` and uncaught page errors for one scene.
 *
 * The atlas runs with NO backend, so a large share of what lands here is the
 * harness telling the truth ("no Tauri backend in the browser tier: …"). Those
 * are classified out in the report; what remains is real debt.
 */
export function watchConsole(page: Page): ConsoleFinding[] {
  const found: ConsoleFinding[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error")
      found.push({ kind: "console.error", text: m.text() });
  });
  page.on("pageerror", (e: Error) =>
    found.push({ kind: "pageerror", text: e.message }),
  );
  return found;
}
