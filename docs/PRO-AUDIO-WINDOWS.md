# Bruke SundayRec med pro-lydkort på Windows (ASIO)

SundayRec støtter **ASIO** på Windows — en lav-latens lyddriver-standard som proff
lydutstyr bruker. Det løser to vanlige problemer med store lydkort/miksere (f.eks.
Soundcraft MADI-USB, Behringer X32, RME, Focusrite):

- **Alle kanaler under én enhet.** Windows' vanlige lydvei (DirectShow/WASAPI)
  deler et flerkanals lydkort opp i flere «stereopar». ASIO viser hele kortet som
  **én enhet** der du kan velge nøyaktig hvilke inn-kanaler du vil ta opp
  (f.eks. kanal 9 og 10 fra en mikser).
- **Mer stabilt.** ASIO snakker direkte med produsentens driver, med færre ledd å
  gå gjennom — som regel mer robust enn den generiske Windows-veien.

## Slik gjør du

1. **Installer produsentens ASIO-driver.** SundayRec lager ikke driveren — den
   bruker den. Last ned og installer ASIO-driveren for lydkortet ditt fra
   produsenten. (Har du ikke en dedikert driver, fungerer **ASIO4ALL** som en
   generisk ASIO-driver oppå Windows-lyden.)
2. **Velg lydkortet i SundayRec.** Gå til **Innstillinger → Lyd**. ASIO-enheter
   vises øverst med et **«ASIO»-merke**. Velg kortet ditt.
3. **Velg kanaler.** Har kortet mer enn 2 kanaler, dukker det opp en
   **kanalvelger** (Venstre / Høyre). Velg hvilke inn-kanaler opptaket skal bruke.
4. **Ta opp som vanlig.** Lyd-only og lyd + video fungerer begge.

## Hvis ASIO ikke er tilgjengelig

Finner SundayRec ingen ASIO-driver, brukes **WASAPI automatisk** i stedet — alt
fungerer som før, bare uten den samlede flerkanals-visningen. Skulle en ASIO-enhet
feile akkurat når et opptak starter (driveren opptatt, kortet frakoblet), faller
SundayRec **automatisk tilbake til WASAPI** og gir en melding om det, slik at
opptaket ikke ryker.

## macOS

På macOS trengs ikke ASIO: Core Audio viser allerede et samle-lydkort som én
enhet med alle kanaler. SundayRec fungerer der som før, uendret.

---

## For utviklere / lisens

ASIO-støtten er en **Windows-only, valgfri** Cargo-feature (`asio`). Bygg-oppsett:
se [`BUILD_ASIO.md`](./BUILD_ASIO.md).

### Tredjeparts-komponenter brukt av ASIO-veien

| Komponent    | Bruk                                   | Lisens                                            |
| ------------ | -------------------------------------- | ------------------------------------------------- |
| **ASIO SDK** | ASIO-driver-grensesnittet (via `cpal`) | Steinberg proprietær (gratis, krever attribusjon) |
| **cpal**     | Lyd-I/O på tvers av plattform          | Apache-2.0 / MIT                                  |
| **ringbuf**  | Lock-free buffer (callback → ffmpeg)   | MIT / Apache-2.0                                  |

**Steinberg-attribusjon** (vises i Windows-bygget under Innstillinger → Generelt →
«Lyd-teknologi», og gjengitt her som lisenskrav):

> ASIO Driver Interface Technology by Steinberg Media Technologies GmbH. ASIO is a
> trademark and software of Steinberg Media Technologies GmbH.

ASIO-logo og videre bruk følger Steinbergs «ASIO SDK Usage Guidelines» dersom
markedsføringsmateriell distribueres (ikke nødvendig inne i selve appen).
