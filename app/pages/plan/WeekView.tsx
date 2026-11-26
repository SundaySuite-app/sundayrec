/**
 * Ukevisningen — PLAN (F6). Mandag–søndag som kolonner, tid nedover, hvert
 * faste opptak som en blokk du drar og strekker. Tidene gjentas hver uke.
 *
 * Erstatter `AutoRecordCard`s ene tid og «Flere tider»-listen: samme data
 * (`settings.slots`), samme skriving, men du ser uka i stedet for å regne den ut.
 *
 * ## Tynt lag
 *
 * Alle regler — piksler↔tid, snapping, minimum, midnatt, overlapp — bor i
 * `plan-core.ts` og er testet der. Denne fila holder bare på pekeren, tastene,
 * og det som skal tegnes. Å lagre går gjennom `patchSettings` +
 * `saveSettingsDebounced` som alt annet, med tilbakerulling hvis lagringen
 * feiler.
 *
 * ## Tre veier til samme endring
 *
 *   • PEKER: dra i en tom flate (ny), i en blokk (flytt), i kantene (strekk).
 *     Et trykk uten dra åpner redigeringen.
 *   • TASTATUR: blokkene er fokuserbare. Piler flytter (kvarter/dag),
 *     Skift + opp/ned endrer lengden, Enter åpner redigeringen, Delete fjerner.
 *   • SKJEMA: «Legg til tid» og redigeringen tar eksakte klokkeslett.
 *
 * ## Overlapp forbys
 *
 * Planleggeren tar opp én ting om gangen (se `plan-core.ts`). En flytting som
 * ville lagt to blokker oppå hverandre blir ikke gjort, og linja under
 * rutenettet sier hvilken tid det støtt mot.
 *
 * ## Lagring ved hvert tastetrykk
 *
 * `usePatch` avviser en skriving mens en annen er i lufta; for en pil som
 * holdes nede er det feil svar. Her skrives alltid mot `settings.peek()` —
 * det som gjelder AKKURAT NÅ — og lagringen samles i `saveSettingsDebounced`.
 */

import type { JSX } from "preact";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "preact/hooks";

import { formatWakeHint, intlParts } from "@lib/status/next-recording-core";
import { capitalizeFirst } from "@lib/ui/capitalize";

import { locale, t, tDyn, tf, tn } from "../../i18n";
import { useReceipt } from "../../settings/use-receipt";
import { nextRecording } from "../../state/next-recording";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
  type Settings,
} from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { Receipt } from "../../ui/Receipt/Receipt";
import { Select } from "../../ui/Select/Select";
import { toast } from "../../ui/toast";
import {
  appendPlacement,
  clamp,
  createPlacement,
  dayFromX,
  dayHeightPx,
  DAY_MINUTES,
  deleteOccurrence,
  DRAG_THRESHOLD_PX,
  endOf,
  findConflict,
  fitDuration,
  HOUR_PX,
  initialTopMinutes,
  keyPlacement,
  minutesOfDate,
  minutesToPx,
  MAX_DURATION,
  MIN_DURATION,
  missedKeysThisWeek,
  movePlacement,
  nextKey,
  occurrenceKey,
  occurrencesOf,
  pxToMinutes,
  rangeLabel,
  replaceOccurrence,
  resizeEnd,
  resizeStart,
  samePlacement,
  sameRef,
  segmentsOf,
  slotTimes,
  suggestPlacement,
  weekdayOf,
  type Occurrence,
  type OccurrenceRef,
  type Placement,
  type PlacementKey,
  type Segment,
} from "./plan-core";
import {
  DEFAULT_PLAN,
  durationBetween,
  isValidTime,
  minutesOfDay,
  timeOfDay,
  WEEKDAYS,
  type Weekday,
} from "./schedule-core";
import styles from "./plan.module.css";

const HINT_ID = "plan-keys-hint";

/** Hvor mange piksler rutenettet rulles tilbake så første timeetikett er synlig. */
const HOUR_LABEL_CLEARANCE_PX = 12;

type MessageKind = "info" | "error";

