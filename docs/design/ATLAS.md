# ATLAS — SundayRec slik appen VAR (v0.15, Fase A)

> 📦 **Dette dokumentet er historisk — v2-notis.** IA-uttrekket under beskriver
> **legacy-skallet** slik det sto i v0.15.0, og det skallet finnes ikke lenger.
> Bildene som hørte til flyttet 2026-08-30 til
> [`atlas-v015/`](atlas-v015/INDEX.md) (arkivet, frosset).
>
> **Det gjeldende atlaset er [`atlas/`](atlas/INDEX.md)** — appen slik den er
> etter D2, D3 og V1 — og det fotograferes på nytt med `npm run atlas`
> (`e2e/atlas/**` + `playwright.atlas.config.ts` er tilbake, mot dagens
> `getByTestId`-scener). Scenetabellen står i `atlas/INDEX.md`.
>
> Analysen under BESTÅR, uendret, av samme grunn som bildene: den er
> begrunnelsen bak halvparten av beslutningene i redesignet. Les den som «slik
> var det», ikke som «slik er det». Hver `atlas/`-lenke i teksten er derfor
> pekt om til `atlas-v015/`.

**Fase A i «Frivilligen først».** Dette dokumentet beskriver informasjonsarkitekturen
i SundayRec slik den står på `main` etter Fase R (PR #139 fjernet delings-klyngen,
PR #141 innholds-klyngen — til sammen ~38 000 linjer: sky, podkast, webhooks,
integrasjoner, whisper, prekenhjelp, kapitler, læringskort), utgitt som v0.15.0.

