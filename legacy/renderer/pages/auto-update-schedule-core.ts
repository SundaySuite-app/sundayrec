/**
 * Whether the automatic update check should be running, and what to do about
 * it — the pure half of the «Oppdater automatisk» toggle, split out so it is
 * testable in the node-only vitest gate (see vitest.config.ts: no jsdom, by
 * decision). The DOM layer (general-page.ts) owns the timer handle and the IPC
 * call; everything decidable is here.
 *
 * PRIVACY.md states the contract this exists to keep: «Slår du den av, tar
 * appen ikke kontakt med serveren — verken ved oppstart eller den vanlige
 * sjekken hver time.» A setting that only holds until the next restart does not
 * keep that promise, so the decision below has to be answerable at any moment,
 * not once at wire-up.
 *
 * The manual «Se etter oppdateringer nå» button is deliberately outside this
 * module's reach — see PRIVACY.md's stated exception, and the comment on its
 * handler in general-page.ts.
 */

/**
 * How often the automatic check runs. Inherited from the Electron app's updater
 * (startup + every 60 min); it is also the number PRIVACY.md quotes to the
 * operator («den vanlige sjekken hver time»), so changing it changes a promise.
 */
export const AUTO_UPDATE_INTERVAL_MS = 60 * 60 * 1000

/**
 * The default-on rule, in one place.
 *
 * Only an explicit `false` counts as off: `settings` starts life as `{}` and is
 * filled in when the persisted blob arrives, so `undefined` means "not answered
 * yet", which must not read as "the operator switched it off".
 */
export function autoUpdateEnabled(setting: boolean | undefined | null): boolean {
  return setting !== false
}

/** What the DOM layer must do to make the world match the setting. */
export interface AutoUpdateScheduleAction {
  /**
   * Arm the schedule: check once now, then every `AUTO_UPDATE_INTERVAL_MS`.
   * The immediate check is part of arming so that startup and a mid-session
   * switch-on are the same path rather than two behaviours to keep in step.
   */
  start: boolean
  /** Cancel the running interval. */
  stop: boolean
}

/**
 * Decide the transition from the timer's real state to the setting's.
 *
 * Both flags are transitions, never restatements of the desired state: that is
 * what makes re-arming idempotent. An operator who flips the toggle off and on
 * a few times, or a settings reload that runs twice, must not end up with two
 * intervals — two intervals is twice the traffic the operator was told about,
 * and nothing on screen would ever show it.
 */
export function planAutoUpdateSchedule(
  running: boolean,
  enabled: boolean,
): AutoUpdateScheduleAction {
  return { start: enabled && !running, stop: !enabled && running }
}