/** Det som tegnes mens en gest pågår. */
type Preview =
  | { kind: "move"; ref: OccurrenceRef; placement: Placement; bad: boolean }
  | { kind: "create"; placement: Placement; bad: boolean };

type Handle = "move" | "start" | "end";

type Gesture =
  | {
      kind: "create";
      day: Weekday;
      fromMinutes: number;
      x0: number;
      y0: number;
      moved: boolean;
    }
  | {
      kind: Handle;
      ref: OccurrenceRef;
      origin: Placement;
      part: Segment["part"];
      x0: number;
      y0: number;
      moved: boolean;
      el: HTMLElement;
    };

interface Editing {
  /** `null` = en ny tid fra «Legg til». */
  ref: OccurrenceRef | null;
  initial: Placement;
  anchor: HTMLElement;
}

/** Gjeldende opptredener — alltid fra det som er lagret NÅ. */
function currentOccurrences(): Occurrence[] {
  return occurrencesOf(settings.peek().slots);
}

function dayName(day: number): string {
  return tDyn("app.setup.days", String(day));
}

function overlapText(conflict: Occurrence): string {
  return tf("app.plan.errOverlap", {
    day: dayName(conflict.day),
    range: rangeLabel(conflict),
  });
}

function blockLabel(occ: Occurrence): string {
  const times = slotTimes(occ);
  return tf("app.plan.block", {
    day: dayName(occ.day),
    start: times.start,
    stop: times.stop,
  });
}

