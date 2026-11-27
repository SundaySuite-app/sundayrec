/**
 * Flisene i ukestripa: Auto, Oppstart, Vekk og Varsel — et 2×2-rutenett.
 *
 * Hver flis er en bryter (`role="switch"`), og bak hver ligger nøyaktig den
 * innstillingen den hadde som rad på de gamle sidene:
 *
 *   Auto      `autoRecordEnabled` — bare flagget skrives. Av/på sletter
 *             INGENTING: tidene blir stående i uka (dempet) og kommer tilbake
 *             som de var. Flisen LYSER bare med flagget OG minst én tid
 *             (`autoRecordOn`); på i en tom uke står «Ingen tider» under, og
 *             ingen tid fantaseres fram.
 *   Oppstart  `launchAtLogin` — meningsløs alene, den er svaret på «skjer det
 *             planlagte opptaket også etter en omstart?». OS-et er fasit: er
 *             det uenig med det lagrede, vinner OS-et (`syncLaunchAtLoginFromOs`).
 *   Vekk      `wakeFromSleep`. Hjørneknappen åpner `WakePopover` (prøv vekking,
 *             logg).
 *   Varsel    `notifyStart` + `notifyStop` (én bryter, to nøkler). Hjørneknappen
 *             åpner `NotifyPopover` (status fra OS-et, «Send testvarsel»,
 *             «Åpne systeminnstillinger»). Verdien under
 *             — `reminderMinutes` — trykkes for å bla gjennom forhåndsvalgene.
 *             Slås varsler PÅ uten at OS-et viser dem, sier stripa fra (og
 *             tilbyr å åpne systeminnstillingene) — feilvarslene er det eneste
 *             som når den som står ved maskinen.
 *
 * Skriving går gjennom `useSetting`/`usePatch` (samme lagring, tilbakerulling og
 * feiltoast som resten av appen); ingen ny lagring.
 */

import type { ComponentChildren } from "preact";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";

import { t, tf } from "../../i18n";
import { usePatch } from "../../settings/use-patch";
import { useSetting } from "../../settings/use-setting";
import {
  notificationPermission,
  refreshNotificationPermission,
} from "../../state/notification-permission";
import { settings, syncLaunchAtLoginFromOs } from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { Icon, type IconName } from "../../ui/Icon/Icon";
import { toast } from "../../ui/toast";
import { autoRecordOn } from "../plan/schedule-core";
import { NotifyPopover } from "./NotifyPopover";
import { WakePopover } from "./WakePopover";
import styles from "./StripTiles.module.css";

/** Minuttene «påminnelse før opptak» blar gjennom. 0 = ingen påminnelse. */
export const REMINDER_CHOICES: readonly number[] = [0, 5, 10, 15, 30, 60];

/** Neste forhåndsvalg etter `current`; en verdi utenfor lista starter på nytt. */
export function nextReminder(current: number): number {
  const i = REMINDER_CHOICES.indexOf(current);
  return REMINDER_CHOICES[(i + 1) % REMINDER_CHOICES.length] ?? 0;
}

interface TileProps {
  icon: IconName;
  label: string;
  title: string;
  on: boolean;
  busy?: boolean;
  onToggle: () => void;
  testId: string;
  children?: ComponentChildren;
}

function Tile({
  icon,
  label,
  title,
  on,
  busy,
  onToggle,
  testId,
  children,
}: TileProps) {
  return (
    <div class={styles.cell}>
      <button
        type="button"
        role="switch"
        aria-checked={on}
        aria-label={title}
        title={title}
        data-testid={testId}
        data-on={on ? "true" : "false"}
        aria-busy={busy ? "true" : undefined}
        class={`${styles.tile} ${on ? styles.on : ""}`}
        onClick={() => {
          if (!busy) onToggle();
        }}
      >
        <Icon name={icon} size={24} />
        <span class={styles.word}>{label}</span>
      </button>
      {children}
    </div>
  );
}

