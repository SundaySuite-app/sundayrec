import { describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";

import {
  ScheduleCalendar,
  buildMonthMarkers,
  jsDayToBackend,
  leadingBlanks,
} from "./ScheduleCalendar";
import type { ScheduleSlot } from "@/lib/bindings/ScheduleSlot";
import type { SpecialRecording } from "@/lib/bindings/SpecialRecording";

// Mock i18n so t("key", "fallback") returns the fallback verbatim.
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, fallback?: string) => fallback ?? _key,
  }),
}));

// A Sunday weekly slot (backend 6 = Sunday) at 11:00.
const SUNDAY_SLOT: ScheduleSlot = {
  days: [6],
  start: "11:00",
  stop: "12:00",
  max: null,
};

// A Wednesday weekly slot (backend 2 = Wednesday) at 19:00.
const WED_SLOT: ScheduleSlot = {
  days: [2],
  start: "19:00",
  stop: "20:00",
  max: null,
};

// A dated special on 2026-06-07 (which is itself a Sunday).
const SPECIAL: SpecialRecording = {
  id: "s1",
  date: "2026-06-07",
  name: "Konfirmasjon",
  start: "11:00",
  stop: "13:00",
  deviceId: null,
};

describe("jsDayToBackend", () => {
  it("maps JS Sunday(0) → backend 6 and JS Monday(1) → backend 0", () => {
    expect(jsDayToBackend(0)).toBe(6); // Sunday
    expect(jsDayToBackend(1)).toBe(0); // Monday
    expect(jsDayToBackend(6)).toBe(5); // Saturday
  });
});

describe("buildMonthMarkers", () => {
  // June 2026 (month index 5). 2026-06-01 is a Monday.
  it("marks every Sunday in the month with a churchtime pill from a Sunday slot", () => {
    const days = buildMonthMarkers(2026, 5, [SUNDAY_SLOT], []);
    // Sundays in June 2026: 7, 14, 21, 28.
    const sundays = [7, 14, 21, 28];
    for (const dnum of sundays) {
      const cell = days.find((d) => d.day === dnum)!;
      expect(cell.markers).toHaveLength(1);
      expect(cell.markers[0].kind).toBe("churchtime");
      expect(cell.markers[0].label).toBe("11:00");
    }
    // A Monday (day 1) has no markers.
    expect(days.find((d) => d.day === 1)!.markers).toHaveLength(0);
  });

  it("marks a non-Sunday weekly slot as 'weekly'", () => {
    const days = buildMonthMarkers(2026, 5, [WED_SLOT], []);
    // Wednesdays in June 2026: 3, 10, 17, 24.
    const wed = days.find((d) => d.day === 3)!;
    expect(wed.markers).toHaveLength(1);
    expect(wed.markers[0].kind).toBe("weekly");
    expect(wed.markers[0].label).toBe("19:00");
  });

  it("marks a dated special on its exact day, alongside an overlapping slot", () => {
    const days = buildMonthMarkers(2026, 5, [SUNDAY_SLOT], [SPECIAL]);
    // 2026-06-07 is a Sunday with both a special and the Sunday slot.
    const cell = days.find((d) => d.iso === "2026-06-07")!;
    const kinds = cell.markers.map((m) => m.kind);
    expect(kinds).toContain("special");
    expect(kinds).toContain("churchtime");
    const special = cell.markers.find((m) => m.kind === "special")!;
    expect(special.label).toBe("Konfirmasjon");
    // Another Sunday without a special only has the churchtime marker.
    const plain = days.find((d) => d.iso === "2026-06-14")!;
    expect(plain.markers.map((m) => m.kind)).toEqual(["churchtime"]);
  });

  it("returns one entry per day of the month", () => {
    // June has 30 days; February 2026 has 28.
    expect(buildMonthMarkers(2026, 5, [], [])).toHaveLength(30);
    expect(buildMonthMarkers(2026, 1, [], [])).toHaveLength(28);
  });
});

describe("leadingBlanks", () => {
  it("computes Mon–Sun column offset for the 1st of the month", () => {
    // 2026-06-01 is a Monday → 0 leading blanks.
    expect(leadingBlanks(2026, 5)).toBe(0);
    // 2026-07-01 is a Wednesday → 2 leading blanks.
    expect(leadingBlanks(2026, 6)).toBe(2);
  });
});

describe("ScheduleCalendar component", () => {
  it("renders the fixed month title, legend and coloured day markers", () => {
    render(
      <ScheduleCalendar
        slots={[SUNDAY_SLOT]}
        specials={[SPECIAL]}
        initialYear={2026}
        initialMonth={5}
      />,
    );

    expect(screen.getByTestId("calendar-title").textContent).toContain(
      "Juni 2026",
    );

    // The 7th carries both a special and a churchtime pill.
    const cell = screen.getByTestId("day-2026-06-07");
    const special = within(cell).getByText("Konfirmasjon");
    expect(special.getAttribute("data-marker-kind")).toBe("special");
    expect(
      within(cell).getAllByText("11:00")[0].getAttribute("data-marker-kind"),
    ).toBe("churchtime");

    // Legend entries present.
    expect(screen.getByText("Ukentlig opptak")).toBeTruthy();
    expect(screen.getByText("Spesialopptak")).toBeTruthy();
    expect(screen.getByText("Kirketid")).toBeTruthy();
  });
});