export function WeekView() {
  const s = settings.value;
  const slots = s.slots ?? [];
  const armed = s.autoRecordEnabled !== false;
  const state = nextRecording.value;

  const occs = useMemo(() => occurrencesOf(slots), [slots]);
  const [now, setNow] = useState(() => new Date());
  const [preview, setPreview] = useState<Preview | null>(null);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [message, setMessage] = useState<{
    kind: MessageKind;
    text: string;
  } | null>(null);
  const { receipt, show: showReceipt } = useReceipt();

  const wrapRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const colsRef = useRef<HTMLDivElement>(null);
  const gesture = useRef<Gesture | null>(null);
  const focusAfter = useRef<OccurrenceRef | null>(null);
  const scrolled = useRef(false);

  // «Nå»-linja og «i dag» følger klokka.
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 30_000);
    return () => clearInterval(id);
  }, []);

  // Rutenettet åpner slik at det finnes en tid å se (én gang).
  useLayoutEffect(() => {
    if (scrolled.current || !scrollRef.current) return;
    scrolled.current = true;
    const visible = pxToMinutes(scrollRef.current.clientHeight);
    // Litt luft over første timelinje: etiketten står midt på linja og ville
    // ellers havnet halvt under den klebrige dagsraden.
    scrollRef.current.scrollTop = Math.max(
      0,
      minutesToPx(initialTopMinutes(occs, visible)) - HOUR_LABEL_CLEARANCE_PX,
    );
  }, [occs]);

  // Etter en tastaturflytting havner fokus på blokken igjen, selv om den bytte
  // dag (og dermed ble en ny DOM-node).
  useEffect(() => {
    const ref = focusAfter.current;
    if (!ref) return;
    focusAfter.current = null;
    const el = wrapRef.current?.querySelector<HTMLElement>(
      `[data-block="${ref.index}-${ref.day}"]`,
    );
    el?.focus({ preventScroll: false });
  }, [slots]);

  const nextAt = state.next?.at ?? null;
  const nextK = nextKey(nextAt);
  const missedKeys = useMemo(
    () => missedKeysThisWeek(state.missed, now),
    [state.missed, now],
  );

  // ── Skriving ──────────────────────────────────────────────────────────────

  async function write(
    patch: Partial<Settings>,
    done: { kind: MessageKind; text: string } | null,
  ): Promise<void> {
    const cur = settings.peek() as Record<string, unknown>;
    const before: Record<string, unknown> = {};
    for (const key of Object.keys(patch)) before[key] = cur[key];
    patchSettings(patch);
    setMessage(done);
    showReceipt("saving");
    const ok = await saveSettingsDebounced(120);
    if (ok) {
      showReceipt("saved");
      return;
    }
    patchSettings(before as Partial<Settings>);
    showReceipt("failed");
    toast("error", t("general.saveFailed"));
  }

  function fail(text: string): void {
    setMessage({ kind: "error", text });
  }

  /** Flytt/strekk en opptreden. `false` = ikke gjort (overlapp eller ingen endring). */
  function apply(ref: OccurrenceRef, next: Placement): boolean {
    const cur = settings.peek().slots ?? [];
    const occ = occurrencesOf(cur).find((o) => sameRef(o, ref));
    if (!occ) return false;
    if (samePlacement(occ, next)) return false;
    const conflict = findConflict(occurrencesOf(cur), next, ref);
    if (conflict) {
      fail(overlapText(conflict));
      return false;
    }
    const out = replaceOccurrence(cur, ref, next);
    focusAfter.current = out.ref;
    void write(
      { slots: out.slots },
      {
        kind: "info",
        text: tf("app.plan.changed", {
          day: dayName(next.day),
          range: rangeLabel(next),
        }),
      },
    );
    return true;
  }

  function create(p: Placement): boolean {
    const cur = settings.peek().slots ?? [];
    const out = appendPlacement(cur, p);
    focusAfter.current = out.ref;
    void write(
      // Å legge til en tid ARMERER — som «Legg til» alltid har gjort. En tid
      // som ikke teller er en tid ingen ba om.
      { slots: out.slots, autoRecordEnabled: true },
      {
        kind: "info",
        text: tf("app.plan.added", {
          day: dayName(p.day),
          range: rangeLabel(p),
        }),
      },
    );
    return true;
  }

  function remove(ref: OccurrenceRef): void {
    const cur = settings.peek().slots ?? [];
    const occ = occurrencesOf(cur).find((o) => sameRef(o, ref));
    if (!occ) return;
    void write(
      { slots: deleteOccurrence(cur, ref) },
      {
        kind: "info",
        text: tf("app.plan.removed", {
          day: dayName(occ.day),
          range: rangeLabel(occ),
        }),
      },
    );
    setEditing(null);
    // Blokken er borte: fokus til knappen som alltid finnes.
    queueMicrotask(() =>
      wrapRef.current
        ?.querySelector<HTMLElement>('[data-testid="plan-add"]')
        ?.focus(),
    );
  }

  // ── Pekeren ───────────────────────────────────────────────────────────────

  function colsRect(): DOMRect | null {
    return colsRef.current?.getBoundingClientRect() ?? null;
  }

  function minutesAt(clientY: number): number {
    const rect = colsRect();
    return rect ? pxToMinutes(clientY - rect.top) : 0;
  }

  function dayAt(clientX: number): Weekday {
    const rect = colsRect();
    return rect ? dayFromX(clientX - rect.left, rect.width) : 0;
  }

  function listen(): void {
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onEscape);
  }

  function unlisten(): void {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onCancel);
    window.removeEventListener("keydown", onEscape);
  }

  function onEscape(event: KeyboardEvent): void {
    if (event.key === "Escape") onCancel();
  }

  function onCancel(): void {
    gesture.current = null;
    unlisten();
    setPreview(null);
  }

  function previewFor(g: Gesture, e: PointerEvent): Preview {
    const all = currentOccurrences();
    if (g.kind === "create") {
      const placement = createPlacementFrom(g, e);
      return {
        kind: "create",
        placement,
        bad: findConflict(all, placement) !== null,
      };
    }
    const dMin = pxToMinutes(e.clientY - g.y0);
    let placement: Placement;
    if (g.kind === "move") {
      placement = movePlacement(g.origin, dayAt(e.clientX) - dayAt(g.x0), dMin);
    } else if (g.kind === "start") {
      placement = resizeStart(g.origin, g.origin.start + dMin);
    } else {
      placement = resizeEnd(
        g.origin,
        endOf(g.origin) + dMin,
        g.part === "tail" ? undefined : DAY_MINUTES,
      );
    }
    return {
      kind: "move",
      ref: g.ref,
      placement,
      bad: findConflict(all, placement, g.ref) !== null,
    };
  }

  function createPlacementFrom(
    g: Extract<Gesture, { kind: "create" }>,
    e: PointerEvent,
  ): Placement {
    return createPlacement(
      g.day,
      g.fromMinutes,
      minutesAt(e.clientY),
      !g.moved,
    );
  }

  function onMove(e: PointerEvent): void {
    const g = gesture.current;
    if (!g) return;
    if (!g.moved) {
      if (Math.hypot(e.clientX - g.x0, e.clientY - g.y0) < DRAG_THRESHOLD_PX) {
        return;
      }
      g.moved = true;
    }
    setPreview(previewFor(g, e));
  }

  function onUp(e: PointerEvent): void {
    const g = gesture.current;
    gesture.current = null;
    unlisten();
    setPreview(null);
    if (!g) return;

    if (g.kind === "create") {
      const wanted = createPlacementFrom(g, e);
      const all = currentOccurrences();
      if (g.moved) {
        const conflict = findConflict(all, wanted);
        if (conflict) fail(overlapText(conflict));
        else create(wanted);
        return;
      }
      // Et trykk: standardvarigheten, men kortet til det som får plass.
      const fitted = fitDuration(all, wanted);
      if (fitted) create(fitted);
      else fail(t("app.plan.errNoRoom"));
      return;
    }

    if (!g.moved) {
      setEditing({ ref: g.ref, initial: g.origin, anchor: g.el });
      return;
    }
    const p = previewFor(g, e);
    if (p.kind === "move" && p.bad) {
      const conflict = findConflict(currentOccurrences(), p.placement, g.ref);
      if (conflict) fail(overlapText(conflict));
      return;
    }
    if (p.kind === "move") apply(g.ref, p.placement);
  }

  function beginCreate(
    e: JSX.TargetedPointerEvent<HTMLDivElement>,
    day: Weekday,
  ) {
    if (e.button !== 0) return;
    if (editing) {
      // Første trykk utenfor lukker redigeringen; det lager ingen ny tid.
      setEditing(null);
      return;
    }
    e.preventDefault();
    setMessage(null);
    gesture.current = {
      kind: "create",
      day,
      fromMinutes: minutesAt(e.clientY),
      x0: e.clientX,
      y0: e.clientY,
      moved: false,
    };
    listen();
  }

  function beginBlock(
    e: JSX.TargetedPointerEvent<HTMLElement>,
    seg: Segment,
    kind: Handle,
    el: HTMLElement,
  ) {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    setMessage(null);
    setEditing(null);
    const ref = { index: seg.occ.index, day: seg.occ.day };
    gesture.current = {
      kind,
      ref,
      origin: {
        day: seg.occ.day,
        start: seg.occ.start,
        duration: seg.occ.duration,
      },
      part: seg.part,
      x0: e.clientX,
      y0: e.clientY,
      moved: false,
      el,
    };
    listen();
  }

  // ── Tastaturet ────────────────────────────────────────────────────────────

  function onBlockKey(
    e: JSX.TargetedKeyboardEvent<HTMLDivElement>,
    seg: Segment,
  ) {
    if (e.target !== e.currentTarget) return;
    const ref = { index: seg.occ.index, day: seg.occ.day };
    const occ = currentOccurrences().find((o) => sameRef(o, ref));
    if (!occ) return;

    let move: PlacementKey | null;
    switch (e.key) {
      case "ArrowUp":
        move = e.shiftKey ? "shrink" : "up";
        break;
      case "ArrowDown":
        move = e.shiftKey ? "grow" : "down";
        break;
      case "ArrowLeft":
        move = e.shiftKey ? null : "left";
        break;
      case "ArrowRight":
        move = e.shiftKey ? null : "right";
        break;
      case "Enter":
      case " ":
        e.preventDefault();
        setEditing({ ref, initial: occ, anchor: e.currentTarget });
        return;
      case "Delete":
      case "Backspace":
        e.preventDefault();
        remove(ref);
        return;
      default:
        return;
    }
    e.preventDefault();
    if (move === null) return;
    const next = keyPlacement(occ, move);
    if (samePlacement(occ, next)) return;
    setEditing(null);
    apply(ref, next);
  }

  // ── «Legg til tid» ────────────────────────────────────────────────────────

  function openAdd(anchor: HTMLElement): void {
    setMessage(null);
    const suggestion = suggestPlacement(currentOccurrences(), {
      day: DEFAULT_PLAN.day,
      start: minutesOfDay(DEFAULT_PLAN.start) ?? 660,
      duration: DEFAULT_PLAN.minutes,
    });
    if (!suggestion) {
      fail(t("app.plan.errNoRoom"));
      return;
    }
    setEditing({ ref: null, initial: suggestion, anchor });
  }

  // ── Tegningen ─────────────────────────────────────────────────────────────

  const shown: Occurrence[] = occs.map((o) =>
    preview?.kind === "move" && sameRef(o, preview.ref)
      ? { ...o, ...preview.placement }
      : o,
  );
  const segments = shown.flatMap(segmentsOf);
  const todayDay = weekdayOf(now);
  const nowMinutes = minutesOfDate(now);

  const parts = state.next ? intlParts(locale.value)(state.next.atMs) : null;
  const wake = formatWakeHint(state, {
    t,
    tf,
    tn,
    parts: intlParts(locale.value),
    nowMs: now.getTime(),
  });

  return (
    <section
      ref={wrapRef}
      class={styles.wrap}
      data-testid="plan-week"
      data-armed={armed ? "true" : "false"}
      aria-label={t("app.plan.gridLabel")}
      style={`--hour-px:${HOUR_PX}px`}
    >
      <div class={styles.toolbar}>
        <div class={styles.nextWrap}>
          <div class={styles.nextLabel}>{t("app.record.nextAuto")}</div>
          {parts ? (
            <div data-testid="plan-next" class={styles.nextValue}>
              {tf("app.record.nextAutoWhen", {
                day: capitalizeFirst(parts.weekdayLong, locale.value),
                time: parts.time,
              })}
            </div>
          ) : (
            <div data-testid="plan-next-none" class={styles.nextValue}>
              {t("app.record.nextNone")}
            </div>
          )}
          {wake ? (
            <div data-testid="plan-next-wake" class={styles.nextWake}>
              {wake}
            </div>
          ) : null}
        </div>
        <Receipt state={receipt} testId="plan-receipt" />
        <Button
          variant="secondary"
          testId="plan-add"
          onClick={(e) => openAdd(e.currentTarget)}
        >
          {t("app.plan.add")}
        </Button>
      </div>

      {!armed ? (
        <p class={styles.offNote} data-testid="plan-off">
          {t("app.plan.off")}
        </p>
      ) : null}

      <div class={styles.scroll} ref={scrollRef} data-testid="plan-scroll">
        <div class={styles.head}>
          <div />
          <div class={styles.days}>
            {WEEKDAYS.map((d) => (
              <div
                key={d}
                class={styles.dayHead}
                data-testid={`plan-head-${d}`}
                aria-current={d === todayDay ? "date" : undefined}
              >
                {dayName(d)}
              </div>
            ))}
          </div>
        </div>

        <div class={styles.body} style={{ height: `${dayHeightPx()}px` }}>
          <div class={styles.hours} aria-hidden="true">
            {Array.from({ length: 23 }, (_, i) => i + 1).map((h) => (
              <span
                key={h}
                class={styles.hour}
                style={{ top: `${minutesToPx(h * 60)}px` }}
              >
                {timeOfDay(h * 60)}
              </span>
            ))}
          </div>

          <div class={styles.cols} ref={colsRef} data-testid="plan-cols">
            {WEEKDAYS.map((d) => (
              <div
                key={d}
                class={styles.col}
                data-testid={`plan-col-${d}`}
                data-day={d}
                data-today={d === todayDay ? "true" : "false"}
                onPointerDown={(e) => beginCreate(e, d)}
              >
                {d === todayDay ? (
                  <div
                    class={styles.now}
                    data-testid="plan-now"
                    aria-hidden="true"
                    style={{ top: `${minutesToPx(nowMinutes)}px` }}
                  />
                ) : null}

                {segments
                  .filter((seg) => seg.day === d)
                  .map((seg) => (
                    <Block
                      key={seg.key}
                      seg={seg}
                      armed={armed}
                      isNext={
                        seg.part !== "tail" &&
                        nextK === occurrenceKey(seg.occ.day, seg.occ.start)
                      }
                      isMissed={
                        seg.part !== "tail" &&
                        missedKeys.has(
                          occurrenceKey(seg.occ.day, seg.occ.start),
                        )
                      }
                      dragging={
                        preview?.kind === "move" &&
                        sameRef(seg.occ, preview.ref)
                      }
                      bad={
                        preview?.kind === "move" &&
                        sameRef(seg.occ, preview.ref) &&
                        preview.bad
                      }
                      onPointerDown={beginBlock}
                      onKeyDown={onBlockKey}
                    />
                  ))}

                {preview?.kind === "create" && preview.placement.day === d ? (
                  <GhostSegments
                    placement={preview.placement}
                    bad={preview.bad}
                  />
                ) : null}
                {preview?.kind === "create" &&
                endOf(preview.placement) > DAY_MINUTES &&
                (preview.placement.day + 1) % 7 === d ? (
                  <GhostTail placement={preview.placement} bad={preview.bad} />
                ) : null}
              </div>
            ))}

            {occs.length === 0 && !preview ? (
              <div class={styles.empty} data-testid="plan-empty">
                <span class={styles.emptyNote}>{t("app.plan.empty")}</span>
              </div>
            ) : null}
          </div>
        </div>
      </div>

      <p
        class={styles.status}
        role="status"
        aria-live="polite"
        data-testid="plan-message"
        data-kind={message?.kind ?? "idle"}
      >
        {message?.text ?? ""}
      </p>
      <p class={styles.hint} id={HINT_ID} data-testid="plan-hint">
        {t("app.plan.keys")}
      </p>

      {editing ? (
        <SlotEditor
          key={editing.ref ? `${editing.ref.index}-${editing.ref.day}` : "new"}
          editing={editing}
          wrap={wrapRef}
          onClose={() => {
            const el = editing.anchor;
            setEditing(null);
            queueMicrotask(() => el.isConnected && el.focus());
          }}
          onSave={(placement) => {
            const ref = editing.ref;
            if (ref === null) {
              const conflict = findConflict(currentOccurrences(), placement);
              if (conflict) return overlapText(conflict);
              create(placement);
            } else {
              const conflict = findConflict(
                currentOccurrences(),
                placement,
                ref,
              );
              if (conflict) return overlapText(conflict);
              apply(ref, placement);
            }
            setEditing(null);
            return null;
          }}
          onDelete={
            editing.ref ? () => remove(editing.ref as OccurrenceRef) : null
          }
        />
      ) : null}
    </section>
  );
}

