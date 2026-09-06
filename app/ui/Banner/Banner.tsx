/**
 * Banner — den brede stripen øverst på en side, for det som ikke kan vente.
 *
 * To toner, og bare to (canvasens sett 7):
 *
 *   `bad`  — noe gikk TAPT, og setningen sier når. «Opptaket ble avbrutt
 *            kl. 11:42» — tidspunktet er halve informasjonen.
 *   `warn` — noe trenger deg FØR søndag. Ingenting er ødelagt ennå.
 *
 * Ingen `info`-tone. En blå stripe som ikke krever noe blir tapetet folk slutter
 * å lese, og da forsvinner de to som betyr noe sammen med den. Ting som bare
 * er verdt å nevne er en toast.
 *
 * `role="alert"` for `bad` (avbryter — noe er tapt), `role="status"` for
 * `warn` (venter på tur — det haster ikke i sekunder).
 *
 * ## `detail` kan bli langt (F1-P1)
 *
 * Hver `detail` Banner viste før var ÉN linje — en enhet, en feilkode, en
 * disk-setning. Releasenotatet (`state/banners.ts`s `update`-oppføring) er
 * det første som kan være et helt avsnitt, med linjeskift forfatteren la inn
 * med vilje (se `docs/release-notes/README.md`). Derfor:
 *
 *   - `white-space: pre-wrap` på `.detail` — linjeskiftene i teksten bevares
 *     i stedet for å kollapse til ett mellomrom. Trygt for alle de andre
 *     `detail`-kallerne også: ingen av dem har `\n` i teksten sin, så
 *     `pre-wrap` gjengir dem bit for bit likt `normal`.
 *   - klippet til 5 linjer med en «Vis mer» når teksten faktisk er lengre —
 *     MÅLT (skjermens `scrollHeight` mot den klipte `clientHeight`), ikke
 *     gjettet på tegnantall: en lang setning uten et eneste linjeskift
 *     brytes likevel over flere linjer ved en smal bredde, og en kort
 *     flerlinjers tekst kan holde seg under fem. Klippingen er ren CSS —
 *     HELE teksten står i DOM-en hele tiden, så «Kopier»-knappen
 *     (`GlobalErrorBanner`) og en test som leser `textContent` ser alt,
 *     uansett om noen har trykket «Vis mer».
 */

import type { ComponentChildren } from "preact";
import { useEffect, useId, useRef, useState } from "preact/hooks";

import { t } from "../../i18n";
import { Button } from "../Button/Button";
import styles from "./Banner.module.css";

export interface BannerProps {
  tone: "bad" | "warn";
  title: string;
  /** Detaljen: hva som skjedde, når, og hva som er lagret. */
  detail?: string;
  /** Knapper til høyre. */
  actions?: ComponentChildren;
  /** Lukkekryss. Utelates når stripen ikke skal kunne avvises. */
  onDismiss?: () => void;
  testId?: string;
}

export function Banner({
  tone,
  title,
  detail,
  actions,
  onDismiss,
  testId,
}: BannerProps) {
  return (
    <div
      role={tone === "bad" ? "alert" : "status"}
      data-tone={tone}
      data-testid={testId}
      class={`${styles.banner} ${tone === "bad" ? styles.bad : styles.warn}`}
    >
      <div class={styles.text}>
        <div class={styles.title}>{title}</div>
        {detail ? <BannerDetail text={detail} testId={testId} /> : null}
      </div>
      <div class={styles.actions}>
        {actions}
        {onDismiss ? (
          <Button
            variant="ghost"
            testId={testId ? `${testId}-dismiss` : undefined}
            onClick={onDismiss}
          >
            {t("app.common.dismiss")}
          </Button>
        ) : null}
      </div>
    </div>
  );
}

/** `detail`, klippet til 5 linjer med en «Vis mer» når det er noe å vise mer
 *  av. Se filhodet over for hvorfor målt og ikke gjettet. */
function BannerDetail({ text, testId }: { text: string; testId?: string }) {
  const bodyId = useId();
  const ref = useRef<HTMLDivElement | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [clampable, setClampable] = useState(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // Målt med klippingen PÅSLÅTT (mount starter alltid kollapset): er den
    // ekte høyden større enn den klipte, er det noe «Vis mer» faktisk viser.
    // JSDOM (vitest) har ikke ekte layout og svarer alltid 0/0 her — denne
    // grenen beviser seg selv bare i en ekte nettleser, altså e2e.
    setClampable(el.scrollHeight > el.clientHeight + 1);
  }, [text]);

  return (
    <>
      <div
        ref={ref}
        id={bodyId}
        data-testid={testId ? `${testId}-detail` : undefined}
        class={`${styles.detail} ${!expanded ? styles.detailClamped : ""}`}
      >
        {text}
      </div>
      {clampable ? (
        <Button
          variant="ghost"
          size="md"
          testId={testId ? `${testId}-more` : undefined}
          expanded={expanded}
          controls={bodyId}
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? t("app.common.showLess") : t("app.common.showMore")}
        </Button>
      ) : null}
    </>
  );
}
