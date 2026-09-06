> **ARKIVERT 2026-08-30 (V1/PR3).** Revisjonens keep/wire/cut-backlog er
> GJENNOMFØRT: 19 mørke kommandoer slettet, `email_clear_smtp_password` koblet
> opp, og resten begrunnet enkeltvis i PR-teksten. Registeret gikk 111 → 92 og
> unådde 41 → 21. Dokumentet står som HISTORIKK — resonnementet bak hvert
> keep/wire/cut-valg er verdt å kunne lese igjen, men **tallene og tabellene
> under er ikke lenger sanne**.
>
> **Den levende sannheten er `scripts/command-reachability-baseline.json`** +
> `scripts/check-command-reachability.mjs` (kjøres i `npm run check`). De 21
> som står igjen er listet med grunn der de bor: i doc-kommentaren på
> kommandoen selv, og i `docs/APP-SHELL.md` §Etter byttet pkt. 3.

# Kommando-revisjon — august 2026

> **Etterskrift (v0.14):** Direkte-fjerningen tok 16 av kommandoene i denne
> revisjonen ut av registeret: `stream_*` (6), `ndi_*` (5, §4.5 —
> eierbeslutningen falt på _fjern_), `live_bridge_*` (3, §4.6) og
> `start_preview`/`stop_preview`. Tallene og tabellene under er ØYEBLIKKSBILDET
> fra august og oppdateres ikke; `scripts/check-command-reachability.mjs` +
> baselinen er den levende sannheten.
>
> **Etterskrift (R1 «Frivilligen først», 2026-08-23):** delings-klyngen tok
> ytterligere 48 kommandoer ut av registeret — alt som ikke tjener de fire
> kjernejobbene (ta opp · rediger · miks/master · eksporter). Git-historikken
> er feature-flagget. Borte: `cloud_*` (14, §4.1), `integrations_*` (10, §4.2)
>
> - `deeplink_confirm_captions` + `open_in_sundayedit`/`open_in_sundaystudio`
>   (§4.10 — samme overleveringsidé, aldri nådd fra UI), `publish_*` (3, §4.6),
>   `review_*` (7) + `prep_build_episode` + `stage_import_manifest`/
>   `stage_import_apply` (§4.8), `thumbnail_*` (6) + `editor_extract_frame`
>   (§4.10), og `email_test_webhook`. E-post-stien (`email_status`,
>   `email_send_test`, nøkkelring-trioen) BESTÅR — minimal, SMTP-only. Dagens
>   tall: 130 registrert / 115 nådd / 15 unådd (se baselinen).
>
> **Etterskrift (R2 «Frivilligen først», 2026-08-23):** innholds-klyngen tok
> 19 kommandoer til: `whisper_*` (8 — transkripsjon, og med den eneste
> C/C++-avhengigheten i bygget) + `transcripts_list`, `companion_*` (5, §4.9
> — anbefalingen «fjern `companion_llm_status`» falt sammen med hele
> Prekenhjelpen) + `editor_record_companion_suggestion`,
> `editor_detect_chapters`, og `learning_feedback_summary` /
> `learning_local_nudge` / `learning_local_nudge_reset` (visningskortene + den
> lokale justeringen, som mistet sin eneste skriver alt i R1). Ingen nye
> kommandoer. `scheduler::build_opts` er flyttet til `recorder::opts` (ingen
> IPC-endring — `plan_recording_opts` heter det samme). Dagens tall: 111
> registrert / 97 nådd / 14 unådd (se baselinen).

**Hva dette er:** en fullstendig gjennomgang av hver eneste Tauri-kommando appen
registrerer, og svaret på ett spørsmål per kommando: _kan brukergrensesnittet i
det hele tatt kalle den?_

Bakgrunnen er punkt 1 i nattsveipet 2026-08-04 («78 av 162 registrerte
kommandoer er unreachable fra UI — keep/wire/cut-gjennomgang»). Denne revisjonen
gjør den gjennomgangen ferdig, med en målemetode som er dokumentert og kan
gjentas.

Revisjonen endrer **ingen kode**. Den er et beslutningsgrunnlag: hver
unådd kommando får en anbefaling (behold / koble opp / fjern) og, der valget
ikke er teknisk, et 👤-flagg som betyr «eieren må bestemme».

---

## 1. Metode

- **Registeret:** `generate_handler![…]`-lista i `src-tauri/src/lib.rs` er
  fasiten på hva som finnes. Navnene hentes ut mekanisk (én linje per kommando,
  `commands::<område>::<navn>,`).
- **Forbruket:** all produksjonskode i `legacy/` og `src/` (uten `bindings/`,
  `locales/` og `*.test.ts`), med kommentarer strippet vekk først. En kommando
  regnes som **nådd** hvis navnet forekommer som en streng-literal i den
  strippede koden.