Det er **det legacy-skallet** (`legacy/renderer/`) som er fotografert — det som
faktisk sendes ut. Det parallelle Preact-skallet i `app/` (PR #143, S0) har
ingenting å fotografere ennå.

Ingenting her er et forslag. Det er en beskrivelse — inndata til Fase D, der hele
UI-et tegnes på nytt for en frivillig som aldri har sett appen.

Bildene ligger i [`atlas-v015/`](atlas-v015/INDEX.md); scene-id-er i teksten under viser dit.
**63 scener × 2 språk = 181 PNG-er, 8,8 MB**, tatt av `npm run atlas`.
Konsollvakten under fotograferingen står i
[`atlas-v015/CONSOLE-FINDINGS.md`](atlas-v015/CONSOLE-FINDINGS.md).

> ⚠️ **Fotografen som tok DISSE bildene finnes ikke lenger.** `e2e/atlas/**`,
> `playwright.atlas.config.ts` og `npm run atlas` ble slettet i fase B sammen
> med skallet de fotograferte: de drev Vite-serveren fra `playwright.config.ts`
> og klikket seg gjennom legacy-selektorer, så etter byttet ville kommandoen
> startet det NYE skallet og feilet hver eneste scene. Bildene i
> `atlas-v015/` er derfor ikke regenererbare — de er frosset som Fase A-fasit.
>
> V1/PR6 skrev fotografen på nytt, mot dagens skall og dagens `data-testid`-er.
> `npm run atlas` fotograferer nå inn i [`atlas/`](atlas/INDEX.md); den rører
> ikke arkivet.

---

## 0. Tallene

|                                                        |                                                                                                                                                                              Antall |
| ------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------: |
| Sider i navigasjonen                                   |                                                                                                                       **5** (Hjem · Tidsplan · Rediger · Historikk · Innstillinger) |
| Faner inne i sider                                     |                                                                                                                                                **8** (Innstillinger ×5, Rediger ×3) |
| Fullskjermsoverlegg                                    |                                                                                                                               **2** (opptaksoverlegget, første-oppstart-veiviseren) |
| Faste modaler                                          | **6** (`modal-manual` · `modal-confirm-stop` · `modal-note` · `audio-diagnose-modal` · `telemetry-preview-modal` · `editor-export-modal`) + `ui/dialog.ts`-bekreftelser (dynamiske) |
| Bannere/toasts som kan stå over alt                    |                                              **6** (global feilstripe · `ui-banner`-regionen · `ui-toast`-stabelen · oppdateringsvarselet · samtykkekortet · `editor-prompt-toast`) |
| Interaktive kontroller totalt (markup)                 |                                                                                                                                                                            **~231** |
| — herav Innstillinger (5 faner + fanestripe)           |                                                                                                                                                                              **76** |
| — herav Rediger + eksportmodalen                       |                                                                                                                                                                              **72** |
| — herav Hjem                                           |                                                                                                                                                                              **21** |
| Innstillingskontroller (den formelle opptellingen, §2) |                                                                                                                            **65** (64 synlige) — dirigentplanen talte 74 før Fase R |
| Bakendkommandoer i `api-shim.ts`                       |                                                                                                                                  **96** (91 nås fra renderer; 5–6 er død overflate) |
| i18n-nøkler i katalogen                                |                                                                                                                                         **895** per språk, 7 språk, perfekt paritet |
| — i bruk                                               |                                                                                                                                                           858 · **37 foreldreløse** |
| `data-i18n*`-attributter i markup                      |                                                                                                                                                             **437** (405 distinkte) |
| `t()`-kall i sidemodulene                              |                                                                                                                                                                            **~497** |
| Språk i språkvelgeren                                  |                                                                                                                                        **7** — men bare **no** og **en** er i drift |

### i18n-nøkler per side

| Side                                    | `data-i18n*` i markup | `t()` i modulene | ≈ totalt |
| --------------------------------------- | --------------------: | ---------------: | -------: |
| Innstillinger (5 faner)                 |                   161 |              115 |  **276** |
| Rediger (+ eksportmodal + prompt-toast) |                   111 |               89 |  **200** |
| Tidsplan                                |                    45 |               75 |  **120** |
| Hjem                                    |                    42 |               56 |   **98** |
| Første oppstart                         |                     1 |               64 |   **65** |
| Historikk                               |                    24 |               40 |   **64** |
| Opptaksoverlegget                       |                    15 |               33 |   **48** |
| Skall (sidepanel, toasts, status)       |                    21 |               25 |   **46** |

### Klikk per flyt (§3)

| Flyt                                                |                                      Klikk | Distinkte flater |
| --------------------------------------------------- | -----------------------------------------: | ---------------: |
| (a) Manuelt opptak fra **kald** app til fil på disk |                                     **10** |                7 |
| (a′) Samme, når appen er satt opp                   |                                      **4** |                3 |
| (b) Åpne opptak → finne preken → klippe → lagre     |                                    **5–7** |                4 |
| (c) Mix/master                                      |  5 uavhengige steder, ingen felles inngang |                3 |
| (d) Eksport (lyd)                                   |       **3** minimum, **9** hvis alt velges |                2 |
| (e) Første oppstart til «klar til søndag»           | **6** (hopp over alt) – **12+** (gjør alt) |           6 steg |

---

## 1. IA-treet som det ER

Notasjon: `#id` · `i18n-nøkkel` · _(N kontroller)_. `[skjult]` = `display:none` i markup.
Kommandolisten er hva sidemodulen kaller gjennom `api-shim.ts`.

### 1.0 Skallet — på alle sider

```
nav#sidebar (5 kontroller)
├─ logo — ingen nøkkel
├─ 5 navigasjonslenker · nav.home / nav.schedule / nav.editor / nav.search / nav.settings
│     «Hjem · Tidsplan · Rediger · Historikk · Innstillinger»
│     Frivilligen ser: fem ord som skal dekke hele appen.
├─ #status-dot + #status-label · status.ready «Alt er klart»
│     Én prikk og én setning som påstår at alt er i orden. Males fra delt tilstand
│     (status/next-recording.ts), ikke fra siden man står på.
└─ #sidebar-version — versjonsnummeret, nederst
```

Fem flater kan legge seg over hva som helst:

| Flate              | id                                  | Hva frivilligen ser                                                          | Scene                                          |
| ------------------ | ----------------------------------- | ---------------------------------------------------------------------------- | ---------------------------------------------- |
| Global feilstripe  | `#global-error-banner`              | Rød stripe øverst. Fyres av `recording://error`, CSP-feil og oppstartsfeil.  | `home--backend-feil`                           |
| Banner-region      | `.ui-banner-region` (`ui/toast.ts`) | Nøklede bannere som blir stående til de lukkes — tapt opptak, datatap-alarm. | `home--tapt-opptak`, `home--kvalitetsalarm`    |
| Toast-stabel       | `.ui-toast-stack`                   | Kortlevde meldinger, noen med «Angre».                                       | `search--slett-angre`, `toast--lagring-feilet` |
| Oppdateringsvarsel | `#update-toast`                     | Nede til høyre: «Oppdatering tilgjengelig» + installer.                      | `settings-general--oppdatering-varsel`         |
| Samtykkekort       | `#telemetry-consent-toast`          | Engangsspørsmålet om anonym diagnostikk.                                     | `home--samtykkekort`                           |

### 1.1 `#page-home` — «Hjem» (21 kontroller, 42 markup-nøkler)

Kommandoer: `run_preflight` · `get_disk_space` · `recordings_list` · `trash_list` ·
`list_devices` · `ffmpeg_health` · `media_permissions` · `run_test_recording` ·
`run_capture_bench` · `settings_save` · `scheduler_reschedule` (+ `start_vu`/`stop_vu`
via VU-feeden, `scheduler_status` via next-recording-lageret).

```
#missed-card [skjult] (2) — tittel males fra kode
    «N planlagte opptak ble ikke tatt opp» + liste. Frivilligen ser: en rød alarm
    om at gudstjenesten ikke ble tatt opp. Drives av `scheduler://missed`.
#preflight-card [skjult] (2) — tittel males fra kode
    Funn fra pre-start-sjekken 30 min før start. Frivilligen ser: gule/røde punkter
    om at noe må fikses. Drives av `scheduler://preflight`.
#hero-ok (0) · status.ok / home.readyTitle «Alt er klart»
    ├─ #hero-next-section · home.nextRecording — #next-date / #next-countdown
    └─ #next-wake-badge [skjult] — om maskinen vekkes for opptaket
#hero-warn [skjult] (2) · home.warnTitle «Lydmikseren er ikke tilkoblet»
    └─ #hero-warn-detail — «Koble til {navn} via USB»
.quick-row (2) — #btn-start-recording «Start opptak» · #btn-video-toggle
.vu-section (1) · home.audioLevel «Lydnivå — live»
    ├─ #signal-dot/#signal-text («—»/Svakt/Bra/Høyt/Klipper!) · #signal-peak «Maks: −12,3 dBFS»
    ├─ L/R-bjelker: #vu-l/#vu-r, #vu-peak-l/r, #vu-clip-l/r, #vu-db-l/r
    └─ #btn-go-health · home.goCheck «Test og sjekk system →»
#video-preview-section [skjult] (6) — kameraforhåndsvisning + kildevelger
.info-strip (3) — tre statuskort:
    ├─ #home-audio-card · home.audioDevice «LYDKILDE» → #btn-go-audio-page «Endre»
    ├─ #home-format-card · home.formatLabel «FORMAT» («MP3 · 256k» / «Stereo · 48 kHz»)
    └─ #home-storage-card · home.storage «LAGRING» («412,3 GB ledig · … · ca. 300 t»)
#video-info-strip [skjult] (2) — KAMERA + VIDEOKVALITET
#video-mode-layout [skjult] (0) — TOMT skall. Når video slås på FLYTTER home.ts
    fysisk .vu-section, #video-preview-section og info-kortene inn i sporene her
    (og tilbake igjen). Samme DOM-noder, ny forelder.
#home-lower (1) — «Siste opptak» (5 rader) + #home-see-all «Se alle →»
```

### 1.2 `#page-schedule` — «Tidsplan» (23 kontroller, 45 markup-nøkler)

Kommandoer: `scheduler_reschedule` · `settings_save` · `wake_capabilities` ·
`wake_verify` · `wake_test` · `wake_cancel_test` · `wake_get_sleep_config` ·
`wake_fix_sleep` · `wake_failure_history` · `wake_reschedule`.

```
.wake-summary-card (1) — én dom øverst: blir maskinen vekket? → «Detaljer»
.cal-card (3) — månedskalender: #cal-prev/#btn-today/#cal-next, #cal-grid, forklaring
#cal-day-detail [skjult] (5) — dukker opp ved klikk på en dag:
    #special-name · #special-start · #special-stop · #btn-add-special
#cal-hint-card · calendar.hintShort «Klikk på en dag for å legge til opptak»
.card «Planlagte spesialopptak» → #planned-list
.card «Ukentlig tidsplan» → #slots-list · #schedule-next-preview · #btn-add-slot
#slot-editor [skjult] (5) — #day-picker (7 dagbrikker) · #slot-start · #slot-stop
    · #slot-duration-display · #slot-max · lagre/avbryt
.adv-card → #btn-adv-toggle «Vekk maskin fra dvale»
  └─ #adv-section [skjult] (7)
      ├─ #opt-wake (hovedbryter)
      └─ #wake-details [skjult] (6)
          ├─ «Maskinen din» — #wake-capability-text · #wake-capability-issues
          ├─ «Status akkurat nå» — strøm · standby · søvnkonfig · verifisering · siste test
          └─ #btn-test-wake «Test wake nå (60 sek)» · #btn-cancel-test-wake
```

### 1.3 `#page-settings` — 5 faner (76 kontroller, 161 markup-nøkler)

Fanestripe `#settings-tabs`: `nav.audio` Lyd · `nav.video` Video · `nav.files` **Opptak** ·
`nav.sharing` **Deling** · `nav.system` System.
Full kontrollinventar: [§2](#2-innstillingsinventar-etter-fase-r).

```
#settings-audio (14) — enhetsliste · kanalrutenett · sjekk-og-test · samplingsrate
    · «Lyd-motor (avansert)»
    kommandoer: list_audio_devices · list_audio_input_channels · scan_device_channels
                · diagnose_audio · run_diagnostics · ffmpeg_health · media_permissions
                · list_devices · settings_save · scheduler_reschedule
#settings-video (4) — hovedbryter + [kamera · behold separat lydfil]
    kommandoer: list_devices · get_camera_capabilities
#settings-files (19) — mappe · navnemønster · format/kvalitet · autosletting
    · «Mens et opptak går» (beskytt · stopp ved stillhet · maks lengde)
    · «Filer» (del opp · forhåndsbuffer)
    kommandoer: INGEN — ren innstillingsbinding (+ native mappevelger)
#settings-sharing (15) — kun «Varsler»: PC-varsler · påminnelse · e-post ved feil + SMTP
    kommandoer: email_status · email_send_test · email_set/clear/has_smtp_password
#settings-general (19) — språk · kirkeprofil · system · oppdateringer · hjelp
    · innstillingsprofil · personvern og diagnostikk (+ #telemetry-preview-modal)
    kommandoer: 21 (update_* ×4, telemetry_* ×5, settings_export/import, logs_*, …)
```

### 1.4 `#page-search` — «Historikk» (11 kontroller, 24 markup-nøkler)

Kommandoer: `recordings_list` · `trash_list` · `trash_move` · `trash_restore` ·
`trash_purge` · `recordings_delete` · `recordings_prune` · `recording_update_note`.

```
#search-query — fritekstsøk i filnavn og notat
#btn-trash-open · trash.open «Papirkurv» → #trash-view ERSTATTER tabellen
#search-index-status — «Ingen treff for …»
#search-history-wrap (7)
  ├─ #btn-clear-history · #btn-history-more «⋯» → #history-more-panel
  │      (#btn-delete-errors · #btn-prune-history)
  ├─ #history-stats — #stat-count / #stat-duration / #stat-last (følger filteret)
  ├─ #history-filter-chips — Alle · Lyd · Video
  └─ tabell → #history-tbody (radhandlinger: vis i Finder, rediger, notat, slett)
#trash-view [skjult] (2) — #btn-trash-empty · #btn-trash-close · #trash-list
#search-empty [skjult] — «ingen opptak ennå»
```

### 1.5 `#page-editor` — «Rediger» (47 kontroller + 25 i eksportmodalen)

Kommandoer: `editor_load_recording` · `editor_probe_streams` · `editor_peaks` ·
`editor_read_file` · `editor_extract_playback_proxy` · `editor_allow_asset_path` ·
`editor_segments` · `editor_sermon_pick` · `editor_record_sermon_pick` ·
`editor_read/write/delete_sidecar` · `editor_export` · `editor_cancel_export` ·
`editor_auto_process` · `editor_diagnose_channels` · `editor_master_presets` ·
`editor_master_preview` · `editor_master_apply` · `editor_master_cancel` ·
`editor_mastering_analyze` · `editor_probe_peak` · `haptic_perform`.

```
#editor-empty (1) — slippsone + «Åpne fil…» + siste opptak
#editor-loading [skjult] — «Analyserer …» + framdrift
#editor-workspace [skjult] (18 før fanene)
  ├─ topplinje: #editor-filename · #editor-dirty-dot · #editor-header-summary
  │      · #btn-editor-change · #btn-editor-close
  ├─ transport: spill · forhåndslytt · løkke · #editor-time-cur/#editor-time-tot
  ├─ #editor-minimap + #editor-canvas ← SELVE ARBEIDSFLATEN (bølgeformen)
  ├─ verktøy: zoom inn/ut/tilpass · #btn-editor-view-menu → lag-popover (tale/musikk/stillhet)
  ├─ #editor-suggestion-banner [skjult] (2) — «Forslag klart» + bruk/avvis
  ├─ #editor-cuts-panel [skjult] — «Kuttede regioner» → #editor-cuts-list
  ├─ #editor-remaining — «Resultat» + varighet
  ├─ #editor-tabs — Lyd · Innhold · Klipp-verktøy
  ├─ #editor-tabpanel-audio (18)
  │    ├─ #editor-normalize-panel — «Normaliser lydnivå» (mastring-sted 1 av 5)
  │    ├─ #editor-io-panel (9, sammenslått) — Intro & Outro (lyd + video)
  │    └─ #editor-master-section [skjult+sammenslått] (7) — «Mastering
  │           (klargjør for publisering)» (mastring-sted 2 av 5). Skjult for videofiler.
  ├─ #editor-tabpanel-content (4) — #meta-title · #meta-speaker · #meta-description
  ├─ #editor-tabpanel-clip (3) — «Analyser opptak» · «Marker preken automatisk»
  │       · #editor-sermon-picker «Er ikke dette prekenen?» · #editor-analyze-summary
  └─ lagre-linje: framdrift + #btn-editor-save «Eksporter»
#editor-export-modal (25 + 23 valg) — eksporttype · videoformat · videokodek · format
    · bitrate/bithybde · destinasjon · «Lydforbedring» (mastring-sted 3, 4 og 5)
#editor-prompt-toast (2) — etter et opptak: «vil du redigere?»
```

### 1.6 Overleggene

| Flate                | id                         |   Kontroller | Hva den gjør                                                                                                                                 |
| -------------------- | -------------------------- | -----------: | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Start manuelt opptak | `#modal-manual`            |            6 | Kilde, kamera, filnavn, start                                                                                                                |
| Opptaksoverlegg      | `#recording-overlay`       |            3 | Fullskjerm mens det spilles inn: tid, størrelse, diskplass, VU, bølgeform, stillhetsvarsel, auto-stopp-nedtelling, gjenkoblingsbanner, stopp |
| Stopp-bekreftelse    | `#modal-confirm-stop`      |            2 | «Stopp opptak?» — Avbryt er primærknappen                                                                                                    |
| Notat                | `#modal-note`              |            4 | Notat på en historikkrad                                                                                                                     |
| Lydenhetsdiagnose    | `#audio-diagnose-modal`    | 1 + dynamisk | Diagnoserader + full systemrapport                                                                                                           |
| Hva sendes           | `#telemetry-preview-modal` |            1 | Rå JSON-nyttelast                                                                                                                            |
| Første oppstart      | `#onboarding-overlay`      |   1 i markup | 6 steg, alt males av `onboarding.ts` (64 `t()`-kall)                                                                                         |
| Bekreftelser         | `.ui-dialog-backdrop`      |     dynamisk | `confirmIf`-vaktene (enhetsbytte, motorbytte, beta-kanal, autosletting < 30 dager, import, slett diagnostikk)                                |

---

## 2. Innstillingsinventar ETTER Fase R

**65 kontroller** (64 synlige — `email-port` er `type="hidden"`). Dirigentens
plan-kartlegging talte **74** før Fase R; ni er borte med delings- og
innholds-klyngen.

Kolonnen «Leses av» er grep i `crates/` + `src-tauri/` + `legacy/renderer/`.

### 2.1 Lyd — 12 kontroller

| id                            | Etikett (no)                       | i18n                                                 | Type        | Nøkkel                    | Default  | Leses av | Vakt                                   |
| ----------------------------- | ---------------------------------- | ---------------------------------------------------- | ----------- | ------------------------- | -------- | -------- | -------------------------------------- |
| `device-list`                 | Tilgjengelige enheter              | `audio.available`                                    | knappeliste | `deviceId` + `deviceName` | `null`   | begge    | **confirmIf** «Bytte lydenhet»         |
| `ch-grid`                     | Kanaler og signal                  | `audio.gridTitle`                                    | rutenett    | `deviceChannels[id]`      | `{}`     | begge    | — (inert ved ≤2 kanaler)               |
| `ch-assign-row`               | VENSTRE (L) / HØYRE (R)            | `audio.channelL/R`                                   | brikker     | —                         | —        | renderer | skjult ved ≤2 kanaler                  |
| `btn-scan-channels`           | 🔍 Finn kanaler med signal (3 sek) | `audio.scanChannels`                                 | knapp       | —                         | —        | —        | skjult unntatt ved fallback            |
| radio `channels` ×4           | Stereo / Mono / Mono L / Mono R    | `audio.channels` (**valgnavnene har ingen nøkkel**)  | radio       | `channels`                | `stereo` | begge    | tvinges til `monoL` for 1-kanals enhet |
| `btn-test-recording-settings` | 🎙️ Test-opptak (30 sek)            | `home.testRecording`                                 | knapp       | —                         | —        | —        | nekter under opptak                    |
| `btn-run-preflight-settings`  | ✅ Sjekk system nå                 | `home.checkSystem`                                   | knapp       | —                         | —        | —        | —                                      |
| `btn-capture-bench`           | 📏 Presisjonstest (60 sek)         | `audio.captureBench`                                 | knapp       | —                         | —        | —        | —                                      |
| `btn-audio-diagnose`          | Diagnose                           | `audio.diagnose`                                     | knapp       | —                         | —        | —        | —                                      |
| radio `sampleRate` ×3         | Automatisk / 44 100 Hz / 48 000 Hz | `audio.sampleRate` (**Hz-navnene har ingen nøkkel**) | radio       | `sampleRateMode`          | `auto`   | rust     | —                                      |
| `opt-classic-ffmpeg`          | Klassisk opptaksmotor (ffmpeg)     | `audio.classicFfmpeg`                                | bryter      | `classicFfmpegAudio`      | `false`  | rust     | **confirmIf** «Bytte opptaksmotor»     |
| `opt-classic-dshow`           | Klassisk lyd-motor (DirectShow)    | `audio.classicDshow`                                 | bryter      | `classicDirectshow`       | `false`  | rust     | **confirmIf** · kun Windows            |

### 2.2 Video — 4 kontroller

| id                          | Etikett               | i18n                   | Type   | Nøkkel                                  | Default | Leses av | Vakt                              |
| --------------------------- | --------------------- | ---------------------- | ------ | --------------------------------------- | ------- | -------- | --------------------------------- |
| `opt-video-enable`          | Aktiver videoopptak   | `video.enableTitle`    | bryter | `videoEnabled`                          | `false` | begge    | —                                 |
| `video-device-select`       | VELG KAMERA           | `video.pickCamera`     | select | `videoDeviceName` (+`videoDeviceIndex`) | `null`  | rust     | **confirmIf** · bak hovedbryteren |
| `btn-video-refresh-devices` | Oppdater              | `video.refresh`        | knapp  | —                                       | —       | —        | bak hovedbryteren                 |
| `opt-video-keep-audio`      | Behold separat lydfil | `video.keepAudioTitle` | bryter | `keepSeparateAudio`                     | `true`  | begge    | bak hovedbryteren                 |

### 2.3 Opptak («Filer») — 15 kontroller

| id                      | Etikett                         | i18n                                          | Type                | Nøkkel                  | Default | Leses av         | Vakt                         |
| ----------------------- | ------------------------------- | --------------------------------------------- | ------------------- | ----------------------- | ------- | ---------------- | ---------------------------- |
| `save-folder`           | LAGRES I                        | `files.saveTo`                                | tekst (kun visning) | `saveFolder`            | `null`  | begge            | —                            |
| `btn-pick-folder`       | Velg mappe                      | `files.browse`                                | mappevelger         | `saveFolder`            | `null`  | begge            | native dialog                |
| `pattern-select`        | NAVNEFORMAT                     | `files.nameFormat`                            | select ×4           | `filenamePattern`       | `date`  | begge            | —                            |
| radio `format` ×3       | MP3 / FLAC / WAV                | `files.format` (**navnene har ingen nøkkel**) | radio               | `format`                | `mp3`   | begge            | —                            |
| radio `bitrate` ×3      | Kompakt / Anbefalt / Høyeste    | kun `files.qualityHighest` har nøkkel         | radio               | `bitrate`               | `256`   | begge            | skjult unntatt mp3/aac       |
| `opt-auto-delete`       | Slett automatisk etter 90 dager | `files.autoDelete`                            | bryter              | `autoDeleteDays`        | `0`     | rust             | **confirmIf** < 30 dager     |
| `auto-delete-days`      | Slett etter … dager             | `files.deleteAfter`                           | tall 1–3650         | `autoDeleteDays`        | `0`     | rust             | **confirmIf** · bak bryteren |
| `opt-protect`           | Beskytt pågående opptak         | `schedule.protectTitle`                       | bryter              | `protectRecording`      | `true`  | **kun renderer** | —                            |
| `opt-silence`           | Stopp ved vedvarende stillhet   | `schedule.silenceTitle`                       | bryter              | `stopOnSilence`         | `false` | rust             | —                            |
| `opt-silence-threshold` | **GRENSE (dBFS)**               | `schedule.silenceThresholdLabel`              | select −40…−70      | `silenceThreshold`      | `-50`   | rust             | bak `opt-silence`            |
| `opt-silence-timeout`   | VARIGHET FØR STOPP              | `schedule.silenceTimeoutLabel`                | select              | `silenceTimeoutMinutes` | `5`     | rust             | bak `opt-silence`            |
| `opt-manual-max`        | Maks varighet — manuelle opptak | `schedule.manualMaxTitle`                     | select 0–360        | `manualMaxMinutes`      | `0`     | begge            | —                            |
| `opt-split-minutes`     | Del opp filer per time          | `schedule.splitTitle`                         | select              | `splitMinutes`          | `0`     | rust             | —                            |
| `opt-preroll-seconds`   | Forhåndsbuffer                  | `schedule.preRollTitle`                       | select 0/15/30 s    | `preRollSeconds`        | `0`     | begge            | —                            |
| `opt-preroll-enabled`   | Aktiver forhåndsbuffer          | `schedule.prerollEnabledTitle`                | bryter              | `prerollEnabled`        | `false` | **kun renderer** | —                            |

### 2.4 Deling — 16 kontroller (15 synlige)

Etter #139 inneholder fanen **bare** seksjonen «Varsler». Fanenavnet og innholdet
henger ikke lenger sammen.

| id                     | Etikett                        | i18n                        | Type        | Nøkkel            | Default | Leses av | Vakt                                 |
| ---------------------- | ------------------------------ | --------------------------- | ----------- | ----------------- | ------- | -------- | ------------------------------------ |
| `opt-notify-start`     | Varsel når opptak starter      | `general.notifyStart`       | bryter      | `notifyStart`     | `true`  | rust     | —                                    |
| `opt-notify-stop`      | Varsel når opptak avsluttes    | `general.notifyStop`        | bryter      | `notifyStop`      | `true`  | rust     | —                                    |
| `opt-reminder-minutes` | Påminnelse før opptak          | `schedule.reminderTitle`    | select      | `reminderMinutes` | `0`     | rust     | —                                    |
| `opt-email-error`      | Send e-post ved feil           | `general.emailError`        | bryter      | `emailOnError`    | `false` | rust     | hele kortet bak `applyFeatureGate`   |
| `email-address`        | SEND TIL                       | `general.sendTo`            | e-post      | `emailAddress`    | `""`    | rust     | regex + bak bryteren                 |
| `email-smtp-advanced`  | Oppsett av e-postserver (SMTP) | `notify.emailAdvanced`      | `<details>` | —                 | —       | —        | —                                    |
| `email-smtp`           | E-POSTTJENESTE (SMTP)          | `notify.smtpServer`         | tekst       | `emailSmtp`       | `""`    | rust     | eksplisitt lagring                   |
| `email-user`           | BRUKERNAVN                     | `notify.smtpUser`           | tekst       | `emailSmtpUser`   | `""`    | rust     | eksplisitt lagring                   |
| `email-from`           | AVSENDERADRESSE                | `notify.smtpFrom`           | e-post      | `emailSmtpFrom`   | `""`    | rust     | eksplisitt lagring                   |
| `email-pass`           | PASSORD / APP-PASSORD          | `notify.smtpPass`           | passord     | **OS-nøkkelring** | —       | rust     | aldri i innstillingsblobben          |
| `btn-save-smtp-pass`   | Lagre i nøkkelring             | `notify.savePassToKeychain` | knapp       | —                 | —       | —        | nekter tomt                          |
| `btn-clear-smtp-pass`  | Fjern                          | `general.clearPass`         | knapp       | —                 | —       | —        | kun når passord finnes               |
| `email-port`           | _(ingen — skjult, fast 587)_   | —                           | hidden      | `emailSmtpPort`   | `587`   | rust     | ikke synlig                          |
| `btn-smtp-save`        | Lagre                          | `general.save`              | knapp       | 4 SMTP-felt       | —       | —        | validerer først                      |
| `btn-smtp-cancel`      | Avbryt                         | `general.cancel`            | knapp       | —                 | —       | —        | —                                    |
| `btn-test-email`       | Test e-post                    | `general.testEmail`         | knapp       | —                 | —       | —        | `disabled` uten transport + mottaker |

### 2.5 System — 18 kontroller

| id                       | Etikett                          | i18n                           | Type   | Nøkkel                                          | Default  | Leses av                 | Vakt                             |
| ------------------------ | -------------------------------- | ------------------------------ | ------ | ----------------------------------------------- | -------- | ------------------------ | -------------------------------- |
| `language-select`        | APPSPRÅK (7 språk)               | `general.appLanguage`          | select | `language`                                      | `null`   | begge                    | —                                |
| `church-name`            | MENIGHET / KIRKE                 | `general.church`               | tekst  | `churchName`                                    | `""`     | rust (kun varsel-e-post) | —                                |
| `responsible-person`     | ANSVARLIG PERSON                 | `general.responsible`          | tekst  | `responsiblePerson`                             | `""`     | rust                     | —                                |
| `opt-autostart`          | Start automatisk med Windows/Mac | `general.autoStart`            | bryter | `launchAtLogin`                                 | `false`  | renderer → OS            | —                                |
| `opt-ask-open-editor`    | Spør om redigering etter opptak  | `general.askOpenEditor`        | bryter | `askOpenEditor`                                 | `true`   | **kun renderer**         | —                                |
| `opt-auto-update`        | Oppdater automatisk              | `general.autoUpdate`           | bryter | `autoUpdate`                                    | `true`   | **kun renderer**         | —                                |
| `opt-update-channel`     | Oppdateringskanal                | `general.updateChannel`        | select | `updateChannel`                                 | `stable` | rust                     | **confirmIf** kun mot beta       |
| `btn-check-updates`      | Se etter oppdateringer nå        | `general.checkNow`             | knapp  | —                                               | —        | —                        | bevisst ugatet                   |
| `btn-restart-install`    | Start på nytt og installer       | `update.btnRestartInstall`     | knapp  | —                                               | —        | —                        | skjult til en oppdatering finnes |
| `btn-show-onboarding`    | Åpne oppstartsveileder           | `general.openOnboarding`       | knapp  | —                                               | —        | —                        | —                                |
| `btn-show-log`           | Vis logg                         | `general.showLog`              | knapp  | —                                               | —        | —                        | —                                |
| `btn-copy-log`           | Kopier siste logg                | `general.copyLog`              | knapp  | —                                               | —        | —                        | —                                |
| `btn-settings-export`    | Eksporter innstillinger…         | `general.settingsExportBtn`    | knapp  | —                                               | —        | —                        | native dialog                    |
| `btn-settings-import`    | Importer innstillinger…          | `general.settingsImportBtn`    | knapp  | —                                               | —        | —                        | **confirmDialog**                |
| `opt-telemetry-consent`  | Del anonym diagnostikk           | `general.telemetryToggleTitle` | bryter | _ikke i Settings_ — `telemetry_consent_get/set` | av       | rust                     | bevisst ingen bekreftelse        |
| `btn-telemetry-preview`  | Vis hva som sendes               | `general.telemetryPreviewBtn`  | knapp  | —                                               | —        | —                        | —                                |
| `btn-telemetry-delete`   | Slett mine data                  | `general.telemetryDeleteBtn`   | knapp  | —                                               | —        | —                        | **confirmDialog (danger)**       |
| `telemetry-privacy-link` | Les personvernerklæringen ↗      | `general.telemetryReadMore`    | lenke  | —                                               | —        | —                        | —                                |

### 2.6 Funn i inventaret

**Fire innstillinger har ingen leser i bakenden.** Ingen er helt døde, men alle fire
håndheves _bare_ av rendereren — altså ignorert av alt bakenden gjør på egen hånd
(planlagte opptak, importerte profiler brukt før UI-et har kjørt):

1. **`protectRecording`** — null Rust-lesere. Kun `pages/recording.ts`.
2. **`askOpenEditor`** — null Rust-lesere.
3. **`prerollEnabled`** — null Rust-lesere. Verre: bakenden porter forhåndsbufferen på
   `pre_roll_seconds > 0`, og telemetrien _utleder_ `preroll_enabled` fra det samme
   tallet. Telemetrien rapporterer altså en verdi som ikke er bryteren brukeren satte.
   Klassisk [skjøtefeil](../../README.md).
4. **`autoUpdate`** — eneste Rust-referanse er en kopi inn i telemetri-snapshotet;
   den timesvise sjekken bor i `general-page.ts`.

**Døde påstander (ikke døde nøkler):**

- Kirkeprofil-kortet sier «Brukes i **filnavn**, varslings-e-poster og **podcast-RSS**».
  `churchName` brukes _ikke_ i filnavn (`opts.rs` sender `church_name: None`), og
  podkast-RSS ble fjernet i #139. To tredeler av setningen er usann.
- Språkkortet sier «SundayRec støtter **syv språk**» — velgeren viser sju, men bare
  no og en er i drift.
- Innstillingsprofil-hjelpeteksten nevner «strømmenøkler» — også en fjernet funksjon.
- «Mastering»-panelet i editoren sier «for podkast og streaming».

**Etiketter som hardkoder en verdi brukeren kan endre:** «Slett automatisk etter **90**
dager» (feltet går 1–3650) · «…stille i mer enn **5** minutter» (velgeren har 2/5/10/15)
· «Del opp filer **per time**» (velgeren har 30/45/60/90/120 min).
**Bokstavelig «N» i brukertekst:** `schedule.reminderDesc` og `schedule.preRollDesc`.

**Aldri oversatt, i en app med sju språk:** «Stereo», «Mono», «Mono L», «Mono R»,
«44 100 Hz», «48 000 Hz», korttittelen «MP3-kvalitet», «Kompakt», «Anbefalt», «MP3»,
«FLAC», «WAV», «Tapsfri komprimering», «dBFS»-enheten i opptaksoverlegget, alle
minutt-/timesverdier i de fem select-ene, og hele `editor/mixer.ts` (~30 strenger).

---

## 3. De fire flytene, klikk for klikk

### (a) Manuelt opptak — kald app → fil på disk

En helt fersk installasjon har **ingen lydenhet valgt og ingen lagringsmappe**, og
den tar likevel opp. Begge har en stille standardverdi:

- `save_folder` = `<Dokumenter>/SundayRec` (`settings.rs::DEFAULT_SAVE_SUBFOLDER`).
  Feilen `no_save_folder` oppstår bare hvis OS-et ikke kan oppgi noen
  Dokumenter-mappe i det hele tatt.
- `deviceId` = `null` → opptakeren åpner **systemets standard inngang**, altså den
  innebygde mikrofonen på maskinen.

Den kalde stien er derfor kort:

|   # | Klikk                                                 | Flate                                    | Scene                       |
| --: | ----------------------------------------------------- | ---------------------------------------- | --------------------------- |
|   1 | «Start opptak»                                        | `#modal-manual`                          | `home--start-dialog`        |
|   2 | «Start opptak» i dialogen                             | Opptaksoverlegget                        | `opptak--pagar`             |
|   3 | «Trykk for å stoppe opptaket»                         | `#modal-confirm-stop`                    | `opptak--stopp-bekreftelse` |
|   4 | «Stopp opptak»                                        | Overlegget i «Fullfører …» → fil på disk | `opptak--fullforer`         |
|   — | _(hvis `askOpenEditor`: en toast spør om redigering)_ | `#editor-prompt-toast`                   |                             |

**4 klikk, 3 flater** — og en fil på disk, tatt opp fra maskinens egen mikrofon,
uten at noe på veien har nevnt det. Ingen dialog, ingen advarsel, ingen bekreftelse
av kilde. LYDKILDE-kortet på Hjem sier da «Innebygd mikrofon · **Tilkoblet**» i grønt
(`home--kald-forstegangs`), og velgeren i `#modal-manual` står på standardenheten.

Den kalde stien en frivillig _burde_ gå — sette opp mikseren først — er lengre og
finnes ingen steder i UI-et som en anvist rekkefølge:

|    # | Klikk                                                           | Flate                  | Scene                           |
| ---: | --------------------------------------------------------------- | ---------------------- | ------------------------------- |
|    1 | Sidepanel → «Innstillinger»                                     | Innstillinger › Lyd    | `settings-audio--ingen-enheter` |
|    2 | Klikk enhetskortet                                              | _(samme)_              | `settings-audio--enheter`       |
|  3–4 | _(≥ 3 kanaler: to trykk i kanalrutenettet — venstre, så høyre)_ | _(samme)_              | `settings-audio--kanalrutenett` |
|    5 | Fane «Opptak»                                                   | Innstillinger › Opptak | `settings-files--standard`      |
|    6 | «Velg mappe» _(valgfritt — standarden virker)_                  | **native OS-dialog**   | _(ikke fotograferbar)_          |
|    7 | Sidepanel → «Hjem»                                              | Hjem                   | `home--klar-med-enhet`          |
| 8–10 | Start → Start → Stopp → Stopp                                   | som over               |                                 |

**10 klikk, 7 distinkte flater** (én av dem utenfor appen).

### (b) Åpne opptak → finne preken → klippe → lagre

|   # | Klikk                                                                                                          | Flate                  | Scene                   |
| --: | -------------------------------------------------------------------------------------------------------------- | ---------------------- | ----------------------- |
|   1 | Klikk en rad i «Siste opptak» _(eller Historikk → blyantikonet)_                                               | Rediger › Lyd          | `editor--lyd-fane`      |
|   — | _(analysen starter av seg selv i bakgrunnen — `runDetection(true)` når bølgeformen er klar; kun for lydfiler)_ |                        | `editor--laster`        |
|   2 | Fane «Klipp-verktøy»                                                                                           | Rediger › Klipp        | `editor--klipp-fane`    |
|   3 | «Marker preken automatisk»                                                                                     | Kuttlisten fylles      | `editor--kuttliste`     |
|   4 | _(valgfritt)_ «Er ikke dette prekenen?» → velg riktig blokk                                                    | _(samme)_              |                         |
|   5 | «Eksporter»                                                                                                    | Eksportmodalen         | `editor--eksport-modal` |
|   6 | _(valgfritt)_ velg format/destinasjon                                                                          | _(samme)_              |                         |
|   7 | «Eksporter» i modalen                                                                                          | Framdrift → ferdig fil |                         |

**5 klikk minimum (7 med korrigering og formatvalg), 4 flater.**
Merk: forslagsbanneret «Forslag klart» dukker opp av seg selv på Lyd-fanen når
analysen er ferdig, med «Bruk forslag» — det er en _femte_ vei til samme kutt.

### (c) Mix/master — fem steder, ingen felles inngang

|   # | Sted                                                                                                                 | Hva det gjør                                                                                                                              | Resultat                                                                                                                                                                            |
| --: | -------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
|   1 | Rediger › Lyd › **«Normaliser lydnivå»** (`#editor-normalize-panel`)                                                 | Løfter toppunktet til −1 dBFS. Beregnes via `editor_probe_peak`.                                                                          | Ingenting nå — **høres først ved eksport** (teksten sier det selv)                                                                                                                  |
|   2 | Rediger › Lyd › **«Mastering (klargjør for publisering)»** (`#editor-master-section`, sammenslått, skjult for video) | EBU R128 i to passeringer mot en LUFS-forhåndsinnstilling, via `editor_master_apply`                                                      | Skriver en **NY fil** `<navn>_mastert.<ext>` ved siden av originalen. Påvirker **ikke** eksporten. Panelets egen overskrift sier «for eksport med mastring: bruk Eksporter-knappen» |
|   3 | Eksportmodalen › **«Mastering (utgivelsesnivå)»** (`#enhance-master-preset`)                                         | Samme forhåndsinnstillinger, men i eksportkjeden                                                                                          | Ligger i den eksporterte fila. Overstyrer normaliseringen fra sted 1 («Volum styres av mastring»)                                                                                   |
|   4 | Eksportmodalen › **«✨ Automatisk lydforbedring (ett klikk)»** (`#btn-auto-enhance`)                                 | `editor_auto_process`: setter stemmebehandling + kanalreparasjon. Setter **bevisst ikke** mastring (ville dobbeltbehandlet)               | Fyller de andre feltene i modalen                                                                                                                                                   |
|   5 | Eksportmodalen › **«🎛 Avansert lydmikser»** (`#opt-use-mixer`)                                                       | ~20 kontroller: HPF, støyreduksjon, romdemping, gate, kompressor, de-esser, limiter, 3-bånds EQ, sluttgain. Overstyrer stemmebehandlingen | Ligger i den eksporterte fila                                                                                                                                                       |

**Tre flater, fem inngangsdører, to helt ulike utfall** (ny fil ved siden av
originalen vs. behandling i eksporten). Ingenting i UI-et forklarer forskjellen
utover én linje i overskriften på sted 2.

### (d) Eksport

`#btn-editor-save` «Eksporter» → `#editor-export-modal`. Alt er ett klikk unna:

| Rad           | Valg                                                                                   | Bare for   |
| ------------- | -------------------------------------------------------------------------------------- | ---------- |
| Eksporttype   | Video (med lyd) / Kun lyd                                                              | videofiler |
| Video-format  | MP4 / MOV / MKV                                                                        | videofiler |
| Video-kodek   | H.264 (universell) / H.265 (mindre fil)                                                | videofiler |
| Format        | MP3 / WAV / FLAC / AAC                                                                 |            |
| Bitrate       | 128 / 192 / 256 (standard) / 320 kbps                                                  | mp3, aac   |
| Bithybde      | 16-bit (CD-kvalitet) / 24-bit (studio)                                                 | wav        |
| Destinasjon   | **Samme mappe** / Velg mappe…                                                          |            |
| Behandling    | lesbart sammendrag av normalisering/mastring                                           |            |
| Intro & outro | lesbart sammendrag                                                                     |            |
| Lydforbedring | auto-knapp · stemmebehandling ×4 · mastering ×5 · kanalreparasjon ×5 · avansert mikser |            |

**3 klikk minimum** (Eksporter → Eksporter → ferdig), **9+ hvis alt velges**.
Standarddestinasjonen er «Samme mappe» — altså ved siden av originalopptaket, i
lagringsmappen. Scener: `editor--eksport-modal`, `editor--eksport-modal-video`.

### (e) Første oppstart → «klar til søndag»

`#onboarding-overlay`, seks steg. Steg 2–4 har hver sin «Hopp over dette steget».

| Steg | Skjerm                                     | Handling                            | Scene                     |
| ---: | ------------------------------------------ | ----------------------------------- | ------------------------- |
|    1 | «Velkommen til SundayRec»                  | «Kom i gang →»                      | `onboarding--1-velkommen` |
|    2 | «Hvilken lydenhet bruker dere?»            | velg enhet, eller hopp over         | `onboarding--2-lydenhet`  |
|    3 | «Test at lyden fungerer» (live VU)         | snakk i mikrofonen, eller hopp over | `onboarding--3-lydtest`   |
|    4 | «Ukentlig automatisk opptak»               | sett tid, eller hopp over           | `onboarding--4-tidsplan`  |
|    5 | «Vil du hjelpe oss gjøre SundayRec bedre?» | Ja, del anonymt / Nei takk          | `onboarding--5-samtykke`  |
|    6 | «Alt er klart!» + tre tips                 | «Åpne SundayRec →»                  | `onboarding--6-ferdig`    |

**6 klikk hvis alt hoppes over; 12+ hvis alt gjøres.** Veiviseren spør **aldri** om
lagringsmappe — det er nettopp den innstillingen som gjør at det første manuelle
opptaket avvises (§3a). Steg 6 sier «Alt er klart!» til en app som ikke kan ta opp.

---

## 4. Statusspråk-inventar

86 av 895 norske nøkler (~9,6 %) inneholder minst ett teknisk begrep, pluss ~45
hardkodede strenger i markup og ~30 til i `editor/mixer.ts`.
Under: de en frivillig møter **på nivå 1** — uten å åpne noe.

### 4.1 Hjem, kald tilstand — det aller første et menneske ser

|   # | Hvor                       | Nøkkel / kilde                                    | Streng                                               | Begrep                          |
| --: | -------------------------- | ------------------------------------------------- | ---------------------------------------------------- | ------------------------------- |
|  H1 | `#home-format-value`       | **runtime**, `home.ts` — ingen i18n-nøkkel finnes | `MP3 · 256k`                                         | MP3, bitrate som bar «k»        |
|  H2 | `#home-format-sub`         | **runtime**                                       | `Stereo · 48 kHz`                                    | stereo/mono, kHz                |
|  H3 | `#signal-peak`             | `home.peakLabel` + runtime                        | `Maks: −12,3 dBFS`                                   | **dBFS**                        |
|  H4 | `#vu-db-l` / `#vu-db-r`    | runtime, `audio/vu.ts`                            | `−18,4` — **helt uten enhet**                        | umerket dB                      |
|  H5 | klippelampene              | `tooltip.clipReset`                               | `Clip — klikk for å nullstille`                      | **«Clip»**, engelsk, i norsk UI |
|  H6 | `#home-device-status-text` | `home.sourceChannels`                             | `Kanal 1/2 · Stereo — Tilkoblet`                     | kanal, stereo                   |
|  H7 | `#signal-text`             | `home.signalClipping`                             | `Klipper!`                                           | clipping, verbet                |
|  H8 | `#preflight-card-list`     | `health.ffmpegMissing`                            | `Den innebygde lydmotoren (ffmpeg) ble ikke funnet…` | **ffmpeg**                      |
|  H9 | `#home-preroll-chip`       | `home.prerollActive`                              | `Forhåndsbuffer aktiv (5 s)`                         | buffer/preroll                  |
| H10 | `#home-video-quality`      | **hardkodet**, `home.ts`                          | `1080p · 30 fps · MP4`                               | 1080p, fps, MP4                 |

H1–H2 er det tetteste sjargong-punktet i hele appen, og det er _generert i kode_ —
det finnes ingen i18n-nøkkel å omformulere. «MP3 · 256k / Stereo · 48 kHz» må bli
noe i retning av «God kvalitet — passer for nett og deling».

### 4.2 Opptaksoverlegget (fullskjerm, mens gudstjenesten går)

| Hvor                                   | Streng                             | Begrep     |
| -------------------------------------- | ---------------------------------- | ---------- |
| markup, **hardkodet uten i18n-nøkkel** | `dBFS` (enhetsetiketten under L/R) | dBFS       |
| `#rec-vu-db-l/r`                       | `−12,4`                            | umerket dB |
| klippelampe                            | `Clip — klikk for å nullstille`    | Clip       |

### 4.3 Innstillinger › Lyd, nivå 1

`Kanaler og signal` · `Trykk på kanalen for venstre, så kanalen for høyre` ·
`Stereo / Mono / Mono L / Mono R` · **`Samplingsrate`** · `ingen resampling` ·
`44 100 Hz / 48 000 Hz` · `🔍 Finn kanaler med signal (3 sek)` · `📏 Presisjonstest (60 sek)`.
Dypere (avansert-kortet): `ffmpeg`, `DirectShow`, `WASAPI`, `ASIO`, `WASAPI-loopback`.

### 4.4 Innstillinger › Opptak, nivå 1

`MP3 / FLAC / WAV` · `Tapsfri komprimering` · `Ukomprimert` · korttittelen
`MP3-kvalitet` (uten i18n-nøkkel) · **`GRENSE (dBFS)`** med valgene
`-40 dB / -50 dB / -60 dB / -70 dB` · `Forhåndsbuffer` · `Bufferen holder mikrofonen
åpen i bakgrunnen`.

### 4.5 Rediger, nivå 1

`Normaliser lydnivå` · `Justerer toppunktet til −1 dBFS for trygg sluttmiks` ·
`Toppunkt nå −1 dBFS` · **`Mastering (klargjør for publisering)`** ·
`Standardiserer lydstyrke … for podkast og streaming. Bruker EBU R128-normalisering
i to passeringer.` ← appens tetteste enkeltstreng · `Tale-segmenter` / `Musikk-segmenter`.
Runtime under mastring: `Mastrer… (Original: −23,4 LUFS → −16 LUFS)`.

### 4.6 Eksportmodalen — sjargong-episenteret

`Video-kodek` · `H.264 (universell)` / `H.265 (mindre fil)` · `Bitrate` ·
`128/192/256/320 kbps` · **`Bithybde`** _(skrivefeil for «Bitdybde» — sendt i alle sju språk)_ ·
`16-bit (CD-kvalitet)` / `24-bit (studio)` · `Kanalreparasjon` · `Bland til mono` ·
`Mastering (utgivelsesnivå)` med **`−19 / −16 / −14 LUFS`** ×4 ·
`Normalisert (+3,2 dB → −1 dBFS)` · `Volum styres av mastring` · `V −8,4 / H −8,9 dB`.

Og bak `#opt-use-mixer` et helt lydteknikk-bord, **uten i18n i det hele tatt**:
`Lavkutt (HPF)` · `Frekvens 40–200 Hz` · `Støyreduksjon` · `Støygulv −60…−10 dB` ·
`Romdemping (tilnærmet)` · `Gate` med `Terskel`/`Ratio` · `Kompressor` med
`Terskel`/`Ratio`/`Attack`/`Release`/`Makeup` · `De-esser` · `Limiter` med `Tak` ·
`250 Hz (demp mudder)` / `3.5 kHz (nærvær)` / `10 kHz (luft)` · `Sluttgain +2 dB`.

### 4.7 To ordkollisjoner

- **«kanal»** = lydkanal (Lyd-fanen, kanalrutenettet, kanalreparasjon) **og**
  oppdateringskanal (System-fanen). Helt ulike ting, samme ord, samme app.
- **«klipping»** = audio clipping (`Klipper!` på Hjem) **og** å klippe i tid
  (`Klipping og eksport virker som vanlig` i editoren).

---

## 5. Tomtilstander og feiltilstander

### Finnes

| Flate                     | Tomtilstand                                                                                                           | Feiltilstand                                                                       | Scener                                                                                                                                  |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| Hjem                      | Hero «Alt er klart» _(også uten enhet — se §6)_                                                                       | `#hero-warn`, `#preflight-card`, `#missed-card`, global feilstripe, datatap-banner | `home--kald-forstegangs`, `home--enhet-borte`, `home--forhandssjekk`, `home--tapt-opptak`, `home--backend-feil`, `home--kvalitetsalarm` |
| Tidsplan                  | Tom slot-liste + `#cal-hint-card`                                                                                     | «kan ikke vekkes»-tekster i vekkeseksjonen                                         | `schedule--tom`, `schedule--vekking-avansert`                                                                                           |
| Innst. › Lyd              | «Ingen lydenheter funnet»                                                                                             | `#device-ffmpeg-warn`, diagnoserader                                               | `settings-audio--ingen-enheter`, `settings-audio--diagnose`                                                                             |
| Historikk                 | `#search-empty` «ingen opptak ennå» **og** «Ingen opptak i dette filteret» **og** «Ingen treff for …» — tre distinkte | —                                                                                  | `search--tom`, `search--ingen-treff`                                                                                                    |
| Papirkurv                 | _(finnes ikke — se funn 9)_                                                                                           | —                                                                                  | `search--papirkurv-fylt`                                                                                                                |
| Rediger                   | `#editor-empty` med slippsone + siste opptak                                                                          | lastefeil, `#editor-view-popover-empty` («ikke analysert ennå»)                    | `editor--tom`, `editor--feil`                                                                                                           |
| Oppdatering               | «Du er oppdatert»                                                                                                     | «Kunne ikke sjekke for oppdateringer»                                              | `settings-general--oppdatering-feil`                                                                                                    |
| Telemetri-forhåndsvisning | «Ingenting å sende akkurat nå»                                                                                        | «Kunne ikke hente forhåndsvisningen»                                               | `settings-general--telemetri-preview`                                                                                                   |

### Mangler — funn

1. **Innstillinger › Video har ingen tomtilstand for «ingen kamera funnet».**
   Velgeren er bare tom.
2. **Innstillinger › Opptak har ingen tilstand for «ingen mappe valgt».**
   `#save-folder` står tom, og ingenting sier at det blokkerer opptak.
3. **Hjem har ingen tilstand for «ingen lagringsmappe».** Dette er den viktigste
   manglende tomtilstanden i appen: den eneste innstillingen som _garantert_ stopper
   et opptak er også den eneste som ikke har noen synlig mangel-tilstand.
4. **Tidsplan › «Planlagte spesialopptak» har ingen tomtilstand** — bare en tom liste
   under en overskrift.
5. **Historikk › «Flere»-panelet** har ingen tilbakemelding når det ikke finnes noe
   å rydde.
6. **Rediger › Innhold-fanen har ingen tomtilstand og ingen forklaring** — tre tomme
   felter uten kontekst for hva metadataene brukes til (etter #139 brukes de bare i
   sidecar-fila).
7. **`#wake-reliability-card`** er en tom vert som ingenting rendrer inn i lenger.
8. **To døde hendelseskanaler:** `schedule-page.ts` abonnerer på
   `wake-schedule-result` og `test-wake-progress`, som verken finnes i `EVENT_MAP`
   eller `LOCAL_CHANNELS`. Framdriftsvisningen for vekketesten er inert i dag.
9. **Papirkurven har ingen tom-tilstand — fordi inngangen forsvinner.**
   `refreshTrashButton()` i `pages/history.ts` setter `display:none` på
   `#btn-trash-open` når `trash_list` er tom («An empty trash is not a place
   worth offering to visit»), og lukker samtidig `#trash-view` hvis den står
   åpen. Konsekvensen er at «Papirkurv»-lenken kommer og går i grensesnittet
   uten forklaring: en frivillig som slettet noe i går og leter etter det i dag
   finner ingen inngang hvis noen har tømt kurven i mellomtiden. Denne scenen er
   derfor **ikke fotograferbar** og finnes ikke i atlaset.

---

## 6. Hva frivilligen møter først

_Skrevet foran `home--kald-forstegangs` — appen slik den står rett etter installasjon,
uten lydenhet, uten tidsplan, uten opptak. Backend-en har ikke rukket å svare på noe._

Veiviseren spurte om lydenhet (jeg hoppet over — jeg vet ikke hva som er riktig), om
lydtest, om ukentlig opptak, og om jeg ville dele diagnostikk. Så sa den
**«Alt er klart!»**.

Nå står jeg på Hjem. Øverst, i en grønn ramme, med hake:
**«KLAR FOR OPPTAK — Klar, sett opp en tidsplan for å starte automatisk.»**
Nederst i sidepanelet, med grønn prikk: **«Ingen opptak planlagt.»**
De to setningene motsier hverandre, og begge er grønne.

Under står den store røde knappen **«Start opptak»**, og ved siden av **«Video av»**.

Så tre kort:

- **LYDKILDE: «Innebygd mikrofon» — «Tilkoblet», i grønt.** Jeg har aldri valgt en
  lydkilde. Appen har valgt maskinens egen mikrofon for meg og kaller det tilkoblet.
  I dette skjermbildet svarte backend-en med **null lydenheter i det hele tatt** —
  kortet påstår det likevel, fordi koden bak er
  `device?.label ?? t('audio.builtIn')` og `connected = !settings.deviceId || …`.
  Trykker jeg «Start opptak» nå, tar appen opp gudstjenesten på laptop-mikrofonen.
- **FORMAT: «MP3 · 256k» / «Stereo (anbefalt) · 48 kHz».** Jeg vet ikke om det er bra.
  Knappen sier «Endre», så kanskje jeg burde?
- **LAGRING: «250.0 GB ledig» / «Dokumenter/SundayRec · ca. 3 måneder».** Tre måneder
  med hva? Og hvem bestemte at det skulle ligge der?

Midt på siden en stor måler, **«Lydnivå — live»**, med to bjelker (L og R), to «—»
der det pleier å stå tall, og en skala fra «Stille» til «Maks». Ingenting beveger seg.
Ingenting forteller meg hvorfor ingenting beveger seg, eller hva jeg skulle gjort for
å få det til. Under: en lenke som heter **«Test og sjekk system →»** — den fører til
en helt annen side.

Nederst: **«Siste opptak» — «Ingen opptak ennå.»** Det stemmer.

**Det som krever forkunnskap for å komme videre:**
at «lydenhet» betyr miksebordet, ikke maskinens mikrofon, og at appen ikke sier fra
når den bruker feil · at grønt her betyr «ingenting er galt oppdaget», ikke «dette vil
fungere på søndag» · hva en «kanal» er og hvorfor et miksebord har 32 av dem · at
dBFS er negativt og at nær 0 er farlig · at «Format» kan stå i fred · at «Lydnivå —
live» ikke viser noe før en enhet faktisk er åpnet.

**Det appen faktisk sier, tre steder samtidig:** «Klar».

---

## Fotografiske forbehold

Atlaset kjører renderer-en i en vanlig nettleser, uten Tauri-backend. Tre ting ser
derfor litt annerledes ut enn i den ekte appen, og ingen av dem er feil i appen:

- **Editoren viser «Avspilling er ikke tilgjengelig for denne filen»** i alle
  editor-scenene. Lydfila finnes ikke — `convertFileSrc` gir en `asset://`-URL
  ingen nettleser kan laste. Bølgeform, segmenter, kutt og eksport tegnes fra
  fixturer og er ekte.
- **Native OS-dialoger er ikke fotografert.** «Velg mappe», «Åpne fil…»,
  eksport-destinasjon og innstillingsprofil-import/-eksport går gjennom
  `@tauri-apps/plugin-dialog`, som ingen fixtur kan stå i stedet for. De er
  markert som egne trinn i flytene i §3.
- **Kameraforhåndsvisningen er tom.** Selve bildet kommer fra `getUserMedia` i
  webviewen; scenen `home--video-pa` viser rammen og kortene, ikke et videobilde.
- **Atlaset ser Windows.** Playwrights `devices["Desktop Chrome"]` sender en
  Windows-UA (`Mozilla/5.0 (Windows NT 10.0; Win64; x64) …`), og to flater er
  UA-gatet på `/win/i`: `#asio-attribution-card` («Lyd-teknologi», Steinberg-
  attribusjonen på System-fanen) og `#classic-dshow-row` («Klassisk lyd-motor
  (DirectShow)» på Lyd-fanen). Begge er derfor **med** i atlaset selv om en
  macOS-bruker aldri ser dem — nyttig, men verdt å vite før noen teller
  kontroller på et skjermbilde.

Én tilstand er **ikke fotograferbar i det hele tatt**, og det er selve funnet:
den tomme papirkurven (§5, funn 9) — inngangen til den skjuler seg når kurven er
tom.

## Kildehenvisning

- Bilder og scenetabell (arkiv): [`atlas-v015/INDEX.md`](atlas-v015/INDEX.md)
- Konsollfunn under fotograferingen: [`atlas-v015/CONSOLE-FINDINGS.md`](atlas-v015/CONSOLE-FINDINGS.md)
- Scenene som kode, slik de var: `git show fc81919:e2e/atlas/scenes.ts`
- **Dagens atlas:** [`atlas/INDEX.md`](atlas/INDEX.md) · `npm run atlas`
