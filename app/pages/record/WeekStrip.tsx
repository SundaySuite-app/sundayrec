/**
 * Ukestripa nederst på TA OPP — plassholder til E5.
 *
 * Uten plan: en liten kalenderknapp nede til venstre (plusset sier «legg
 * inn»). Med plan: en kompakt linje om neste opptak (+ vekkehintet). Begge
 * åpner planen (`panel: "strip"`). E5 bygger den ekte stripa med mini-uke,
 * nedtelling og ark.
 */

import { formatWakeHint, intlParts } from "@lib/status/next-recording-core";
import { capitalizeFirst } from "@lib/ui/capitalize";

import { locale, t, tf, tn } from "../../i18n";
import { navigate } from "../../router/router";
import { nextRecording } from "../../state/next-recording";
import { Icon } from "../../ui/Icon/Icon";
import styles from "./WeekStrip.module.css";

export function WeekStrip() {
  const state = nextRecording.value;
  const next = state.next;
  const open = () => navigate("record", { panel: "strip" });

  if (!next) {
    return (
      <div data-testid="record-next-none" class={styles.dock}>
        <button
          type="button"
          data-testid="record-next-plan"
          aria-label={t("app.record.planAria")}
          title={t("app.record.planAria")}
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

  const parts = intlParts(locale.value)(next.atMs);
  const wake = formatWakeHint(state, {
    t,
    tf,
    tn,
    parts: intlParts(locale.value),
    nowMs: Date.now(),
  });

  return (
    <div data-testid="record-next-auto" class={styles.dock}>
      <button
        type="button"
        data-testid="record-next-plan"
        aria-label={t("app.record.planAria")}
        class={styles.line}
        onClick={open}
      >
        <Icon name="calendar-check" size={22} />
        <span class={styles.text}>
          <span data-testid="record-next-auto-when" class={styles.when}>
            {tf("app.record.nextAutoWhen", {
              day: capitalizeFirst(parts.weekdayLong, locale.value),
              time: parts.time,
            })}
          </span>
          {wake ? (
            <span data-testid="record-next-auto-wake" class={styles.wake}>
              {wake}
            </span>
          ) : null}
        </span>
      </button>
    </div>
  );
}
