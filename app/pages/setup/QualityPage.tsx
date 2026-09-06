/**
 * 3 — Hvilken kvalitet?
 *
 * Tre kort, ikke to velgere. Dagens app har fire formater OG tre bitrater, som
 * er tolv kombinasjoner en frivillig må rangere selv — og etikettene («MP3»,
 * «Kompakt», «Anbefalt», «Høyeste») er aldri oversatt til noen av de sju
 * språkene og sier ingenting om hva forskjellen betyr.
 *
 * Her er det tre svar med en begrunnelse hver, og hver av dem skriver BEGGE
 * innstillingene: `format` og — når det er MP3 — `bitrate`. Å la bitraten stå
 * urørt ville gitt en «God»-profil som egentlig er 128 kbps fordi noen satte
 * det i den gamle appen i fjor.
 *
 * ## Egendefinert er ikke en feil
 *
 * En lagret kombinasjon som ikke er ett av de tre (MP3 · 320, eller AAC fra en
 * eldre profil) får sitt eget kort ØVERST, valgt, med det den faktisk er som
 * tittel. Alternativet — å tegne «God» som valgt — ville betydd at skjermen
 * sier én ting og fila blir en annen, og at neste lagring stille flytter
 * brukeren dit uten at noen ba om det.
 */

import { t, tDyn, tf } from "../../i18n";
import { useSetting } from "../../settings/use-setting";
import { patchSettings, settings } from "../../state/settings";
import { Card } from "../../ui/Card/Card";
import { RadioCards, type RadioOption } from "../../ui/RadioCards/RadioCards";
import { Receipt } from "../../ui/Receipt/Receipt";
import { qualityIdFor, type QualityId } from "./decisions-core";
import styles from "./setup.module.css";
import { SubPage } from "./SubPage";

/** De tre kortene, og hva hvert av dem SKRIVER. */
const CARDS: ReadonlyArray<{
  id: QualityId;
  format: "mp3" | "flac" | "wav";
  bitrate: string | null;
  recommended?: boolean;
}> = [
  { id: "mp3", format: "mp3", bitrate: "256", recommended: true },
  { id: "flac", format: "flac", bitrate: null },
  { id: "wav", format: "wav", bitrate: null },
];

/** Verdien det egendefinerte kortet bærer. Kan aldri kollidere med de tre. */
const CUSTOM = "custom";

export function QualityPage() {
  const s = settings.value;
  const current = qualityIdFor(s);

  // `format` er nøkkelen kortet HETER etter, og den som bærer kvitteringen.
  // `bitrate` skrives i samme runde gjennom `patchSettings` før commit, slik at
  // ÉN lagring bærer begge — to `useSetting` ville gitt to skrivninger og et
  // vindu der basen har MP3 med FLAC-ens bitrate.
  const format = useSetting("format", { kind: "radio" });

  const options: RadioOption[] = CARDS.map((card) => ({
    value: card.id,
    // `tDyn` og ikke en template inne i `t()`: prefikset er en literal gaten
    // kan slå opp, og suffikset er den halvdelen ingen gate kan kjenne.
    title: tDyn("app.setup.quality", card.id),
    description: tDyn("app.setup.qDesc", card.id),
    recommended: card.recommended,
  }));

  if (!current) {
    options.unshift({
      value: CUSTOM,
      title: tf("app.setup.qualityCustom", {
        format: String(s.format ?? "mp3").toUpperCase(),
        bitrate: String(s.bitrate ?? ""),
      }),
      description: t("app.setup.qualityCustomDesc"),
      // Ikke valgbart: kortet er en BESKRIVELSE av det som står lagret, ikke
      // et valg noen kan ta. Å kunne «velge egendefinert» ville krevd at vi
      // fant på hvilken kombinasjon det skulle være.
      disabled: true,
    });
  }

  return (
    <SubPage lede={t("app.setup.qualityLede")} testId="setup-quality">
      <Card testId="quality-card">
        <RadioCards
          testId="quality-choices"
          value={current ?? CUSTOM}
          options={options}
          disabled={format.busy}
          onChange={(next) => {
            const card = CARDS.find((c) => c.id === next);
            if (!card) return;
            // Bitraten FØRST, i samme signal-oppdatering som resten: `commit`
            // sender hele vokabularet, så begge feltene krysser i én lagring.
            if (card.bitrate) patchSettings({ bitrate: card.bitrate });
            format.set(card.format);
          }}
        />
        <div class={styles.footer}>
          <Receipt state={format.receipt} testId="quality-receipt" />
        </div>
      </Card>
    </SubPage>
  );
}
