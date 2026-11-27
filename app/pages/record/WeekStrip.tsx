/**
 * Ukestripa nederst på TA OPP — planen, uten egen side.
 *
 *   Uten faste tider og lukket: bare en liten kalenderknapp nede til venstre;
 *   det lille gullplusset sier «legg inn». Med tider men «Auto» av: den lukkede
 *   stripa, dempet, med «Automatisk opptak er av» — aldri den bare knappen.
 *
 *   Med plan, lukket: en 112 px stripe — mini-uke (sju dager, hver en liten
 *   tidslinje med gullmerker; neste opptak gløder), «Neste: søndag 11:00» med
 *   en linje under (starter av seg selv · om 2 timer, og den ærlige vekke-
 *   linja), fire statusikoner (gull når på) og en pil.
 *
 *   Åpen (520 px): glir opp med `--ease-out`, slør bak resten av flaten, Esc og
 *   trykk på sløret lukker. Til venstre de fire flisene (`StripTiles`), til
 *   høyre selve uka (`WeekView`, som eier dra/flytt/strekk).
 *
 * ## Åpen/lukket er RUTEN
 *
 * `route.panel === "strip"` er sannheten: `?goto=plan`, tray og lenker åpner
 * stripa ved å navigere, og kalenderknappen/pila/sløret navigerer tilbake.
 * Ingen lokal kopi som kan bli uenig med adressen.
 *
 * ## Hva «plan» betyr
 *
 * `autoRecordOn` (`schedule-core.ts`): flagget OG minst én tid. «Auto» av
 * sletter ingenting — tidene står (dempet) og kommer tilbake som de var, og
 * stripa sier «Automatisk opptak er av» i stedet for å late som det finnes et
 * neste opptak.
 *
 * ## Esc
 *
 * Stripa lytter i boblefasen på `document` og ser bort fra et Esc som allerede
 * er tatt: dra-gesten i uka, redigeringen og vekke-popoveren lukker SEG
 * (capture + `preventDefault`/`stopPropagation`), ikke stripa under.
 */

import { useEffect, useMemo, useState } from "preact/hooks";

import { formatWakeHint, intlParts } from "@lib/status/next-recording-core";
import { capitalizeFirst } from "@lib/ui/capitalize";

import { locale, t, tf, tn } from "../../i18n";
import { navigate, panel } from "../../router/router";
import { nextRecording } from "../../state/next-recording";
import { settings } from "../../state/settings";
import { Icon, type IconName } from "../../ui/Icon/Icon";
import {
  nextKey,
  occurrenceKey,
  occurrencesOf,
  segmentsOf,
  weekdayOf,
} from "../plan/plan-core";
import { autoRecordOn, WEEKDAYS, type Weekday } from "../plan/schedule-core";
import { WeekView } from "../plan/WeekView";
import { hintSpot } from "./first-run-hint";
import { StripTiles } from "./StripTiles";
import styles from "./WeekStrip.module.css";

/** Høyden på lukket stripe — CameraStage lar bildet slutte over den. */
export const STRIP_CLOSED_PX = 112;

