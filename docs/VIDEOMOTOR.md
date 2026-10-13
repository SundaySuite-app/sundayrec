# Videomotoren — lyden først, kameraet som et tillegg

_Skrevet 2026-10-11, lest mot koden på nytt 2026-10-12 ved v0.25.3. Status:
**plan, ikke startet.** Del av
[OMBYGGING.md](OMBYGGING.md) (pakkene C0–C5). Eierens beslutning: gjennomføres,
med store grep og høy risiko, før appen rulles ut bredt._

Videoopptaket har ikke vært godt nok. Denne planen bytter ut hele måten video
tas opp på: **Rust eier kameraet, lydkortet og klokka. ffmpeg koder bare
videoen.** Lyden går alltid gjennom den native lydmotoren, den samme som rene
lydopptak har brukt siden 2026-08-01. Et kamera som faller ut, kan aldri ta
lyden med seg.

## Hvorfor: diagnosen

1. **Fire videostier med ulike evner.**

   | Sti                                                            | Gjenkobling | Oppdeling | Pre-roll | Stopp ved stillhet |
   | -------------------------------------------------------------- | ----------- | --------- | -------- | ------------------ |
   | Mac: ffmpeg `avfoundation` tar bilde og lyd (`run_session`)    | ✅          | ✅        | ✅       | ✅                 |
   | Windows: `dshow`-bilde + cpal-lyd i et rør (`cpal_capture.rs`) | ❌          | ❌        | ❌       | ❌                 |
   | Windows: `dshow` for begge (når funksjonene over trengs)       | ✅          | ✅        | ✅       | ✅                 |
   | Reserve med to prosesser (`two_process.rs`)                    | ❌          | ❌        | ❌       | ❌                 |

2. **Videoøkter på Mac bruker lyden fra `avfoundation`.** Det er nettopp den
   lyden `native_capture/mod.rs` dokumenterer at ffmpeg mister samples fra:
   5,4 % på den innebygde mikrofonen og 55–100 % på en Qu-5. Rene lydopptak
   ble flyttet bort fra den. Videoopptak ble det ikke.
3. **Lyden er gissel for kameraet.** Bilde og lyd tas opp av samme
   ffmpeg-prosess. En USB-hikke i kameraet dreper prosessen, og lyden
   forsvinner til gjenkoblingen har startet en ny.
4. **Windows koder alltid i programvare.** `push_video_encoder_args` i
   `capture.rs` velger maskinvare bare når `hw_accel && macOS`; ellers faller
   den til `libx264 veryfast` — altså på Windows alltid, og også på Mac når
   maskinvarekoding er slått av. Windows-bygget
   av ffmpeg som følger med (gyan.dev essentials 9.0.1, kontrollert 2026-10-11)
   har `h264_nvenc`, `h264_qsv`, `h264_amf` og `h264_mf`. På en eldre
   kirke-PC gir 1080p30 i programvare høy CPU-last, tapte bilder og hakking.
5. **Kamerainngangene i ffmpeg er svake.** `avfoundation` adresserer kameraet med
   et nummer som kan endre seg mellom oppslag og start. Modusene finnes bare
   ved å be om en ugyldig modus og lese feilmeldingen (`media/camera.rs`).
   `dshow` er et gammelt API. Synkroniseringen skjer inne i ffmpeg
   (`-fps_mode cfr`, `aresample=async`) over to klokker. På Windows får
   lyd-røret tidsstempel når dataene kommer fram. Leppesynk over en hel
   gudstjeneste er fortsatt ubekreftet på Windows (RIG-DAY w6).
6. **Kameraet bytter eier ved start.** Forhåndsvisningen er webviewets
   `getUserMedia` (`LiveCameraPreview.tsx`). Den må slippe kameraet før ffmpeg
   åpner det, og vente 3 s etter opptaket før den tar det tilbake. To
   systemer med hvert sitt navn på kameraet betyr at det som vises og det som
   tas opp kan være forskjellige kameraer.

## Målbildet

```text
 lydkort ─ cpal ─▶ native lydmotor (finnes) ─▶ WAV  (krasjsikker; gjenkobling, oppdeling, pre-roll, stillhet)
                         │ lydklokka: (sample-indeks, vertstid) per callback
                         ▼
 kamera ─ AVFoundation / Media Foundation ─▶ bilder med vertstid
                         ▼
     konformeren (sundayrec-core, ren): fast bildefrekvens låst til lydklokka
                         ▼
     begrenset kø (dropper og teller, blokkerer aldri) ─▶ ffmpeg kun for video, stdin
                         │                                 (maskinvarekoder) ─▶ MKV
                         └─▶ forhåndsvisning (nedskalert JPEG, 4 b/s) ─▶ UI

 Ved levering: WAV + MKV ─▶ mp4 (video -c copy, lyd AAC) · WAV-en beholdes som ren lydfil
```