export function StripTiles() {
  const s = settings.value;
  const autoPatch = usePatch();
  const notifyPatch = usePatch();
  const launch = useSetting("launchAtLogin", { kind: "toggle" });
  const wake = useSetting("wakeFromSleep", { kind: "toggle" });
  const reminder = useSetting("reminderMinutes", { kind: "select" });
  const [wakeOpen, setWakeOpen] = useState(false);
  const [notifyOpen, setNotifyOpen] = useState(false);

  useEffect(() => {
    void syncLaunchAtLoginFromOs();
    void refreshNotificationPermission();
  }, []);

  // Flagget er det som skrives; «lyser» gjør det først med minst én tid —
  // «Auto» på i en tom uke er en bryter som lover noe uten å ha noe å love.
  const autoFlag = s.autoRecordEnabled !== false;
  const autoOn = autoRecordOn(s);
  const needsTimes = autoFlag && !autoOn;
  const launchOn = s.launchAtLogin === true;
  const wakeOn = s.wakeFromSleep !== false;
  const notifyOn = s.notifyStart === true || s.notifyStop === true;
  const reminderNow =
    typeof s.reminderMinutes === "number" ? s.reminderMinutes : 0;
  const permission = notificationPermission.value;

  async function toggleNotify(): Promise<void> {
    const next = !notifyOn;
    const saved = await notifyPatch.write({
      notifyStart: next,
      notifyStop: next,
    });
    if (!saved || !next) return;
    // Slått PÅ: har OS-et varslene av, eller er maskinen aldri spurt? Spør nå
    // (macOS-dialogen kommer da, ikke midt i første feilvarsel); er de slått
    // av i systemet, står setningen under flisene.
    try {
      notificationPermission.value =
        await window.api.notificationRequestPermission();
    } catch (err) {
      console.warn("[strip] notification permission:", err);
    }
  }

  async function openSystemSettings(): Promise<void> {
    if (!(await window.api.notificationOpenSettings())) {
      toast("error", t("app.setup.notify.openSettingsFailed"));
    }
  }

  return (
    <div class={styles.wrap}>
      <div class={styles.grid} data-testid="strip-tiles">
        <Tile
          icon="calendar-check"
          label={t("app.strip.tile.auto")}
          title={
            needsTimes
              ? t("app.strip.autoNoTimesTitle")
              : t("app.strip.tile.autoTitle")
          }
          on={autoOn}
          busy={autoPatch.busy}
          onToggle={() =>
            void autoPatch.write({ autoRecordEnabled: !autoFlag })
          }
          testId="strip-tile-auto"
        >
          {needsTimes ? (
            <span
              class={styles.hint}
              data-testid="strip-auto-needs-times"
              title={t("app.strip.autoNoTimesTitle")}
            >
              {t("app.strip.autoNoTimes")}
            </span>
          ) : null}
        </Tile>
        <Tile
          icon="power"
          label={t("app.strip.tile.launch")}
          title={t("app.strip.tile.launchTitle")}
          on={launchOn}
          busy={launch.busy}
          onToggle={() => launch.set(!launchOn)}
          testId="strip-tile-launch"
        />
        <Tile
          icon="moon"
          label={t("app.strip.tile.wake")}
          title={t("app.strip.tile.wakeTitle")}
          on={wakeOn}
          busy={wake.busy}
          onToggle={() => wake.set(!wakeOn)}
          testId="strip-tile-wake"
        >
          <PopCorner
            open={wakeOpen}
            onOpen={setWakeOpen}
            testId="strip-wake-more"
            label={t("app.strip.wakeMore")}
          >
            <WakePopover />
          </PopCorner>
        </Tile>
        <Tile
          icon="bell"
          label={t("app.strip.tile.notify")}
          title={t("app.strip.tile.notifyTitle")}
          on={notifyOn}
          busy={notifyPatch.busy}
          onToggle={() => void toggleNotify()}
          testId="strip-tile-notify"
        >
          <PopCorner
            open={notifyOpen}
            onOpen={setNotifyOpen}
            testId="strip-notify-more"
            label={t("app.strip.notifyMore")}
          >
            <NotifyPopover />
          </PopCorner>
          <button
            type="button"
            class={styles.value}
            data-testid="strip-notify-value"
            title={t("app.sheet.change")}
            disabled={reminder.busy}
            onClick={() => reminder.set(nextReminder(reminderNow))}
          >
            {reminderNow === 0
              ? t("app.strip.noReminder")
              : tf("app.setup.notify.minutes", { n: reminderNow })}
          </button>
        </Tile>
      </div>

      {notifyOn && permission === "denied" ? (
        <div
          class={styles.notice}
          role="status"
          data-testid="strip-notify-denied"
        >
          <span>{t("app.setup.notify.permissionDenied")}</span>
          <Button
            variant="secondary"
            testId="strip-notify-open-settings"
            onClick={() => void openSystemSettings()}
          >
            {t("app.setup.notify.openSettings")}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

/**
 * Hjørneknappen på en flis og popoveren den åpner (Vekk: `WakePopover`; Varsel:
 * `NotifyPopover`). Popoveren er `position: fixed` og plasseres over knappen ved
 * åpning: stripa klipper innholdet sitt (`overflow: hidden` for glidet), og en
 * popover inne i det ville blitt kuttet.
 */
function PopCorner({
  open,
  onOpen,
  testId,
  label,
  icon = "sliders",
  children,
}: {
  open: boolean;
  onOpen: (open: boolean) => void;
  testId: string;
  label: string;
  icon?: IconName;
  children: ComponentChildren;
}) {
  const btn = useRef<HTMLButtonElement | null>(null);
  const holder = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState<{ left: number; bottom: number; max: number }>(
    { left: 0, bottom: 0, max: 380 },
  );

  useLayoutEffect(() => {
    if (!open || !btn.current) return;
    const r = btn.current.getBoundingClientRect();
    const left = Math.max(8, Math.min(r.left, window.innerWidth - 348));
    setPos({
      left,
      bottom: window.innerHeight - r.top + 8,
      max: Math.max(160, r.top - 16),
    });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    // Esc lukker bare popoveren (capture + preventDefault: stripa rundt
    // lukker seg ikke samtidig); trykk utenfor lukker den også.
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      onOpen(false);
      btn.current?.focus();
    };
    const onDown = (e: PointerEvent): void => {
      const target = e.target as Node | null;
      if (
        target &&
        (holder.current?.contains(target) || btn.current?.contains(target))
      ) {
        return;
      }
      onOpen(false);
    };
    document.addEventListener("keydown", onKey, true);
    document.addEventListener("pointerdown", onDown, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      document.removeEventListener("pointerdown", onDown, true);
    };
  }, [open, onOpen]);

  return (
    <>
      <button
        ref={btn}
        type="button"
        class={styles.corner}
        data-testid={testId}
        aria-label={label}
        title={label}
        aria-expanded={open}
        onClick={() => onOpen(!open)}
      >
        <Icon name={icon} size={14} />
      </button>
      {open ? (
        <div
          ref={holder}
          class={styles.popHolder}
          style={{
            left: `${pos.left}px`,
            bottom: `${pos.bottom}px`,
            maxHeight: `${pos.max}px`,
          }}
        >
          {children}
        </div>
      ) : null}
    </>
  );
}
