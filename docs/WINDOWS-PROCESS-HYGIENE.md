# Windows prosess-hygiene — hindre at lyd-tjenesten krasjer

## Bakgrunn (diagnose fra kirkemaskinen)

En diagnose av en faktisk kirke-PC (Windows 11) viste at **Windows Audio-tjenesten
(`Audiosrv`) krasjet gjentatte ganger** — og når den faller forsvinner ALLE
lydenheter samtidig (symptomet brukerne melder som «enheten dukker ikke opp»).
Samtidig kjørte **6 `SundayRec.exe`-prosesser** (Soundcraft-driveren selv var frisk).

Rot-årsak: appen tillot ubegrenset antall instanser, og force-quit/hengte instanser
etterlot ffmpeg-sidecars som fortsatt holdt lydenheten. Flere instanser + orphans
som samtidig holder lyd-ressurser presset Windows Audio-tjenesten til den krasjet.

## Det som er implementert

| Fiks                                                           | Hva                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | Fil                                                                         |
| -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------- |
| **Single-instance**                                            | En ny oppstart fokuserer det eksisterende vinduet i stedet for å starte en ny prosess. Registrert som FØRSTE plugin (Tauri-krav).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | `lib.rs` (`tauri-plugin-single-instance`)                                   |
| **Job Object (kill-on-close)**                                 | Prosessen legges ved oppstart i et Windows Job Object med `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Når SundayRec dør av HVILKEN SOM HELST grunn (inkl. hard kill via Oppgavebehandling) dreper OS alle ffmpeg-barn automatisk.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | `platform/mod.rs` (`windows-sys`)                                           |
| **Exit-opprydding**                                            | Ved app-avslutning stoppes recorder/preview/VU-sidecars eksplisitt (graceful komplement til Job Object).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | `lib.rs` `RunEvent::ExitRequested`                                          |
| **Lukk ≠ avslutt under opptak**                                | Lukkeknappen SKJULER vinduet mens et opptak går eller finaliseres (appen lever videre i systemstatusfeltet). Sidecar-hygienen er uendret: en ekte avslutning går fortsatt gjennom `ExitRequested`, og Job Object-et gjelder uansett hvordan prosessen dør.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | `window.rs` (`CloseRequested`)                                              |
| **Avslutt under opptak**                                       | Første «Avslutt» (systemstatusfeltet, eller vindusbehandlerens avslutning) nektes mens et opptak går; andre trykk innen 10 s stopper opptaket og VENTER på at fila lander før prosessen dør (tak: `QUIT_WAIT_CAP_MS`). Sidecar-oppryddingen over kjører uendret på den avslutningen som til slutt går gjennom. Et tredje trykk avslutter umiddelbart.                                                                                                                                                                                                                                                                                                                                                                                                                    | `window.rs` (`request_quit`)                                                |
| **Nedlasting sperret, installasjon etter stopp** (F2-W1, #243) | «Last ned og installer» er sperret på KNAPPE-NIVÅ mens et opptak går (grå, med begrunnelse) — `download_allowed` avviser med `recording_in_progress` i `Preparing`/`Recording`/`Reconnecting`/`Stopping`, på alle plattformer. Er en oppdatering alt lastet ned (eller på Windows: bytene ligger klare til installasjon) når opptaket starter, venter selve omstarten til opptaket er stoppet og fila er skrevet ferdig (`relaunch_plan` → `arm_wait_then` → `relaunch_now`) før prosessen byttes ut. Omstarten kan ikke angres etterpå — `prevent_exit` er en no-op for `RESTART_EXIT_CODE` — så ventingen skjer først. På Windows disarmes Job Object-ets kill-on-close RETT FØR installeren startes (før #243 tok installeren seg selv i fallet, se DISTRIBUTION.md). | `update/mod.rs` (`relaunch`), `sundayrec-core::update` (`download_allowed`) |
| **`kill_on_drop`** (fra før)                                   | Ren nedstenging dreper ffmpeg når `Child` droppes.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | `media/ffmpeg.rs`                                                           |
| **dshow → WASAPI/ASIO**                                        | Lyd fanges nå via cpal (WASAPI standard / ASIO pro), ikke dshow — se [`PRO-AUDIO-WINDOWS.md`](./PRO-AUDIO-WINDOWS.md). Lydenheten holdes in-process og slippes ved stopp. Lyd-only skriver rett til WAV (ingen ffmpeg-pipe); cpal→ffmpeg-pipen lever videre kun for video-økter og klassisk-hatchen.                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | `recorder/native_capture/` (lyd), `recorder/cpal_capture.rs` (video)        |

## Bevisst utsatt

- **Oppstarts-opprydding av gamle orphans (FIKS 4):** ~~ikke implementert~~ —
  **implementert på macOS/Linux i v0.4.4** etter rigg-hendelsen 2026-07-31 (en
  krasjet instans lot en ffmpeg ta opp rommet i 12+ min; mac har ingen Job
  Object). To mekanismer i `platform/mod.rs` (`unix_imp`): en **oppstarts-sweep**
  (kjøres etter single-instance-gaten, før crash-recovery) og en **frakoblet
  reaper** som dreper sidecars i det appen dør — uansett dødsårsak, SIGKILL
  inkludert. «Risikabelt å drepe ved navn»-innvendingen er løst ved at begge KUN
  matcher den absolutte stien til VÅR bundlede ffmpeg/ffprobe (ERE-escapet, med
  klasse-innpakket sistetegn så mønsteret aldri matcher sin egen bærer-prosess);
  en bar `ffmpeg` fra PATH nektes. På Windows dekker Job Object fortsatt alt —
  sweep/reaper er no-op der.

## Testplan (Windows — må bestås før release)

- [ ] **Single-instance:** dobbeltklikk ikonet 5× raskt → KUN én `SundayRec.exe` i
      Oppgavebehandling; det eksisterende vinduet fokuseres. Logg: «a second SundayRec
      launch was blocked».
- [ ] **Ren lukking:** lukk appen normalt → ingen `SundayRec.exe` eller `ffmpeg.exe`
      blir liggende.
- [ ] **Hard kill midt i opptak:** start opptak → drep `SundayRec.exe` via
      Oppgavebehandling → INGEN `ffmpeg.exe` blir liggende (Job Object gjør jobben).
      Logg ved oppstart: «process placed in kill-on-close Job Object».
- [ ] **Start → stopp → start:** to påfølgende opptak → lydenheten er ledig mellom,
      ingen lås.
- [ ] **60+ min (gudstjeneste-lengde):** kjør et langt opptak → Event Viewer
      (System-logg) skal ha NULL nye «Windows Audio ble uventet avbrutt».
- [ ] **Diagnose på nytt:** kun ÉN SundayRec-prosess, ingen Audio-tjeneste-krasj.

## Changelog-tekst

> Rettet en feil der flere SundayRec-instanser eller etterlatte opptaksprosesser
> kunne overbelaste Windows' lydtjeneste og føre til at lydkortet «forsvant».
> SundayRec kjører nå som én enkelt instans og rydder alltid opp opptaksprosesser
> ved avslutning.