export function WeekStrip() {
  const expanded = panel.value === "strip";
  // Første gang: kalenderknappen (eller stripas topp) pulserer gull mens
  // hintboblen «Faste opptak» peker på den.
  const spot = hintSpot.value === "plan" ? "true" : undefined;
  const s = settings.value;
  const planOn = autoRecordOn(s);
  const state = nextRecording.value;
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);

  // Innholdet bor i treet mens stripa er åpen — og mens den glir ned.
  const [mounted, setMounted] = useState(expanded);
  useEffect(() => {
    if (expanded) {
      setMounted(true);
      return;
    }
    const id = setTimeout(() => setMounted(false), 450);
    return () => clearTimeout(id);
  }, [expanded]);

  useEffect(() => {
    if (!expanded) return;
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      navigate("record");
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [expanded]);

  const open = () => navigate("record", { panel: "strip" });
  const toggle = () => navigate("record", expanded ? {} : { panel: "strip" });

  // Bare kalenderknappen når uka er TOM. Står tidene der men «Auto» er av, er
  // stripa fortsatt synlig (dempet, med «Automatisk opptak er av») — ellers
  // ville «av» og «ingen plan» sett nøyaktig likt ut.
  const hasSlots = (s.slots ?? []).length > 0;
  if (!hasSlots && !expanded) {
    return (
      <div data-testid="record-next-none" class={styles.dock}>
        <button
          type="button"
          data-testid="record-next-plan"
          aria-label={t("app.record.planAria")}
          title={t("app.record.planAria")}
          data-spot={spot}
          class={styles.pill}
          onClick={open}
        >
          <Icon name="calendar" size={24} />
          <span aria-hidden="true" class={styles.plus}>
            <Icon name="plus" size={12} strokeWidth={3} />
          </span>
        </button>
      </div>
    );
  }

  const slots = s.slots ?? [];
  const next = state.next;
  const parts = next ? intlParts(locale.value)(next.atMs) : null;
  const wake = formatWakeHint(state, {
    t,
    tf,
    tn,
    parts: intlParts(locale.value),
    nowMs: now,
  });

  let title: string;
  let sub: string;
  if (!planOn) {
    title = slots.length > 0 ? t("app.strip.autoOff") : t("app.strip.none");
    sub = slots.length > 0 ? t("app.strip.autoOffSub") : t("app.strip.noneSub");
  } else if (parts && next) {
    title = tf("app.strip.next", {
      day: capitalizeFirst(parts.weekdayLong, locale.value),
      time: parts.time,
    });
    const countdown = countdownText(next.atMs - now, locale.value);
    sub = countdown
      ? tf("app.strip.subAuto", { when: countdown })
      : t("app.strip.subAutoNow");
  } else {
    title = t("app.strip.none");
    sub = t("app.strip.subAutoNow");
  }

  const statuses: Array<{ icon: IconName; label: string; on: boolean }> = [
    {
      icon: "calendar-check",
      label: t("app.strip.tile.auto"),
      // Flagget OG minst én tid: «Auto» på i en tom uke er ikke «på».
      on: planOn,
    },
    {
      icon: "power",
      label: t("app.strip.tile.launch"),
      on: s.launchAtLogin === true,
    },
    {
      icon: "moon",
      label: t("app.strip.tile.wake"),
      on: s.wakeFromSleep !== false,
    },
    {
      icon: "bell",
      label: t("app.strip.tile.notify"),
      on: s.notifyStart === true || s.notifyStop === true,
    },
  ];

  return (
    <>
      {expanded ? (
        <button
          type="button"
          data-testid="strip-scrim"
          aria-label={t("app.sheet.close")}
          tabIndex={-1}
          class={styles.scrim}
          onClick={() => navigate("record")}
        />
      ) : null}
      <section
        data-testid={planOn ? "record-next-auto" : "record-next-none"}
        data-expanded={expanded ? "true" : "false"}
        aria-label={t("app.strip.title")}
        data-auto={planOn ? "on" : "off"}
        class={`${styles.strip} ${expanded ? styles.open : ""}`}
      >
        <button
          type="button"
          data-testid="record-next-plan"
          aria-expanded={expanded}
          aria-controls="strip-body"
          title={t("app.strip.title")}
          data-spot={spot}
          class={styles.head}
          onClick={toggle}
        >
          <MiniWeek nowMs={now} />
          <span class={styles.text}>
            <span data-testid="record-next-auto-when" class={styles.when}>
              {title}
            </span>
            <span data-testid="record-next-auto-sub" class={styles.sub}>
              {sub}
            </span>
            {planOn && wake ? (
              <span data-testid="record-next-auto-wake" class={styles.wake}>
                {wake}
              </span>
            ) : null}
          </span>
          <span class={styles.icons}>
            {statuses.map((x) => (
              <span
                key={x.icon}
                role="img"
                data-on={x.on ? "true" : "false"}
                aria-label={
                  x.on
                    ? tf("app.strip.statusOn", { what: x.label })
                    : tf("app.strip.statusOff", { what: x.label })
                }
                title={x.label}
                class={`${styles.status} ${x.on ? styles.statusOn : ""}`}
              >
                <Icon name={x.icon} size={16} strokeWidth={2} />
              </span>
            ))}
            <span
              aria-hidden="true"
              class={`${styles.chev} ${expanded ? styles.chevOpen : ""}`}
            >
              <Icon name="chevron-up" size={22} strokeWidth={2} />
            </span>
          </span>
        </button>

        <div id="strip-body" class={styles.body} hidden={!expanded && !mounted}>
          {mounted ? (
            <>
              <StripTiles />
              <WeekView />
            </>
          ) : null}
        </div>
      </section>
    </>
  );
}

