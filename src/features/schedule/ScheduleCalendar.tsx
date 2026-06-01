import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import type { ScheduleSlot } from "@/lib/bindings/ScheduleSlot";
import type { SpecialRecording } from "@/lib/bindings/SpecialRecording";

/** A marker shown on a single calendar day. */
export type DayMarkerKind = "weekly" | "special" | "churchtime";

export interface DayMarker {
  kind: DayMarkerKind;
  /** Short label for the pill (slot time or special name). */
  label: string;
}

export interface CalendarDay {
  /** 1-based day-of-month. */
  day: number;
  /** ISO `YYYY-MM-DD` for this cell. */
  iso: string;
  markers: DayMarker[];
}

/**
 * Backend weekday convention is 0 = Monday … 6 = Sunday, while JS
 * `Date.getDay()` is 0 = Sunday … 6 = Saturday. Convert JS → backend.
 */
export function jsDayToBackend(jsDay: number): number {
  return (jsDay + 6) % 7;
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

function isoFor(year: number, month: number, day: number): string {
  return `${year}-${pad2(month + 1)}-${pad2(day)}`;
}

/**
 * Pure mapping: given a calendar year + 0-based `month`, the weekly `slots`
 * and the dated `specials`, return one {@link CalendarDay} per day of that
 * month with its derived markers. No wall-clock dependency — fully testable
 * with fixed inputs.
 *
 * Marker rules:
 *  - `special`: a `SpecialRecording` whose `date` falls on this day.
 *  - `weekly`: a `ScheduleSlot` active on this weekday (backend 0=Mon).
 *  - `churchtime`: a weekly slot that lands on a Sunday (church service day),
 *    distinguished so the legend can colour Sundays differently.
 */
export function buildMonthMarkers(
  year: number,
  month: number,
  slots: ScheduleSlot[],
  specials: SpecialRecording[],
): CalendarDay[] {
  const daysInMonth = new Date(year, month + 1, 0).getDate();
  const days: CalendarDay[] = [];

  for (let day = 1; day <= daysInMonth; day++) {
    const iso = isoFor(year, month, day);
    const backendWeekday = jsDayToBackend(new Date(year, month, day).getDay());
    const markers: DayMarker[] = [];

    // Dated specials win first (rendered first / most prominent).
    for (const sp of specials) {
      if (sp.date === iso) {
        markers.push({
          kind: "special",
          label: sp.name || sp.start,
        });
      }
    }

    // Weekly recurring slots active on this weekday.
    for (const slot of slots) {
      if (slot.days.includes(backendWeekday)) {
        // Sunday (backend 6) church service → distinct "churchtime" colour.
        const kind: DayMarkerKind =
          backendWeekday === 6 ? "churchtime" : "weekly";
        markers.push({ kind, label: slot.start });
      }
    }

    days.push({ day, iso, markers });
  }

  return days;
}

/**
 * Pad the first week so day 1 lands under the correct Mon–Sun column.
 * Returns the number of empty leading cells for the month.
 */
export function leadingBlanks(year: number, month: number): number {
  return jsDayToBackend(new Date(year, month, 1).getDay());
}

const PILL_CLASS: Record<DayMarkerKind, string> = {
  weekly: "bg-accent/20 text-accent border border-accent/40",
  special: "bg-rose-500/20 text-rose-300 border border-rose-500/40",
  churchtime: "bg-sky-500/20 text-sky-300 border border-sky-500/40",
};

const WEEKDAY_HEADERS = [
  ["schedule.mon", "Ma"],
  ["schedule.tue", "Ti"],
  ["schedule.wed", "On"],
  ["schedule.thu", "To"],
  ["schedule.fri", "Fr"],
  ["schedule.sat", "Lø"],
  ["schedule.sun", "Sø"],
] as const;

const MONTH_KEYS = [
  ["schedule.month.jan", "Januar"],
  ["schedule.month.feb", "Februar"],
  ["schedule.month.mar", "Mars"],
  ["schedule.month.apr", "April"],
  ["schedule.month.may", "Mai"],
  ["schedule.month.jun", "Juni"],
  ["schedule.month.jul", "Juli"],
  ["schedule.month.aug", "August"],
  ["schedule.month.sep", "September"],
  ["schedule.month.oct", "Oktober"],
  ["schedule.month.nov", "November"],
  ["schedule.month.dec", "Desember"],
] as const;

export interface ScheduleCalendarProps {
  slots: ScheduleSlot[];
  specials: SpecialRecording[];
  /** Optional fixed start month (for deterministic tests). */
  initialYear?: number;
  /** 0-based initial month (for deterministic tests). */
  initialMonth?: number;
}

/**
 * Colour-coded month grid (Mon–Sun) for the schedule, mirroring the old
 * Electron calendar. Days with a recording show a coloured pill derived from
 * {@link buildMonthMarkers}. Read-only / navigational — editing stays in the
 * list UI on {@link SchedulePage}.
 */
export function ScheduleCalendar({
  slots,
  specials,
  initialYear,
  initialMonth,
}: ScheduleCalendarProps) {
  const { t } = useTranslation();
  const now = useMemo(() => new Date(), []);
  const [year, setYear] = useState(initialYear ?? now.getFullYear());
  const [month, setMonth] = useState(initialMonth ?? now.getMonth());

  const days = useMemo(
    () => buildMonthMarkers(year, month, slots, specials),
    [year, month, slots, specials],
  );
  const blanks = useMemo(() => leadingBlanks(year, month), [year, month]);

  const prev = () => {
    if (month === 0) {
      setMonth(11);
      setYear((y) => y - 1);
    } else {
      setMonth((m) => m - 1);
    }
  };
  const next = () => {
    if (month === 11) {
      setMonth(0);
      setYear((y) => y + 1);
    } else {
      setMonth((m) => m + 1);
    }
  };

  return (
    <section
      className="rounded-xl border border-border bg-surface p-4"
      data-testid="schedule-calendar"
    >
      <div className="mb-3 flex items-center justify-between">
        <button
          type="button"
          aria-label={t("schedule.prevMonth", "Forrige måned")}
          className="rounded-lg border border-border bg-surface2 px-2 py-1 text-sm text-text2 hover:bg-surface3"
          onClick={prev}
        >
          ‹
        </button>
        <h3
          className="text-sm font-medium text-text"
          data-testid="calendar-title"
        >
          {t(MONTH_KEYS[month][0], MONTH_KEYS[month][1])} {year}
        </h3>
        <button
          type="button"
          aria-label={t("schedule.nextMonth", "Neste måned")}
          className="rounded-lg border border-border bg-surface2 px-2 py-1 text-sm text-text2 hover:bg-surface3"
          onClick={next}
        >
          ›
        </button>
      </div>

      <div className="grid grid-cols-7 gap-1 text-center">
        {WEEKDAY_HEADERS.map(([key, fallback]) => (
          <div key={key} className="pb-1 text-xs font-medium text-text3">
            {t(key, fallback)}
          </div>
        ))}

        {Array.from({ length: blanks }).map((_, i) => (
          <div key={`blank-${i}`} aria-hidden="true" />
        ))}

        {days.map((d) => (
          <div
            key={d.iso}
            data-testid={`day-${d.iso}`}
            className="flex min-h-[3.5rem] flex-col gap-0.5 rounded-lg border border-border bg-surface2 p-1 text-left"
          >
            <span className="text-xs text-text2">{d.day}</span>
            {d.markers.map((m, mi) => (
              <span
                key={mi}
                data-marker-kind={m.kind}
                title={m.label}
                className={`truncate rounded px-1 text-[10px] leading-tight ${PILL_CLASS[m.kind]}`}
              >
                {m.label}
              </span>
            ))}
          </div>
        ))}
      </div>

      {/* Legend */}
      <div className="mt-3 flex flex-wrap gap-3 text-xs text-text3">
        <span className="flex items-center gap-1">
          <span className={`h-3 w-3 rounded ${PILL_CLASS.weekly}`} />
          {t("schedule.legendWeekly", "Ukentlig opptak")}
        </span>
        <span className="flex items-center gap-1">
          <span className={`h-3 w-3 rounded ${PILL_CLASS.special}`} />
          {t("schedule.legendSpecial", "Spesialopptak")}
        </span>
        <span className="flex items-center gap-1">
          <span className={`h-3 w-3 rounded ${PILL_CLASS.churchtime}`} />
          {t("schedule.legendChurch", "Kirketid")}
        </span>
      </div>
    </section>
  );
}
