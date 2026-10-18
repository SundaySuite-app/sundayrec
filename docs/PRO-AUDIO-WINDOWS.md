# Windows-lyd i SundayRec: moderne lyd-motor (WASAPI + ASIO)

På Windows tar SundayRec opp lyd via en **moderne lyd-motor** i stedet for det
gamle DirectShow-API-et som ga ustabile opptak («virker av og til»). Du trenger
ikke gjøre noe — det skjer automatisk:

- **Vanlige enheter (USB-mikrofon, lydkort): WASAPI.** Windows' moderne lyd-API.
  Mer stabilt enn den gamle DirectShow-veien.
- **Proff lydutstyr (Soundcraft MADI-USB, Behringer X32, RME, Focusrite): ASIO.**
  Når en ASIO-driver er installert, vises hele kortet som **én enhet** med **alle
  kanaler** (DirectShow delte dem i «stereopar», så du f.eks. ikke fikk tak i kanal
  9/10). ASIO gir også lavest latens og mest stabil pro-lyd.

macOS er upåvirket av selve ASIO/WASAPI-skiftet — der brukes Core Audio. Et
**lyd-only**-opptak går siden 2026-08-01 via samme native motor som beskrevet
under («For utviklere»); et opptak **med video** bruker ffmpeg/avfoundation
som før, uendret.

## Slik gjør du

1. **Installer produsentens ASIO-driver.** SundayRec lager ikke driveren — den
   bruker den. Last ned og installer ASIO-driveren for lydkortet ditt fra
   produsenten. (Har du ikke en dedikert driver, fungerer **ASIO4ALL** som en
   generisk ASIO-driver oppå Windows-lyden.)
2. **Velg lydkortet i SundayRec.** Gå til **Oppsett → «Hvilken lyd?»**. ASIO-enheter
   vises øverst med et **«ASIO»-merke**. Velg kortet ditt.
3. **Velg kanaler.** Har kortet mer enn 2 kanaler, dukker det opp en
   **kanalvelger** (Venstre / Høyre). Velg hvilke inn-kanaler opptaket skal bruke.
4. **Ta opp som vanlig.** Lyd-only og lyd + video fungerer begge.

## Robusthet

Skulle den moderne motoren ikke klare å starte (driveren opptatt, kortet
frakoblet i det opptaket begynner), starter ikke opptaket, og appen sier hvorfor
(for eksempel at lydenheten er i bruk av et annet program). Trekkes lydkortet ut
midt i et opptak, prøver appen å koble til igjen; kommer det ikke tilbake,
avsluttes opptaket pent (filen blir lagret) med en tydelig melding.

Den gamle DirectShow-veien og valget «Klassisk lyd-motor» er borte (lydopptak i
A5b, videoopptak i C3b): det finnes én opptaksmotor.

## macOS

På macOS trengs ingen driverinstallasjon eller enhetsmerking av den typen denne
siden beskriver: Core Audio viser allerede et samle-lydkort som én enhet med
alle kanaler. Et **lyd-only**-opptak skriver siden 2026-08-01 direkte til WAV
via den samme native cpal-motoren som Windows-siden (cpal → CoreAudio → ring →
WAV-skriver, ingen ffmpeg i selve fangsten) — den gamle ffmpeg/avfoundation-veien
mistet målbart samples, og det gjaldt begge plattformer. Siden C3b gjør et opptak
**med video** det samme: lyden tas opp av den native motoren, og kameraet leses
av appen selv ved siden av (ffmpeg koder bare bildet).

---

## For utviklere / lisens

Lydfangst på Windows går gjennom `cpal`: **WASAPI** er standard og krever ingen
feature; **ASIO** er en valgfri Cargo-feature (`asio`) fordi den lenker mot
Steinberg ASIO SDK. **Siden 2026-08-01** skriver et lyd-only-opptak (på begge
plattformer) direkte til WAV via den native motoren
(`recorder::native_capture`: cpal → ring → WAV-skriver) — INGEN ffmpeg
involvert i selve fangsten. Siden C3b gjelder det også **video**-økter:
`recorder::cpal_capture`s rå-PCM-pipe inn i ffmpeg er slettet, og kameraet leses
av `crate::camera` (Media Foundation / DirectShow) og kodes av en egen
video-ffmpeg. Nødbryterne som sendte opptak tilbake til ffmpeg
(`classicFfmpegAudio`, A5b) og DirectShow (`classicDirectshow`, C3b) er fjernet. Bygg-oppsett for ASIO: se
[`BUILD_ASIO.md`](./BUILD_ASIO.md).

### Tredjeparts-komponenter brukt av Windows-lyd-veien

| Komponent    | Bruk                                   | Lisens                                            |
| ------------ | -------------------------------------- | ------------------------------------------------- |
| **ASIO SDK** | ASIO-driver-grensesnittet (via `cpal`) | Steinberg proprietær (gratis, krever attribusjon) |
| **cpal**     | Lyd-I/O på tvers av plattform          | Apache-2.0 / MIT                                  |
| **ringbuf**  | Lock-free buffer (callback → ffmpeg)   | MIT / Apache-2.0                                  |

**Steinberg-attribusjon** (vises i Windows-bygget nederst under Oppsett →
Avansert, som «Lyd-teknologi», og gjengitt her som lisenskrav):

> ASIO Driver Interface Technology by Steinberg Media Technologies GmbH. ASIO is a
> trademark and software of Steinberg Media Technologies GmbH.

ASIO-logo og videre bruk følger Steinbergs «ASIO SDK Usage Guidelines» dersom
markedsføringsmateriell distribueres (ikke nødvendig inne i selve appen).