// ── Blokken ─────────────────────────────────────────────────────────────────

interface BlockProps {
  seg: Segment;
  armed: boolean;
  isNext: boolean;
  isMissed: boolean;
  dragging: boolean;
  bad: boolean;
  onPointerDown: (
    e: JSX.TargetedPointerEvent<HTMLElement>,
    seg: Segment,
    kind: Handle,
    el: HTMLElement,
  ) => void;
  onKeyDown: (
    e: JSX.TargetedKeyboardEvent<HTMLDivElement>,
    seg: Segment,
  ) => void;
}

function Block({
  seg,
  armed,
  isNext,
  isMissed,
  dragging,
  bad,
  onPointerDown,
  onKeyDown,
}: BlockProps) {
  const { occ } = seg;
  const times = slotTimes(occ);
  const heightPx = Math.max(minutesToPx(seg.to - seg.from), 6);
  const short = heightPx < 30;
  const status = isNext
    ? t("app.plan.next")
    : isMissed
      ? t("app.plan.missed")
      : null;
  const label = [
    blockLabel(occ),
    seg.part === "tail" ? t("app.plan.continues") : null,
    isNext ? t("app.plan.blockNext") : null,
    isMissed ? t("app.plan.blockMissed") : null,
  ]
    .filter(Boolean)
    .join(", ");

  return (
    <div
      class={styles.block}
      role="button"
      tabIndex={0}
      aria-label={label}
      aria-describedby={HINT_ID}
      data-testid="plan-block"
      data-block={`${occ.index}-${occ.day}`}
      data-index={occ.index}
      data-day={occ.day}
      data-part={seg.part}
      data-start={times.start}
      data-stop={times.stop}
      data-next={isNext ? "true" : undefined}
      data-missed={isMissed ? "true" : undefined}
      data-off={armed ? undefined : "true"}
      data-short={short ? "true" : undefined}
      data-dragging={dragging ? "true" : undefined}
      data-bad={bad ? "true" : undefined}
      style={{
        top: `${minutesToPx(seg.from)}px`,
        height: `${heightPx}px`,
      }}
      onPointerDown={(e) => onPointerDown(e, seg, "move", e.currentTarget)}
      onKeyDown={(e) => onKeyDown(e, seg)}
    >
      {seg.part !== "tail" ? (
        <span
          class={styles.handle}
          data-handle="start"
          data-testid="plan-handle-start"
          aria-hidden="true"
          onPointerDown={(e) => {
            const block = e.currentTarget.parentElement as HTMLElement;
            onPointerDown(e, seg, "start", block);
          }}
        />
      ) : null}
      <span class={styles.blockLabel}>{rangeLabel(occ)}</span>
      {status && !short ? <span class={styles.badge}>{status}</span> : null}
      {seg.part !== "head" ? (
        <span
          class={styles.handle}
          data-handle="end"
          data-testid="plan-handle-end"
          aria-hidden="true"
          onPointerDown={(e) => {
            const block = e.currentTarget.parentElement as HTMLElement;
            onPointerDown(e, seg, "end", block);
          }}
        />
      ) : null}
    </div>
  );
}