- **Hvorfor navnet-som-streng holder:** `legacy/renderer/api-shim.ts` er den
  fila så godt som alle kall går gjennom, så et strengtreff utenfor kommentarer
  er i praksis alltid et kall.
  ⚠️ **Korrigert:** den er _ikke_ den eneste fila som importerer `invoke` fra
  `@tauri-apps/api/core`. `legacy/renderer/deeplinks.ts:32` importerer den også,
  og når `deeplink_confirm_captions` derfra (`deeplinks.ts:129`) — utenom shim-en.
  Påstanden «alle nådde kommandoer går gjennom api-shim» er altså feil med
  nøyaktig én. Målemetoden er upåvirket (den leter etter streng-literaler i hele
  `legacy/`+`src/`, ikke etter kallsteder), men konklusjonen om at shim-en er
  eneste vei inn til backend holder ikke.
- **Kommentar-strippingen er nødvendig, ikke kosmetikk.** Uten den blir
  målingen feil i _optimistisk_ retning: shim-en dokumenterer flere kommandoer
  den ikke kaller (f.eks. `settings_export`, `prep_build_episode`), og et naivt
  søk teller dem som «i bruk».
- **Sammenlikningspunkt:** samme skript kjørt mot `c6325a6` (v0.9.0, grunnlinjen
  for `feat/make-it-real`).

⚠️ **Merk avviket mot 08-04-tallet.** Nattsveipet oppga 78 av 162. Med metoden
over er tallet 60 av 163 på nøyaktig samme kodebase-generasjon. Differansen er
målemetode, ikke kode: 08-04 så etter direkte `invoke("navn")`-kallsteder, og
mistet dermed de kallene der shim-en bruker hjelperne `call()` / `editorCall()`
eller har en generisk typeparameter som selv inneholder en parentes
(`call<import("../bindings/X").X>("navn", …)`). **60 er det riktige tallet.**

---

## 2. Hovedtall