**Lydklokka er sjefen.** Bildene følger lyden, aldri omvendt. Et opptak der
bildet hakker er et dårlig opptak. Et opptak der lyden mangler, er et tapt
opptak.

## Kildene

| Hva                                                                                  | Kilde                                                                      | Lisens                          | Slik brukes den                                                                                           |
| ------------------------------------------------------------------------------------ | -------------------------------------------------------------------------- | ------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Kamerafangst, Windows (Media Foundation, DirectShow-reserve, opptakskort, virtuelle) | Cap `crates/camera-windows`, `camera-mediafoundation`, `camera-directshow` | **MIT** (unntatt fra Caps AGPL) | Tatt inn i `crates/camera-windows/` m.fl. (C1b), låst til Cap `8a1480262db2`, med `UPSTREAM.md`           |
| Kamerafangst, Mac (AVFoundation)                                                     | Cap `crates/camera`, `camera-avfoundation`                                 | **MIT**                         | **Portes** til `objc2-av-foundation`, med Caps rettelser som spesifikasjon (se under)                     |
| `cap-mediafoundation-utils` (108 linjer: MF-oppstart, MMCSS, bufferlås)              | Cap                                                                        | **AGPL**                        | ❌ Leses ikke og kopieres ikke. Skrevet fra Microsofts dokumentasjon i `crates/mf-util` (C1b)             |
| Kodere, `recording`, `timestamp`, `enc-*`                                            | Cap                                                                        | AGPL                            | ❌ Leses ikke                                                                                             |
| nokhwa / crabcamera                                                                  | —                                                                          | Apache/MIT                      | ❌ Cap byttet bort fra nokhwa 2025-08-03 («start using new camera api»); crabcamera er et lag over nokhwa |

**Hvorfor porte Mac-delen i stedet for å ta den inn:** Cap bygger på en egen
gaffel av `cidre` (0.11, git-avhengighet låst til `bf84b67079a8`), mens
`src-tauri` allerede bruker objc2-familien (`objc2-app-kit`,
`objc2-foundation`, `objc2`). Én binding-familie for Apple-API-ene er verdt
600 linjer porting.

**Caps rettelser som må være med i porten** (hver var en ekte feil i
produksjon hos dem):

- Enhetens konfigurasjon låses mens `startRunning` kjører, ellers skriver macOS
  over det valgte formatet.
- Etter `setActiveFormat` settes `activeVideoMinFrameDuration` til valgt fps.
  Ellers leverer kameraet for eksempel 60 b/s mens opptaket regnes som 30, og
  opptaket blir «strukket». Maks-varigheten røres ikke, så kameraet får senke
  farten i dårlig lys.
- Be om kameraets egen pikselformat fra `AVCaptureVideoDataOutput` (ingen
  konvertering). Hvis økten ikke leverer bilder (sett på macOS 26), prøves den
  på nytt i «kompatibilitetsmodus», der OS-et forhandler formatet.
- Én global lås rundt start og stopp av økter. Autorelease-pool rundt
  enhetsoppslag. Bufferne som kommer uten bilde telles og hoppes over.
- Tidsstempelet er presentasjonstiden konvertert fra vertsklokka.
- Kameraet identifiseres med `uniqueID` (og USB-leverandør/produkt), ikke med
  nummer.

## Fasene

### C0 — raske gevinster i dagens motor

Gjøres først, fordi de hjelper flåten med en gang og ikke blir kastet.

- **Maskinvarekoding på Windows.** Ved første videoopptak (og i diagnosen)
  prøves koderne i rekkefølge med et kort testopptak (`-f lavfi -i
testsrc2=s=1280x720:d=0.2 -c:v <koder> -f null -`): `h264_nvenc` →
  `h264_qsv` → `h264_amf` → `h264_mf` → `libx264`. Det første som virker,
  lagres per maskin. Bitrate som på Mac (`default_video_bitrate_kbps`). Feiler
  den valgte koderen under et opptak, faller økta tilbake til `libx264` ved
  gjenkoblingen, og valget nullstilles.
- **Nøkkelbilde hvert 2. sekund** i alle videoopptak (`-g 2×fps`,
  `-keyint_min fps`): kortere tap ved krasj, raskere søk i redigeringen, og
  grunnlaget for rask videoeksport ([LYDMOTOR.md](LYDMOTOR.md) fase 6).
- **Ferdig når:** golden-testene i `capture.rs` dekker argumentene for hver
  koder. Et 60-minutters opptak på Windows-boksen bruker under halvparten av
  CPU-en det bruker i dag (Oppgavebehandling), uten tapte bilder.

### C1 — kamerafangst i Rust