function GhostSegments({
  placement,
  bad,
}: {
  placement: Placement;
  bad: boolean;
}) {
  const to = Math.min(endOf(placement), DAY_MINUTES);
  return (
    <div
      class={styles.ghost}
      data-testid="plan-ghost"
      data-bad={bad ? "true" : undefined}
      style={{
        top: `${minutesToPx(placement.start)}px`,
        height: `${minutesToPx(to - placement.start)}px`,
      }}
    >
      {rangeLabel(placement)}
    </div>
  );
}

function GhostTail({ placement, bad }: { placement: Placement; bad: boolean }) {
  return (
    <div
      class={styles.ghost}
      data-bad={bad ? "true" : undefined}
      style={{
        top: "0px",
        height: `${minutesToPx(endOf(placement) - DAY_MINUTES)}px`,
      }}
    />
  );
}

// ── Redigeringen ────────────────────────────────────────────────────────────

interface SlotEditorProps {
  editing: Editing;
  wrap: { current: HTMLDivElement | null };
  onClose: () => void;
  /** Svarer med en feiltekst, eller `null` når den ble lagret. */
  onSave: (placement: Placement) => string | null;
  onDelete: (() => void) | null;
}

/**
 * Den lille redigeringen ved siden av blokken: dag, start og slutt som ekte
 * `<input type="time">` (OS-ets egen tidsvelger og tastatur; et halvskrevet
 * klokkeslett kan ikke sendes inn), og Slett.
 *
 * Eksakte klokkeslett er lov her også utenfor kvarter-rutenettet — rutenettet
 * er en snarvei, ikke en grense for hva bakenden kan.
 */
