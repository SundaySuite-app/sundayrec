# Audit-backlog — 2026-07-07

Gjenværende funn fra full-auditen 2026-07-07 (Rust-backend, frontend,
prosjekthelse) som IKKE ble tatt i forbedringsrunden på branchen
`claude/sundayrec-audit-improvements-0dy4fa`. Det som BLE fikset der: editor-IPC
path-scoping, panic-frie fallbacks i core-recorder, `LazyLock`-regexer i wake,
join-handle-håndtering, IPC-lytter-guards + global feilhåndtering + locale-
paritet i rendereren, vitest/version-sync/audit i CI, reell bindings-driftsjekk,
sjekksum-rammeverk for ffmpeg/ASIO, ts-rs-bindings-adopsjon
(`legacy/bindings/` + re-eksporter), README/docs-opprydding.

## Sikkerhet / release-bootstrap

1. **Pin sjekksummene.** `scripts/ffmpeg-checksums.json` er tom og ASIO-pinnen
   i `release.yml` er `""` — begge mekanismene er på plass, men verdiene må
   fylles fra en betrodd kjøring (kjør `npm run ffmpeg` på hver
   release-plattform og kopiér hashen; ASIO-steget printer zip-hashen i
   Actions-loggen). Før pinning er verifiseringen kun varslende.
2. **`opener:allow-open-path`-capability** (`src-tauri/capabilities/default.json`)
   lar en kompromittert webview åpne vilkårlige lokale stier. Vurder å scope
   eller erstatte med en kommando som kun åpner lagringsmappen/opptak.
3. **Path-scoping på øvrige IPC-flater.** `commands/editor.rs` er nå guardet
   (`commands/path_guard.rs`); samme mønster kan gjenbrukes for andre
   kommandoer som tar stier fra rendereren (media/whisper/publish).

## ts-rs-bindings — neste steg

4. **Rust-side optionality-fiks for drifted typer.** Disse er IKKE
   re-eksportert fra `legacy/bindings` fordi generert form avviker fra
   håndskrevet (`?` vs `| null`, manglende felter):
   - `WakeFailureEntry` (`reason`/`deltaSec`: `| null` vs `?`) — kandidater for
     `#[ts(optional)]` i `core/wake.rs`.
   - `ScheduleSlot` (`max`), `SpecialRecording` (`id`/`deviceId`) — samme.
   - `EpisodePrep` — generert mangler `publishYoutube`/`publishedAt`/
     `recordingTimestamp` og har `| null` i stedet for `?`. Avklar hvilken side
     som er sannheten.
   - `OverlayConfig` — generert mangler `chromaKey`/`crop` og bruker
     `OverlaySource`-objekt i stedet for `type` + `source`-strenger.
   - `IntegrationSettings` — generert `PeerToggle` mister `manifestFolder`/
     `autoSubmitUsage`/`autoSchedule`.
   - `RecordingOpts` — håndskrevet `extends Partial<Settings>`; generert er en
     annen (mindre) form.
     Etter hvert som Rust-typene justeres kan re-eksportlisten øverst i
     `legacy/types/index.ts` utvides.
5. **Typegenerér `window.api`.** Den ~200-linjers ambiente
   `Window['api']`-deklarasjonen i `legacy/renderer/main.ts:38-245` er
   håndskrevet; bygg signaturene på bindings-typene så IPC-flaten også er
   driftsikret.
6. **Slett resten av `reference/`** (~60 filer React-redesign-kode).
   `reference/bindings/` er allerede fjernet (erstattet av `legacy/bindings/`);
   resten importeres ikke av noe og er ekskludert fra tsconfig/eslint/prettier.

## Frontend

7. **~50 tause stub-metoder i `api-shim.ts`.** Rundt en tredjedel av de 149
   API-metodene er hardkodede tomme stubs (`=> []`, `=> null`, `=> false`) uten
   backend-kall — bl.a. `getLogs`, `cloudListFolders`, `overlayListScreens`,
   `transcriptListAll`, `planFetchServices`. UI-et ser «tomt» ut i stedet for
   «ikke tilgjengelig». Merk dem (headeren lover `// TODO Phase 3`-markører som
   ikke finnes), og la panelene vise en «ikke innebygd»-hint der det passer.
8. **`call()`-innpakningen i `api-shim.ts:82` svelger alle backend-feil** til
   en fallback med kun `console.warn` — kombinert med punkt 7 er backend-feil
   uskillelige fra tom tilstand. Vurder en synlig feilkanal (f.eks.
   `backend-warning`-toasten).
9. **`state.ts:3` settings-singleton** (`export let settings = {} as Settings`)
   lyver om typen før `loadSettings()`; tidlige lesere ser et tomt objekt.
10. **Tester for ren frontend-logikk:** `shared/church-calendar.ts`
    (liturgisk datomatte), `i18n.ts` (`t`/`tArr`/fallback),
    api-shim-adapterne (`RecordingRow → RecordingEntry`),
    `editor/detection.ts`. Vitest-oppsettet finnes allerede.
11. **Én `setInterval` uten clear** (15 `setInterval` vs 14 `clearInterval`) —
    verifiser hvilken.

## Rust

12. **Splitt `recorder/engine.rs` (3049 linjer)** i supervisor / progress-
    payloads / stderr-parsing. Størst enkeltfil i repoet.
13. **Tester for `recorder/cpal_capture.rs` (759 linjer)** — WASAPI/ASIO
    PCM-capture-stien er eneste store utestede produksjonsmodul. Også
    `media/preview.rs` (1404 linjer) mangler tester.
14. **`tokio = { features = ["full"] }`** i `src-tauri` — smalere featuresett
    kutter kompileringstid og binærstørrelse.
15. **Strukturerte feil i `secrets/mod.rs`/`cloud/oauth_flow.rs`** — keychain-
    feil flates i dag til `AppError::Internal(String)`.

## CI / release

16. ~~Vurder PR/push-trigger for CI~~ **Gjort (2026-07-08):** repoet ble
    offentlig (gratis Actions-minutter), og `ci.yml` trigges nå på push til
    `main` + PR-er i tillegg til `v*`-tagger.
17. **macOS Intel/universal-target** i `release.yml` (i dag kun arm64) +
    Windows whisper-feature når MSVC-bygget er verifisert.
18. **Åpne punkter fra `docs/NEEDS-RICHARD.md`** (konto/nøkkel/rigg):
    Apple-signering/notarisering (p12-passord + ny app-spesifikk
    nøkkel), `TAURI_SIGNING_*`-secrets, Google OAuth desktop-klient, Windows
    code-signing, rigg-verifisering av capture/ASIO/wake/NDI/streaming
    (inkl. `stream_start`-signaturmismatchen) og sample-rate-velgeren
    (avklaringspunktet nederst i NEEDS-RICHARD).