Ny modul `src-tauri/src/camera/`, med de importerte Windows-cratene og porten
for Mac. Ingen endring i opptaket ennå.

- `list()` gir stabil ID, visningsnavn, modell-ID og om det er et opptakskort
  eller et virtuelt kamera. `formats(id)` gir oppløsning, fps og pikselformat.
  Dette erstatter `-list_devices` og modussøket via feilmeldinger
  (`media/camera.rs`, kameradelen av `device_enum`).
- `start(id, format, callback)` gir bilder (NV12/UYVY/YUY2/MJPEG) med
  vertstid. Callbacken gjør bare én ting: legger bildet i en begrenset kø.
- Tillatelse på Mac via den eksisterende `AVCaptureDevice
authorizationStatus`-koden.
- `examples/camera_probe.rs` lister kameraene og tar opp 10 s til en rå fil,
  med statistikk: fps, jitter og tapte bilder.
- **Ferdig når:** probe-eksempelet virker på Macen (innebygd kamera, et
  USB-kamera, et HDMI-opptakskort hvis et finnes) og på Windows-boksen.
  Diagnoserapporten bruker den nye listen.

### C2 — konformeren (ren kjernekode)

`sundayrec-core::av_clock` og `sundayrec-core::video_conform`. Ingen I/O og
ingen klokke, bare tall inn og tall ut, og alt testes.

- **Lydklokka:** par av (sample-indeks, vertstid) fra lydmotorens callbacks.
  Vertstid knyttes til sample-indeks med en lineær tilpasning over et glidende
  vindu. Lydkort og vertsklokke driver 20–50 ppm fra hverandre, og 50 ppm er
  0,27 s på 90 minutter. Tilpasningen fanger det.
- **Bildeplassene:** plass k ligger på sample `k × rate / fps`. For hver plass
  velges det nyeste bildet med tid ≤ plassen. Kommer ikke noe nytt, gjentas
  forrige (dårlig lys gir 15 b/s fra kameraet). Kommer flere, droppes de
  ekstra.
- **Hull:** når kameraet er borte, gjentas siste bilde i opptil 2 s, og
  deretter brukes svart bilde. Starter video etter lyden, fylles starten med
  svart. Hopper vertstiden (maskinen sov), startes tilpasningen på nytt.
- **Proptest:** tilfeldig drift, jitter, hull og fps-fall gir aldri mer enn
  ±1 bilde (33 ms) avvik fra lydklokka over 3 timer simulert opptak.

### C3 — den nye videoøkta

Den native motorens segmentløper (`native_capture/segment.rs`) får et valgfritt
videospor.

- Per fragment startes én ffmpeg-prosess **kun for video**: `-f rawvideo
-pix_fmt nv12 -s WxH -framerate 30 -i pipe:0` (eller `-f mjpeg` for
  MJPEG-kameraer), C0s maskinvarekoder, `-g`, `video_flip` som `hflip`, og MKV
  ut. Fragmentene får samme navn og grenser som lydfragmentene, så oppdeling og
  gjenkobling styres av lydmotoren som i dag.
- Køen til ffmpeg er begrenset til rundt 1 s med bilder. Er den full, droppes
  det eldste bildet og telles (`video.frames_dropped` i telemetri og i
  opptaksoverlegget). Kamera-callbacken blokkerer aldri.
- **Levering:** WAV + MKV blir mp4 (`-c:v copy`, lyd til AAC), og WAV-en
  beholdes som ren lydfil (`keep_separate_audio` blir gratis). Manifestet for
  krasjgjenoppretting lister begge, og gjenopprettingen setter dem sammen.
- **Pre-roll:** lyd-pre-rollen virker som i dag. Video starter når opptaket
  starter, og starten fylles med svart for pre-roll-lengden. Det er et bevisst
  valg; video-pre-roll er ikke planlagt.
- **Det som slettes** (11 000–14 000 linjer, målt 2026-10-12):
  `cpal_capture.rs` (1 562), `two_process.rs` (561, kjerne og skall),
  ffmpeg-fangsten i `engine.rs` (av 1 457 — se presiseringen under) og `engine/` (4 051: `supervisor`,
  `reader`, `args`, `process`, `finalize`, `emit`, `payloads`), kamera- og
  lyd-fangstbyggerne i `capture.rs` (≈700–1 300; `push_video_encoder_args`,
  `audio_encode_args` og kanalkartene **beholdes** — de brukes fortsatt av
  concat, finalize og ASIO), `device_enum` for ffmpeg (977, begge filene i sin
  helhet), `media/camera.rs` (165), `media/video_probe.rs` (473), og
  innstillingen `classic_directshow` (38 kallesteder). ffmpeg-pre-rollen og
  `classic_ffmpeg_audio`/`classic_ffmpeg_preroll` er alt borte med A5.
  `preflight`, `diagnostics` og `test_recording` går over til cpal- og
  kameralisten.