// ── Mini-uka ────────────────────────────────────────────────────────────────

/** Mini-tidslinjens høyde (px) og luften over/under. */
const MINI_PX = 50;
const MINI_PAD = 4;

/** Korte ukedagsnavn, mandag først, i appens språk («Man», «Tir» …). */
function shortDays(loc: string): string[] {
  // 2024-01-01 var en mandag.
  const fmt = new Intl.DateTimeFormat(loc, { weekday: "short" });
  return WEEKDAYS.map((d) =>
    capitalizeFirst(
      fmt.format(new Date(2024, 0, 1 + d)).replace(/\.$/, ""),
      loc,
    ),
  );
}

function MiniWeek({ nowMs }: { nowMs: number }) {
  const s = settings.value;
  const slots = s.slots ?? [];
  const armed = s.autoRecordEnabled !== false;
  const nextAt = nextRecording.value.next?.at ?? null;
  const nextK = nextKey(nextAt);
  const days = useMemo(() => shortDays(locale.value), [locale.value]);
  const today: Weekday = weekdayOf(new Date(nowMs));

  const segments = useMemo(
    () => occurrencesOf(slots).flatMap(segmentsOf),
    [slots],
  );
  // Tidsaksen følger tidene: aldri trangere enn 06–22, men glir ut til natta
  // når en tid ligger der. Slik er 90 minutter alltid en synlig strek.
  const lo = Math.min(
    6 * 60,
    ...segments.map((g) => Math.floor(g.from / 60) * 60),
  );
  const hi = Math.max(
    22 * 60,
    ...segments.map((g) => Math.ceil(g.to / 60) * 60),
  );
  const scale = (MINI_PX - 2 * MINI_PAD) / (hi - lo);

  return (
    <span class={styles.mini} aria-hidden="true" data-testid="strip-mini">
      {WEEKDAYS.map((d) => (
        <span key={d} class={styles.miniDay}>
          <span
            class={`${styles.miniLabel} ${d === today ? styles.miniToday : ""}`}
          >
            {days[d]}
          </span>
          <span class={styles.miniLine}>
            {segments
              .filter((g) => g.day === d)
              .map((g) => {
                const isNext =
                  g.part !== "tail" &&
                  armed &&
                  nextK === occurrenceKey(g.occ.day, g.occ.start);
                return (
                  <span
                    key={g.key}
                    data-testid="strip-mini-mark"
                    data-next={isNext ? "true" : undefined}
                    data-off={armed ? undefined : "true"}
                    class={`${styles.mark} ${isNext ? styles.markNext : ""}`}
                    style={{
                      top: `${MINI_PAD + (g.from - lo) * scale}px`,
                      height: `${Math.max(4, (g.to - g.from) * scale)}px`,
                    }}
                  />
                );
              })}
          </span>
        </span>
      ))}
    </span>
  );
}

/**
 * «om 2 timer» / «om 12 minutter» / «om 3 dager», i appens språk. Intl gjør
 * ordene og bøyningen; vi velger bare enheten. Under et minutt eller bak oss er
 * det ingenting å telle ned: tom streng.
 */
export function countdownText(diffMs: number, loc: string): string {
  if (!(diffMs > 0)) return "";
  const minutes = Math.floor(diffMs / 60_000);
  const rtf = new Intl.RelativeTimeFormat(loc, { numeric: "always" });
  if (minutes < 1) return "";
  if (minutes < 60) return rtf.format(minutes, "minute");
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return rtf.format(hours, "hour");
  return rtf.format(Math.floor(hours / 24), "day");
}
