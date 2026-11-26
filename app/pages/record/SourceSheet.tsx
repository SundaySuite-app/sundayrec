/**
 * Lydarket — et ark fra høyre kant (440 px). Skallet (E3) eier glid, slør,
 * Esc og fokus; innholdet (`SheetSound`, `SheetMore`) er E4.
 *
 * Ark-kontrakten, som alle arkene i «Én flate» deler:
 *   - glir inn med `--ease-out` (`--dur-slow`), slør bak (`--scrim-sheet`),
 *   - Esc og trykk på sløret lukker,
 *   - fokus går inn i arket når det åpnes, fanger Tab mens det står, og
 *     tilbake til det som åpnet det når det lukkes.
 * Arket er alltid i treet (`inert` + `aria-hidden` når lukket), slik at
 * glidet går begge veier.
 */

import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";

import { t } from "../../i18n";
import { Icon } from "../../ui/Icon/Icon";
import styles from "./SourceSheet.module.css";

export interface SourceSheetProps {
  open: boolean;
  onClose: () => void;
  children?: ComponentChildren;
}

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function SourceSheet({ open, onClose, children }: SourceSheetProps) {
  // Innholdet står bare i treet mens arket er åpent (og glir ut): målerne i det
  // skal ikke holde VU-strømmen åpen for et ark ingen ser.
  const [mounted, setMounted] = useState(open);
  useEffect(() => {
    if (open) {
      setMounted(true);
      return;
    }
    const id = setTimeout(() => setMounted(false), 450);
    return () => clearTimeout(id);
  }, [open]);
  const sheet = useRef<HTMLElement | null>(null);
  const closeBtn = useRef<HTMLButtonElement | null>(null);
  const opener = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;
    opener.current = document.activeElement as HTMLElement | null;
    closeBtn.current?.focus();
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab" || !sheet.current) return;
      const items = [
        ...sheet.current.querySelectorAll<HTMLElement>(FOCUSABLE),
      ].filter((el) => el.offsetParent !== null);
      if (items.length === 0) return;
      const first = items[0]!;
      const last = items[items.length - 1]!;
      const active = document.activeElement;
      if (
        event.shiftKey &&
        (active === first || !sheet.current.contains(active))
      ) {
        event.preventDefault();
        last.focus();
      } else if (
        !event.shiftKey &&
        (active === last || !sheet.current.contains(active))
      ) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      opener.current?.focus?.();
    };
  }, [open]);

  return (
    <>
      {open ? (
        <button
          type="button"
          data-testid="sheet-scrim"
          aria-label={t("app.sheet.close")}
          tabIndex={-1}
          class={styles.scrim}
          onClick={onClose}
        />
      ) : null}
      <section
        ref={sheet}
        data-testid="source-sheet"
        data-open={open ? "true" : "false"}
        role="dialog"
        aria-modal="true"
        aria-label={t("app.sheet.title")}
        aria-hidden={open ? undefined : "true"}
        inert={!open}
        class={`${styles.sheet} ${open ? styles.open : ""}`}
      >
        <header class={styles.head}>
          <span class={styles.title}>
            <Icon name="sliders" size={20} />
            <span>{t("app.sheet.title")}</span>
          </span>
          <button
            ref={closeBtn}
            type="button"
            data-testid="sheet-close"
            aria-label={t("app.sheet.close")}
            title={t("app.sheet.close")}
            class={styles.close}
            onClick={onClose}
          >
            <Icon name="x" size={20} strokeWidth={2} />
          </button>
        </header>
        <div class={styles.body}>{mounted ? children : null}</div>
      </section>
    </>
  );
}
