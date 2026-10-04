/**
 * «Vis i Finder»/«Vis i Utforsker» — og loggradens søsken, «Vis».
 *
 * Fire knapper gjør nøyaktig det samme: spør en bakend-kommando som svarer
 * `true`/`false`, og si fra når svaret er `false`. En vellykket åpning sier
 * INGENTING — Finder/Utforsker åpner seg foran deg, og en toast oppå det hadde
 * vært å fortelle noen om noe de allerede ser. En FEILET åpning MÅ si fra, for
 * da skjedde det ingenting synlig, og en knapp som stille ikke gjør noe er
 * verre enn ingen knapp.
 *
 * ## Hva som var duplisert, og hva som ikke var det (F1-R2 / R10)
 *
 * `RecordPage` og `LibraryPage` bar denne funksjonen ordrett, kopiert.
 * `ExportPage`s kvittering hadde IKKE den: knappen kastet svaret fra
 * `window.api.revealFile` og sa aldri fra ved feil — den stille knappen R10
 * fant. `MaintenanceRows`s loggrad gjør samme FORM mot en annen kommando
 * (`logs_reveal`, som med vilje ikke tar en sti — se
 * `src-tauri/src/commands/logs.rs`s filhode for hvorfor: «det er ingenting for
 * renderen å navngi, så det er ingenting for en kompromittert webview å peke et
 * annet sted») og sin egen feiltekst. Den kan derfor ikke ringe `reveal(path)`
 * — `revealResult` er formen alle fire deler; `reveal(path)` er den ene,
 * vanlige spesialiseringen tre av de fire stedene faktisk trenger.
 *
 * ## Hva bakenden godtar
 *
 * `window.api.revealRecording`/`revealExport` er Rust-kommandoene
 * `recordings_reveal` og `recordings_reveal_export`, ikke opener-pluginen:
 * webviewet har ingen `opener:`-tillatelse. Ingen av dem tar en STI (B-
 * familien, PR-D): det første tar en historikkrads id og Rust slår opp fila i
 * databasen; det andre tar lappen eksportens svar bar. Alt bakenden ikke
 * kjenner, og alt som ikke finnes, er `false` — og dermed den samme toasten
 * her.
 *
 * ## Hvorfor `app/ui/`, ikke `app/lib/ui/`
 *
 * `app/lib/` er den porterte inventaren: den har et HÅNDHEVET ESLint-forbud
 * mot å importere fra skallet rundt seg (`no-restricted-imports`, se
 * `eslint.config.*`), nettopp fordi inventaren skal forbli ren og tas i bruk
 * fil for fil. Denne fila MÅ toaste og MÅ oversette — det er hele poenget —
 * så den bor ved siden av `toast.ts` og `dialog.ts`, de andre delte
 * SKALL-hjelperne, ikke i inventaren.
 */

import { t } from "../i18n";
import { toast } from "./toast";

/**
 * Si fra når en bakend-kommando som svarer `true`/`false` feilet.
 *
 * `failedMessage` er allerede OVERSATT tekst, ikke en katalognøkkel:
 * `scripts/check-i18n-keys.mjs` krever at hvert `t()`-kall i `app/` bruker en
 * literal streng, så nøkkelvalget må stå ved KALLSTEDET (`t("app.done.
 * revealFailed")` i `reveal` under; loggraden i `MaintenanceRows` sender sin
 * egen). En videresendt variabel ville vært nøyaktig den formen gaten finnes
 * for å nekte.
 */
export async function revealResult(
  ok: boolean,
  failedMessage: string,
): Promise<void> {
  if (!ok) toast("error", failedMessage);
}

/**
 * «Vis i Finder» for ÉT OPPTAK i historikken — radens id, aldri en sti. `null`/
 * tom id = ingenting å vise, og — som før — ingen feil å si fra om: et kort
 * uten et opptak ennå skal kunne rendre uten at det er noe å avsløre.
 */
export async function reveal(recordingId: string | null): Promise<void> {
  if (!recordingId) return;
  await revealResult(
    await window.api.revealRecording(recordingId),
    t("app.done.revealFailed"),
  );
}

/**
 * «Vis i Finder» på eksport-kvitteringen — lappen Rust la i eksportens svar.
 * `null` = fila fikk ingen lapp (den lot seg ikke sjekke), og da er det ikke
 * noe å vise.
 */
export async function revealExport(token: string | null): Promise<void> {
  if (!token) return;
  await revealResult(
    await window.api.revealExport(token),
    t("app.done.revealFailed"),
  );
}