> ⏱️ **Tallene her er et øyeblikksbilde, ikke en løpende sannhet — og de har
> allerede flyttet seg.** `scripts/check-command-reachability.mjs` (§7) måler det
> samme på nytt hver `npm run check` og skriver ut avviket selv: per 2026-08-09
> (etter PR #114–#118) er tallene 194 registrert / 145 nådd / **49 unådd** mot
> tabellens 178/118/60 — integrasjons- og publish-oppkoblingen i #114 flyttet
> en hel gruppe fra «ikke nådd» til «nådd». Skriptet **feiler ikke** på et
> slikt avvik — bare på en ekte regresjon — så tabellen under skal leses som
> «slik så det ut da revisjonen ble gjort». Kjør skriptet for dagens tall.

| Måling                 | v0.9.0 (`c6325a6`) | Nå (`feat/make-it-real`) |
| ---------------------- | -----------------: | -----------------------: |
| Registrerte kommandoer |                163 |                      178 |
| Nådd fra UI            |                103 |                      118 |
| **Ikke nådd fra UI**   |             **60** |                   **60** |
| Andel nådd             |               63 % |                     66 % |

**15 kommandoer kom til i natt, og alle 15 er koblet opp fra dag én:**

`trash_move`, `trash_list`, `trash_restore`, `trash_purge`,
`email_set_smtp_password`, `email_has_smtp_password`, `review_update_trim`,
`review_update_master_preset`, `review_update_jingles`,
`thumbnail_set_default`, `thumbnail_clear_default`,
`thumbnail_get_default_info`, `thumbnail_set_episode`,
`thumbnail_clear_episode`, `thumbnail_resolve`.

**Ingen kobling gikk tapt i natt** (ingen kommando som var nådd i v0.9.0 er
blitt uåpnelig).

### Hva natten faktisk rettet — og hva den ikke rørte

Det er verdt å være presis, for de to tallene over kan lese som «ingenting
skjedde»:

- Natten fjernet **ikke** noe fra 60-lista. De 60 var uåpnelige før og er
  uåpnelige nå. De er stort sett hele funksjonsområder som venter på en
  konto-ID, et SDK eller et UI-prosjekt (se §4).
- Natten fjernet i stedet **den motsatte løgnen**: 15 steder der grensesnittet
  så ferdig ut, men shim-en svarte med en fast verdi fordi det _ikke fantes noen
  Rust-kommando_ bak. De tre `review_update_*`-stubbene, de seks
  `thumbnail_*`-flatene og sletting-uten-papirkurv er nå ekte kommandoer med
  ekte kall. Den kategorien er verre enn en uåpnelig kommando — en uåpnelig
  kommando er død kode ingen ser, en stub er en knapp som lyver til brukeren.
- Natten koblet dessuten opp flere **rene backend-sømmer** som ikke er
  kommandoer og derfor ikke synes i tallene: feilvarslings-utsendelsen
  (`dispatch_failure`), påminnelses-tikket for gjennomgangskøen, og
  `streaming://stats`-hendelsen. Se §5.

---

## 3. Hendelser (emit → listen)

Ikke en kommando-måling, men samme spørsmål stilt til den andre halvdelen av
IPC-sømmen: sender backend hendelser ingen lytter på?

**Nei.** 29 hendelsesnavn sendes fra `src-tauri/src/`, og alle 29 har minst én
lytter i renderer-en. Ingen foreldreløse hendelser.

`backend://warning` · `deeplink://captions` · `deeplink://import` ·
`editor://analysis-progress` · `editor://export-progress` ·
`editor://peaks-progress` · `editor://proxy-progress` · `preview://error` ·
`preview://frame` · `recording://error` · `recording://finished` ·
`recording://levels` · `recording://progress` · `recording://quality` ·
`recording://reconnected` · `recording://reconnecting` · `recording://silence` ·
`recording://started` · `recording://state` · `recording://warning` ·
`scheduler://missed` · `scheduler://next` · `scheduler://preflight` ·
`streaming://stats` · `tray://action` · `vu://levels` ·
`whisper://model-progress` · `whisper://progress`

To av disse var foreldreløse ved starten av natten og ble koblet opp underveis:
`streaming://stats` (ble aldri sendt — Fase 6) og `backend://warning` (fantes
ikke — Fase 2).

---

## 4. De 60 uåpnelige — anbefaling per gruppe

Kolonnen **👤** betyr at anbefalingen krever en eierbeslutning, ikke bare
arbeid.

### 4.1 Sky-lagring — 13 kommandoer · anbefaling: **BEHOLD** · 👤

| Kommando                  | Anbefaling | Begrunnelse                                                                                                                                                                                    |
| ------------------------- | ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cloud_connection_status` | behold     | Hele området venter på én ting: en Google OAuth-klient-ID av typen «Desktop app» (`SUNDAYREC_GOOGLE_CLIENT_ID`, se `docs/archive/GOOGLE-OAUTH-SETUP.md` og punkt 5 i `docs/NEEDS-RICHARD.md`). |
| `cloud_connect`           | behold     | Uten ID-en kan ikke innloggingsvinduet åpnes i det hele tatt.                                                                                                                                  |
| `cloud_cancel_connect`    | behold     | Følger `cloud_connect`.                                                                                                                                                                        |
| `cloud_list_folders`      | behold     | Følger innloggingen.                                                                                                                                                                           |
| `cloud_set_folder`        | behold     | Følger innloggingen.                                                                                                                                                                           |
| `cloud_get_folder`        | behold     | Følger innloggingen.                                                                                                                                                                           |
| `cloud_process_queue_now` | behold     | Køen kjører i backend når kontoen finnes.                                                                                                                                                      |
| `cloud_queue_status`      | behold     | Statuskortet er allerede tegnet, men gatet.                                                                                                                                                    |
| `cloud_enqueue_backup`    | behold     | Følger køen.                                                                                                                                                                                   |
| `cloud_retry_upload`      | behold     | Følger køen.                                                                                                                                                                                   |
| `cloud_remove_upload`     | behold     | Følger køen.                                                                                                                                                                                   |
| `cloud_clear_failed`      | behold     | Følger køen.                                                                                                                                                                                   |
| `cloud_disconnect`        | behold     | Følger innloggingen.                                                                                                                                                                           |

`cloud_is_configured` **er** koblet opp, og det er nettopp det som gjør dagens
tilstand ærlig: Deling-siden spør backend om nøkkelen finnes, og skriver «Ikke
konfigurert i denne bygningen» i stedet for å vise tolv knapper som ikke virker.
Det er riktig oppførsel å beholde inntil ID-en finnes.

**👤 Eierbeslutning:** skaff OAuth-klient-ID-en, eller bestem at sky-lagring
utgår fra produktet. Halvveis er ikke et alternativ som koster noe i dag, men
det er 13 kommandoer og et helt UI som vedlikeholdes uten å brukes.

### 4.2 Sunday-suite-integrasjoner — 10 kommandoer · ✅ **LØST i PR #114 (2026-08-09)**

> **Denne gruppa er koblet opp.** PR #114 byttet stubbene i `api-shim.ts` mot
> ekte `invoke`-kall for alle ti kommandoene under: panelet lagrer nå gjennom
> `integrations_get/set_settings`, API-nøkkelen når nøkkelringen, og
> kvitteringene er ærlige («Lagret ✓» først etter at IPC-en svarte; en feilet
> lagring viser grunnen). Renderer-halvdelen ble pinnet i
> `e2e/integrations.spec.ts`, og alle ti forlot `unreachable`-settet i
> `scripts/command-reachability-baseline.json`. Tabellen under står som
> beslutningsgrunnlaget slik det så ut FØR #114 — les «Anbefaling» som
> historikk, ikke som gjenstående arbeid. **(R1 2026-08-23: hele gruppa — og
> spec-en — er fjernet; se etterskriftet øverst.)**

| Kommando                           | Anbefaling | Begrunnelse                                                                                                                                                                                                                                                                                                                              |
| ---------------------------------- | ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `integrations_get_settings`        | koble opp  | **Siden finnes allerede** (`pages/integrations-page.ts`, «Sunday-suite» under System → Avansert) og dokumenterer i sin egen filhode at den lagrer gjennom `window.api.get/setIntegrationSettings`. Men de shim-metodene er stubber (`async () => ({ enabled: false })`). Bryteren står altså der og lar seg skru på, uten at noe lagres. |
| `integrations_set_settings`        | koble opp  | Samme stub. Dette er samme feilklasse som de tre `review_update_*`-løgnene Fase 3 rettet.                                                                                                                                                                                                                                                |
| `integrations_get_service_link`    | koble opp  | Stub returnerer `null`.                                                                                                                                                                                                                                                                                                                  |
| `integrations_song_set_apikey`     | koble opp  | Stub returnerer `true` — «lagret» uten å lagre.                                                                                                                                                                                                                                                                                          |
| `integrations_song_has_apikey`     | koble opp  | Stub returnerer `false`.                                                                                                                                                                                                                                                                                                                 |
| `integrations_song_submit_usage`   | koble opp  | Stub returnerer `{ ok: false }`. NETTVERK-UVERIFISERT i backend.                                                                                                                                                                                                                                                                         |
| `integrations_plan_fetch_services` | koble opp  | Stub returnerer `[]`.                                                                                                                                                                                                                                                                                                                    |
| `integrations_plan_update_service` | koble opp  | Stub returnerer `{ ok: false }`.                                                                                                                                                                                                                                                                                                         |
| `integrations_sundayedit_send`     | koble opp  | Stub returnerer `{ ok: false }`.                                                                                                                                                                                                                                                                                                         |
| `integrations_sundayedit_import`   | koble opp  | Stub returnerer `{ ok: false }`.                                                                                                                                                                                                                                                                                                         |

**Dette VAR den høyest prioriterte gruppa i hele revisjonen** — et fullt
synlig, klikkbart panel over stubber. Det uakseptable mellomstadiet er borte:
shim-metodene er ekte siden PR #114, og HTTP-sidene av kommandoene står igjen
som det de alltid var — NETTVERK-UVERIFISERT til en rigg med søsterappene
prøver dem (se `SMOKE-TEST.md` §P2b).

### 4.3 Innstillinger — 6 kommandoer · anbefaling: **DELT**

| Kommando                    | Anbefaling | Begrunnelse                                                                                                                                                                                                                                                                |
| --------------------------- | ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `settings_get`              | behold     | Renderer-en leser innstillinger fra `localStorage` (`loadSettings()`) og _skriver_ til backend med `settings_save`. Det er med vilje etter Fase 1, men gjør lese-kommandoen unødvendig. Behold som eneste maskinlesbare vei til sannheten (diagnostikk, framtidig import). |
| `settings_reset`            | 👤 avgjør  | UI-et ble fjernet i v4.31 (se kommentaren i `pages/general-page.ts` ~L125).                                                                                                                                                                                                |
| `settings_export`           | 👤 avgjør  | Samme.                                                                                                                                                                                                                                                                     |
| `settings_import`           | 👤 avgjør  | Samme.                                                                                                                                                                                                                                                                     |
| `settings_export_to_file`   | 👤 avgjør  | Samme.                                                                                                                                                                                                                                                                     |
| `settings_import_from_file` | 👤 avgjør  | Samme.                                                                                                                                                                                                                                                                     |

**👤 Eierbeslutning:** «sikkerhetskopi av innstillinger» — kom tilbake eller ut?
For en frivillig som setter opp maskinen én gang i året er eksport/import en
reell verdi (bytte av PC, andre kirke i samme menighet). Kommandoene er ferdige;
det er tre knapper som mangler. Alternativet er å slette de fem kommandoene.

⚠️ Se også §6: rapporten fra 08-05 påstår at sikkerhetskopi «er wiret». Det
stemmer ikke i koden.

### 4.4 Sunday Account (SSO) — 5 kommandoer · anbefaling: **BEHOLD** · 👤

`sunday_account_configured` · `sunday_account_status` · `sunday_sign_in` ·
`sunday_sign_out` · `sunday_whoami_song`

Det finnes ingen innloggingsflate i SundayRec. Backend-siden er den delte
Sunday Account-kontrakten som resten av suiten bruker. Å koble opp krever et
UI-prosjekt (innloggingsskjerm, kontokort, utloggingsvei), ikke en shim-metode.

**👤 Eierbeslutning:** skal SundayRec ha Sunday Account-innlogging i det hele
tatt? Appen er i dag en ren lokal maskin-app; SSO gir først mening sammen med
sky-lagring eller integrasjonene over.

### 4.5 NDI — 5 kommandoer · anbefaling: **BEHOLD (stub)** · 👤

`ndi_list_sources` · `ndi_start_receiver` · `ndi_output_runtime_available` ·
`ndi_output_start` · `ndi_output_stop`

Bak `ndi`-feature-et (som ikke er i `default`), og implementasjonen er selv
merket STUB — den ekte NDI-runtimen krever NewTek-SDK-et (`dep:libloading`
laster det dynamisk). Ingen UI, og det er riktig: en NDI-knapp uten SDK er en
knapp som ikke kan virke.

**👤 Eierbeslutning:** trenger riggen NDI? Hvis nei, er dette den reneste
kandidaten for _fjerning_ i hele revisjonen — 5 kommandoer, ett feature-flagg og
en valgfri avhengighet forsvinner.

### 4.6 Podkast-publisering (RSS) — 3 kommandoer · anbefaling: **BEHOLD**

`publish_feed_status` · `publish_feed_preview` · `publish_generate_feed`

Bak `publish`-feature-et (ikke i `default`). Deling-siden finnes, men
feed-delen er portet vekk. Beholdes fordi den henger sammen med sky-lagring
(feed-en må lastes opp et sted) — bestem den sammen med §4.1.

### 4.7 Live-bro (SundayStage cue → kapittel) — 3 kommandoer · anbefaling: **BEHOLD**

`live_bridge_status` · `live_bridge_channel` · `live_bridge_map_event`

Bak `bridge`-feature-et (WebSocket, ikke i `default`). Den _manuelle_ veien til
samme resultat — «↧ Stage-kapitler»-knappen i editoren — finnes og er koblet
opp (`pages/editor/stage-ui.ts`), men den bruker `window.api.stageImport`, som
er en stub (se §4.8). Live-broen er sanntidsvarianten av det samme og bør
avgjøres etter at den manuelle importen virker.

### 4.8 Gjennomgangskø og episodeklargjøring — 3 kommandoer · anbefaling: **DELT**

| Kommando                   | Anbefaling | Begrunnelse                                                                                                                                                                                                                                                      |
| -------------------------- | ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `review_process_reminders` | behold     | **Ikke lenger død funksjonalitet.** Fase 3 la inn en egen timer i backend (`src-tauri/src/notify/reminders.rs`, 45 s + hver time) som kjører nøyaktig samme eskaleringsstige. Kommandoen er nå bare en manuell inngang som ingen trenger. Behold for feilsøking. |
| `prep_build_episode`       | 👤 avgjør  | Episodeklargjøring fra analysesegmenter. Ingen flate. Køen fylles i dag fra editoren i stedet.                                                                                                                                                                   |
| `stage_import_manifest`    | koble opp  | «↧ Stage-kapitler»-knappen finnes i markup og har en klikk-lytter, men `window.api.stageImport` er stubben `async () => ({ ok: false })`. Nok en knapp som er garantert å mislykkes.                                                                             |

`stage_import_manifest` hører sammen med §4.2 — det er samme feilklasse
(synlig knapp, stub under), bare i en annen del av appen.

### 4.9 Erstattet av noe bedre — 6 kommandoer · anbefaling: **FJERN**

| Kommando                 | Anbefaling | Begrunnelse                                                                                                                                                  |
| ------------------------ | ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `list_video_devices`     | fjern      | Ren delmengde av `list_devices` (som er koblet opp og returnerer `DeviceInventory` med både lyd og video), med samme 1,5 s-cache.                            |
| `list_recording_devices` | fjern      | Erstattet av `list_audio_devices` / `list_devices` fra `audio::device_enum`. Kommandoens egen doc-kommentar peker på etterfølgeren.                          |
| `recording_status`       | fjern      | Erstattet av `recording://state`-hendelsen, som 8 filer lytter på. Å spørre synkront om en tilstand som pushes er en kilde til uenighet mellom to sannheter. |
| `setting_get`            | fjern      | Generisk nøkkel/verdi-inngang til `settings`-tabellen. Den typede `settings_get`/`settings_save`-veien er den appen bruker.                                  |
| `setting_set`            | fjern      | Samme. En utypet skrivevei inn i innstillingstabellen er dessuten den slags dør man ikke vil ha stående åpen.                                                |
| `companion_llm_status`   | fjern      | `companion_llm_configured` (koblet opp) svarer på det UI-et faktisk spør om.                                                                                 |

Ingen av disse haster. Poenget med å notere dem er at de er de eneste seks der
svaret er entydig teknisk og ikke krever eieren.

### 4.10 Resten — 6 kommandoer

| Kommando                    | Område    | Anbefaling | Begrunnelse                                                                                                                                                                                                                                                                                   |
| --------------------------- | --------- | ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `scheduler_check_missed`    | scheduler | behold     | Var det klassiske eksempelet på et sikkerhetsnett ingen dro i. Nattsveipet 08-04 koblet det opp **i backend** (oppstart + etter mistenkt dvale, `scheduler/mod.rs`). Kommandoen er nå en manuell inngang, ikke et hull.                                                                       |
| `editor_extract_frame`      | editor    | 👤 avgjør  | Skulle drive «Hent bilde fra video» i episodebilde-panelet. Fase 6 hoppet over strekket på grunn av to reelle blokkere. Se §5 og chip-en fra i natt.                                                                                                                                          |
| `editor_cleanup_temp_files` | editor    | koble opp  | Ryddekommando for editorens midlertidige filer. Ingen kaller den; ryddingen skjer i stedet ad hoc. En eksplisitt opprydding ved appstart er billig og trygg.                                                                                                                                  |
| `liturgical_month`          | calendar  | 👤 avgjør  | Kirkeårets høytider for en måned, klar til å tegnes som blå «hoy»-hendelser i Tidsplan-kalenderen. Regnestykket (computus) er ferdig og enhetstestet i `sundayrec-core::church_calendar`. Kalendersiden nevner ikke liturgi med et ord. Dette er den billigste _nye_ funksjonen i hele lista. |
| `open_in_sundayedit`        | bridge    | 👤 avgjør  | Overlevering til søsterappen. Overlapper med `integrations_sundayedit_send`; avgjør sammen med §4.2 hvilken av de to som er veien videre.                                                                                                                                                     |
| `open_in_sundaystudio`      | bridge    | 👤 avgjør  | Samme.                                                                                                                                                                                                                                                                                        |

---

## 5. Hva som ble koblet opp i natt (for ordens skyld)

Kommandotallene i §2 fanger ikke alt arbeidet, fordi mye av det som var «bygget
og aldri koblet opp» ikke var kommandoer. Kort liste, med fasen som gjorde det:

| Søm som var død                                         | Fase | Nå                                                              |
| ------------------------------------------------------- | ---- | --------------------------------------------------------------- |
| E-postsending (`email`-feature ikke i noen bygg)        | P1   | I `default` + begge release-listene                             |
| Kirke-/e-post-/webhook-innstillinger nådde aldri sqlite | P1   | Skrives gjennom `settings_save` (kritisk funn, se rapporten)    |
| Feil som ikke varslet noen                              | P2   | `dispatch_failure`-søm: native + e-post + webhook               |
| `backend://warning`                                     | P2   | Ny hendelse, med lytter                                         |
| Fem stille feilklasser                                  | P2   | Snakker (preroll, sky, gjenoppretting, enhet borte, lite plass) |
| Tre `review_update_*`-stubber                           | P3   | Ekte kommandoer, ekte kall                                      |
| Påminnelsesstigen (24 t / 48 t / 7 d / 14 d)            | P3   | Egen timer i `notify/reminders.rs`                              |
| `streaming://stats` ble aldri sendt                     | P6   | 1 Hz + overgangsemits + hale-push                               |
| Seks `thumbnail_*`-stubber                              | P6   | Ekte kommandoer, ekte kall                                      |
| Sletting uten angremulighet                             | P7   | Fire `trash_*`-kommandoer, alle koblet opp                      |

---

## 6. Uoverensstemmelser funnet under sveipen (ikke rettet)

Notert her fordi de er dokumentasjonsfeil, ikke kodefeil — og fordi de vil
forvirre neste person som leser:

1. **`SundayRec-UX-NIGHT-2026-08-05.md` linje 78** påstår «Sikkerhetskopi av
   innstillinger (eksport/import/nullstill) wiret». Det finnes ingen referanse
   til `settings_export`, `settings_import`, `settings_reset`,
   `settings_export_to_file` eller `settings_import_from_file` noe sted i
   renderer-koden, og `pages/general-page.ts` ~L125 sier eksplisitt at
   knappene ble fjernet i v4.31 og at shim-metodene ble slettet i
   «2026-08-audit». Rapporten er feil på dette punktet.
2. **`pages/general-page.ts` ~L132** sier at «btn-test-email / btn-test-webhook
   har ingen fungerende backend ennå» og at begge «er deaktivert i markup med en
   ærlig begrunnelse». Etter Fase 1 stemmer ikke det lenger: `disabled` står
   fortsatt i `index.html`, men koden lenger ned i samme fil (~L272 og ~L339)
   skrur knappene på ved kjøretid og kobler dem til `email_send_test` /
   `email_test_webhook`. Kommentaren er utdatert.
3. **`docs/NEEDS-RICHARD.md`** sto med «`email` er ikke i default-bygget»-
   antakelsen flere steder. Rettet i samme runde som denne revisjonen.
4. **Tallet 78 fra nattsveipet 08-04** er en målemetodefeil, ikke en endring i
   koden. Se §1.

---

## 7. Slik gjentar du målingen

**Det ER et skript i repoet:** `scripts/check-command-reachability.mjs`. Det
kjører nøyaktig oppskriften under, og er en obligatorisk port — `npm run
reachability` inngår i `npm run check` og i CI (`.github/workflows/ci.yml`,
steget «Command reachability regression check»). Det feiler på en _regresjon_
(en kommando som var nådd, ikke er det lenger) eller på en nyregistrert
kommando ingen har klassifisert — ikke på tallene i seg selv.

```bash
npm run reachability                  # sjekk mot grunnlinja
npm run reachability:write-baseline   # regenerer grunnlinja fra treet
```

Oppskriften det følger, om du vil gjøre det for hånd:

1. Hent kommandonavnene ut av `generate_handler![…]` i `src-tauri/src/lib.rs` —
   én per linje på formen `commands::<område>::<navn>,`.
2. Les all `.ts`/`.js` under `legacy/` og `src/`, hopp over `bindings/`,
   `locales/` og `*.test.ts`.
3. Strip blokk- og linjekommentarer **før** du søker. Uten dette blir svaret
   for optimistisk.
4. En kommando er nådd hvis navnet finnes som streng-literal (`"navn"`,
   `'navn'` eller `` `navn` ``) i resten.

Samme oppskrift på hendelser: `grep` etter `"…://…"`-literaler i
`src-tauri/src/` for senderne, og etter de samme strengene i `legacy/` for
lytterne.

---

## 8. Vedlegg — de 118 nådde kommandoene i denne målingen

De 118 radene under går alle gjennom `legacy/renderer/api-shim.ts`. Det er
**ikke** fordi shim-en er den eneste fila som importerer `invoke` — se §1:
`legacy/renderer/deeplinks.ts` gjør det også, og `deeplink_confirm_captions`
nås derfra. Den kommandoen mangler i tabellen under; lista er altså den
api-shim-baserte delmengden, ikke fasiten på hva som er nåbart. Kjør
`npm run reachability` for det gjeldende tallet. Kolonnen «kalt fra» viser hvor
strengen står; `(+N)` betyr at N andre filer også nevner navnet (typisk en
side som logger eller tester rundt kallet).

| Kommando                        | Område      | Kalt fra    |
| ------------------------------- | ----------- | ----------- |
| `app_info`                      | app         | api-shim.ts |
| `set_launch_at_login`           | app         | api-shim.ts |
| `get_launch_at_login`           | app         | api-shim.ts |
| `tray_set_language`             | app         | api-shim.ts |
| `list_input_devices`            | audio       | api-shim.ts |
| `list_audio_devices`            | audio       | api-shim.ts |
| `probe_device_channels`         | audio       | api-shim.ts |
| `scan_device_channels`          | audio       | api-shim.ts |
| `list_audio_input_channels`     | audio       | api-shim.ts |
| `list_devices`                  | audio       | api-shim.ts |
| `get_camera_capabilities`       | audio       | api-shim.ts |
| `diagnose_audio`                | audio       | api-shim.ts |
| `start_vu`                      | audio       | api-shim.ts |
| `stop_vu`                       | audio       | api-shim.ts |
| `ffmpeg_health`                 | media       | api-shim.ts |
| `start_preview`                 | media       | api-shim.ts |
| `stop_preview`                  | media       | api-shim.ts |
| `media_permissions`             | media       | api-shim.ts |
| `recording_preview_frame`       | recorder    | api-shim.ts |
| `plan_recording_opts`           | recorder    | api-shim.ts |
| `start_recording`               | recorder    | api-shim.ts |
| `stop_recording`                | recorder    | api-shim.ts |
| `recording_scheduled_stop_ms`   | recorder    | api-shim.ts |
| `recording_extend_autostop`     | recorder    | api-shim.ts |
| `recording_cancel_autostop`     | recorder    | api-shim.ts |
| `preroll_start`                 | recorder    | api-shim.ts |
| `preroll_stop`                  | recorder    | api-shim.ts |
| `preroll_status`                | recorder    | api-shim.ts |
| `get_disk_space`                | recorder    | api-shim.ts |
| `run_test_recording`            | recorder    | api-shim.ts |
| `run_capture_bench`             | recorder    | api-shim.ts |
| `recordings_list`               | db          | api-shim.ts |
| `transcripts_list`              | db          | api-shim.ts |
| `recordings_delete`             | db          | api-shim.ts |
| `recordings_clear`              | db          | api-shim.ts |
| `recording_update_note`         | db          | api-shim.ts |
| `recordings_prune`              | db          | api-shim.ts |
| `trash_move`                    | trash       | api-shim.ts |
| `trash_list`                    | trash       | api-shim.ts |
| `trash_restore`                 | trash       | api-shim.ts |
| `trash_purge`                   | trash       | api-shim.ts |
| `cloud_is_configured`           | cloud       | api-shim.ts |
| `settings_save`                 | settings    | api-shim.ts |
| `run_preflight`                 | diagnostics | api-shim.ts |
| `run_diagnostics`               | diagnostics | api-shim.ts |
| `haptic_perform`                | haptics     | api-shim.ts |
| `editor_load_recording`         | editor      | api-shim.ts |
| `editor_peaks`                  | editor      | api-shim.ts |
| `editor_extract_playback_proxy` | editor      | api-shim.ts |
| `editor_allow_asset_path`       | editor      | api-shim.ts |
| `editor_probe_peak`             | editor      | api-shim.ts |
| `editor_segments`               | editor      | api-shim.ts |
| `editor_master_presets`         | editor      | api-shim.ts |
| `editor_detect_chapters`        | editor      | api-shim.ts |
| `editor_diagnose_channels`      | editor      | api-shim.ts |
| `editor_auto_process`           | editor      | api-shim.ts |
| `editor_mastering_analyze`      | editor      | api-shim.ts |
| `editor_export`                 | editor      | api-shim.ts |
| `editor_cancel_export`          | editor      | api-shim.ts |
| `editor_read_sidecar`           | editor      | api-shim.ts |
| `editor_write_sidecar`          | editor      | api-shim.ts |
| `editor_delete_sidecar`         | editor      | api-shim.ts |
| `editor_probe_streams`          | editor      | api-shim.ts |
| `editor_read_file`              | editor      | api-shim.ts |
| `editor_master_preview`         | editor      | api-shim.ts |
| `editor_master_apply`           | editor      | api-shim.ts |
| `editor_master_cancel`          | editor      | api-shim.ts |
| `email_status`                  | email       | api-shim.ts |
| `email_send_test`               | email       | api-shim.ts |
| `email_test_webhook`            | email       | api-shim.ts |
| `email_clear_smtp_password`     | email       | api-shim.ts |
| `email_set_smtp_password`       | email       | api-shim.ts |
| `email_has_smtp_password`       | email       | api-shim.ts |
| `scheduler_reschedule`          | scheduler   | api-shim.ts |
| `scheduler_status`              | scheduler   | api-shim.ts |
| `wake_capabilities`             | wake        | api-shim.ts |
| `wake_get_sleep_config`         | wake        | api-shim.ts |
| `wake_fix_sleep`                | wake        | api-shim.ts |
| `wake_verify`                   | wake        | api-shim.ts |
| `wake_reschedule`               | wake        | api-shim.ts |
| `wake_test`                     | wake        | api-shim.ts |
| `wake_cancel_test`              | wake        | api-shim.ts |
| `wake_failure_history`          | wake        | api-shim.ts |
| `wake_clear_failure_history`    | wake        | api-shim.ts |
| `whisper_list_models`           | whisper     | api-shim.ts |
| `whisper_model_status`          | whisper     | api-shim.ts |
| `whisper_download_model`        | whisper     | api-shim.ts |
| `whisper_cancel_download`       | whisper     | api-shim.ts |
| `whisper_delete_model`          | whisper     | api-shim.ts |
| `whisper_transcribe`            | whisper     | api-shim.ts |
| `whisper_cancel_transcribe`     | whisper     | api-shim.ts |
| `whisper_export_transcript`     | whisper     | api-shim.ts |
| `companion_build`               | companion   | api-shim.ts |
| `companion_llm_configured`      | companion   | api-shim.ts |
| `companion_set_llm_key`         | companion   | api-shim.ts |
| `companion_clear_llm_key`       | companion   | api-shim.ts |
| `review_queue_list`             | review      | api-shim.ts |
| `review_mark_published`         | review      | api-shim.ts |
| `review_mark_discarded`         | review      | api-shim.ts |
| `review_update_trim`            | review      | api-shim.ts |
| `review_update_master_preset`   | review      | api-shim.ts |
| `review_update_jingles`         | review      | api-shim.ts |
| `stream_status`                 | streaming   | api-shim.ts |
| `stream_start`                  | streaming   | api-shim.ts |
| `stream_stop`                   | streaming   | api-shim.ts |
| `stream_preview_path`           | streaming   | api-shim.ts |
| `stream_set_key`                | streaming   | api-shim.ts |
| `stream_delete_key`             | streaming   | api-shim.ts |
| `thumbnail_set_default`         | thumbnail   | api-shim.ts |
| `thumbnail_clear_default`       | thumbnail   | api-shim.ts |
| `thumbnail_get_default_info`    | thumbnail   | api-shim.ts |
| `thumbnail_set_episode`         | thumbnail   | api-shim.ts |
| `thumbnail_clear_episode`       | thumbnail   | api-shim.ts |
| `thumbnail_resolve`             | thumbnail   | api-shim.ts |
| `update_status`                 | update      | api-shim.ts |
| `update_check`                  | update      | api-shim.ts |
| `update_download_install`       | update      | api-shim.ts |
| `update_relaunch`               | update      | api-shim.ts |