- **Tre presiseringer slettelista ikke kan klare seg uten:**
  - **`engine.rs` slettes kirurgisk, ikke som helfil.** `SupervisorPanicGuard`
    (#341) har to kallesteder, og det ene er den generelle `run_session` som
    native-økter går gjennom i dag. Vakten flyttes, den dør ikke.
  - **Kjernens `preroll.rs` er bare ≈280 linjer ffmpeg.**
    `build_preroll_capture_args`, `build_preroll_trim_args`,
    `ms_to_seconds_string` og testene deres. De øvrige ~800 linjene er ren
    matematikk — segment-beholdning, harvest-trim, uttoning,
    `preroll_restart_delay` — som den native pre-rollen bruker. De overlever.
  - **To rettelser må arves, ikke mistes.** #341 la stopprekkefølgen i
    `cpal_capture.rs` (stopp fangsten _før_ writeren, og sett tidsgrense på
    hver venting) — uten den kommer tapt hale og hengende stopp tilbake. #337
    la melding om en død lydprosess under videoopptak i `two_process.rs` — den
    nye motoren må si fra om det samme på egen kjøl.
- **Ferdig når:** RIG-DAY sin nye «(v) Videomotor»-runde er grønn på Mac og
  Windows: 90 minutter med klapp i start og slutt (synk ±1 bilde), USB-kabelen
  til kameraet trukket i 10 s midt i (lyden uten hull, bildet fryser og kommer
  tilbake), `kill -9` midt i (gjenopprettet mp4), og CPU-last som i C0.

### C4 — forhåndsvisning fra samme kameraøkt

- Kameraøkta åpnes når Opptak-siden viser kamerakortet, og lever videre inn i
  opptaket. Ingen overlevering, ingen ventetid.
- Annethvert til hvert sjuende bilde nedskaleres til 640 px bredde og kodes som
  JPEG i Rust, med `fast_image_resize` og `jpeg-encoder` (begge MIT/Apache,
  ren Rust). Bildene sendes på en Tauri-kanal, cirka 4 bilder i sekundet.
- **Det som slettes:** `LiveCameraPreview.tsx` og `getUserMedia`,
  `ownership.ts`, `PolledCameraPreview.tsx`, `frame-poll-core.ts`, ffmpegs
  ekstra JPEG-utgang (`preview_jpg`) og `PREVIEW_RESTART_MS`.
- **Ferdig når:** bildet før og under opptaket kommer fra samme økt, uten
  avbrudd ved Start og Stopp (e2e med fixture-bilder, og på Macen).

### C5 — kameraet kobles til på nytt uavhengig av lyden

- Mac: `AVCaptureDeviceWasDisconnected`/`…WasConnected`. Windows: kameradelen av
  enhetslytteren fra A4 ([OMBYGGING.md](OMBYGGING.md)). Når samme stabile ID
  dukker opp igjen, startes kameraøkta på nytt. Konformeren fyller hullet, og
  lyden merker ingenting.
- UI-et sier «Kameraet falt ut — lyden tas fortsatt opp», og varsler (VARSLING)
  hvis kameraet er borte i over 60 s.
- **Ferdig når:** kabeltrekket i C3 gir et gjenopprettet bilde uten at
  opptaket stopper, på begge plattformer.

## Notert, ikke planlagt

- **Helt native koding.** filmcrafts VideoToolbox- og NVENC-kodere (MIT/Apache)
  kunne erstatte ffmpeg også for video. Det tas bare hvis C3 måler at røret
  inn i ffmpeg er en flaskehals. 1080p30 NV12 er rundt 93 MB/s, og det bør et
  rør klare.
- **Flere kameraer samtidig.** Arkitekturen tillater det (ett spor per kamera),
  men ingen kirke har bedt om det.
- **Video-pre-roll.** Krever en ringbuffer med kodede bilder.

## Risiko

- **Dette er kjernen i opptaket.** Hver fase har sin riggsjekk, og C3 er den store.
  Macen der planen gjennomføres, har kamera og mikrofon og kan kjøre mye av
  riggen selv. Windows trenger Windows-boksen.
- **Kamerafølsomhet.** Opptakskort med bare DirectShow-driver dekkes av Caps
  reserve. Kameraer som stopper med native format, dekkes av
  kompatibilitetsmodus.
- **Tillatelser.** Kameratilgang på Mac må fortsatt bes om før første opptak.
  Det gjør `media/permissions.rs` allerede.
- **Ingen vei tilbake.** De gamle stiene slettes i C3, ikke etter en
  overgangsutgivelse. Det er eierens valg: få brukere i dag, og én sti er
  bedre enn fire.