function SlotEditor({
  editing,
  wrap,
  onClose,
  onSave,
  onDelete,
}: SlotEditorProps) {
  const { initial, ref } = editing;
  const popRef = useRef<HTMLFormElement>(null);
  const startRef = useRef<HTMLInputElement>(null);
  const [day, setDay] = useState<Weekday>(initial.day);
  const [start, setStart] = useState(timeOfDay(initial.start));
  const [stop, setStop] = useState(timeOfDay(endOf(initial)));
  const [error, setError] = useState<string | null>(null);

  // Plassér ved siden av blokken: til høyre hvis det er plass, ellers til
  // venstre; aldri utenfor kortet.
  useLayoutEffect(() => {
    const pop = popRef.current;
    const box = wrap.current;
    if (!pop || !box) return;
    const a = editing.anchor.getBoundingClientRect();
    const w = box.getBoundingClientRect();
    const pw = pop.offsetWidth;
    const ph = pop.offsetHeight;
    const gap = 8;
    let left = a.right - w.left + gap;
    if (left + pw > w.width) left = a.left - w.left - pw - gap;
    left = clamp(left, gap, Math.max(gap, w.width - pw - gap));
    const top = clamp(a.top - w.top, gap, Math.max(gap, w.height - ph - gap));
    pop.style.left = `${left}px`;
    pop.style.top = `${top}px`;
  }, [editing.anchor, wrap]);

  useEffect(() => {
    startRef.current?.focus();
  }, []);

  const validTimes = isValidTime(start) && isValidTime(stop);
  const duration = validTimes ? durationBetween(start, stop) : 0;
  const lengthOk =
    validTimes &&
    start !== stop &&
    duration >= MIN_DURATION &&
    duration <= MAX_DURATION;
  const placement: Placement | null = lengthOk
    ? { day, start: minutesOfDay(start) ?? 0, duration }
    : null;

  const problem = !validTimes
    ? t("app.plan.errTime")
    : !lengthOk
      ? t("app.plan.errLength")
      : placement
        ? (() => {
            const conflict = findConflict(currentOccurrences(), placement, ref);
            return conflict ? overlapText(conflict) : null;
          })()
        : null;

  const crosses = placement ? endOf(placement) > DAY_MINUTES : false;

  function submit(e: Event): void {
    e.preventDefault();
    if (!placement) return;
    setError(onSave(placement));
  }

  return (
    <form
      ref={popRef}
      class={styles.pop}
      role="dialog"
      aria-modal="false"
      aria-label={ref ? t("app.plan.editTitle") : t("app.plan.newTitle")}
      data-testid="plan-editor"
      onSubmit={submit}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <h3 class={styles.popTitle}>
        {ref ? t("app.plan.editTitle") : t("app.plan.newTitle")}
      </h3>

      <label class={styles.field}>
        {t("app.setup.auto.day")}
        <Select
          value={String(day)}
          options={WEEKDAYS.map((d) => ({
            value: String(d),
            label: dayName(d),
          }))}
          onChange={(next) => setDay(Number(next) as Weekday)}
          testId="plan-editor-day"
        />
      </label>

      <div class={styles.fieldRow}>
        <label class={styles.field}>
          {t("app.setup.auto.start")}
          <input
            ref={startRef}
            type="time"
            class={styles.input}
            value={start}
            data-testid="plan-editor-start"
            onInput={(e) => setStart((e.target as HTMLInputElement).value)}
          />
        </label>
        <label class={styles.field}>
          {t("app.plan.stop")}
          <input
            type="time"
            class={styles.input}
            value={stop}
            data-testid="plan-editor-stop"
            onInput={(e) => setStop((e.target as HTMLInputElement).value)}
          />
        </label>
      </div>

      {placement ? (
        <p class={styles.popInfo} data-testid="plan-editor-info">
          {tf("app.setup.auto.minutes", { n: duration })}
          {crosses ? ` · ${t("app.plan.nextDay")}` : ""}
        </p>
      ) : null}
      {problem || error ? (
        <p class={styles.popError} role="alert" data-testid="plan-editor-error">
          {error ?? problem}
        </p>
      ) : null}

      <div class={styles.popActions}>
        <Button
          variant="primary"
          type="submit"
          disabled={placement === null || problem !== null}
          disabledReason={problem ?? t("app.plan.errTime")}
          testId="plan-editor-save"
        >
          {t("app.setup.save")}
        </Button>
        <Button
          variant="secondary"
          onClick={onClose}
          testId="plan-editor-cancel"
        >
          {t("app.setup.cancel")}
        </Button>
        <span class={styles.popSpacer} />
        {onDelete ? (
          <Button
            variant="danger"
            onClick={onDelete}
            testId="plan-editor-delete"
          >
            {t("app.plan.delete")}
          </Button>
        ) : null}
      </div>
    </form>
  );
}
