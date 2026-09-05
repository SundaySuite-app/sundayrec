# SundayRec for frivillige

Denne siden er for deg som skal ta opp gudstjenesten — alene, uten en
utvikler i rommet. Den dekker det du faktisk trenger: installere appen én
gang, ta opp hver søndag, og vite hva du gjør når noe ser feil ut.

Kjenner du SundayRec fra før og bare vil sjekke noe raskt: bruk
overskriftene under som innholdsfortegnelse.

## Installer appen (bare første gang)

Last ned installasjonsfilen menigheten har fått tilsendt, eller hent den
selv fra [GitHub Releases](https://github.com/SundaySuite-app/sundayrec/releases/latest).

**På Mac:** macOS kjenner ikke igjen appen ennå (den er ikke «notarisert» av
Apple — det koster og tar tid, og vi har ikke kommet dit). Du får en advarsel
første gang. Slik åpner du den likevel:

1. Dobbeltklikk installasjonsfilen som vanlig.
2. Ser du «kan ikke åpnes fordi utvikleren ikke kan bekreftes» — ikke trykk
   «Flytt til papirkurven». Gå i stedet til **Finder → Programmer**, **høyreklikk**
   på SundayRec og velg **«Åpne»**. Nå får du et ekstra spørsmål med en
   **«Åpne»**-knapp — den finnes ikke i den første advarselen, bare i denne.
3. Dette gjør du bare ÉN gang per maskin. Etterpå åpnes appen normalt, også
   fra Dock eller Launchpad.

**På Windows:** installasjonsfilen er ikke signert ennå, så Windows advarer
med «SmartScreen beskyttet PC-en din».

1. Kjør installasjonsfilen.
2. Trykk **«Mer info»**, og deretter **«Kjør likevel»**.
3. Følg installasjonsveiviseren som vanlig.

Advarselen betyr ikke at noe er galt — den betyr bare at ingen har betalt
Apple/Microsoft for et sertifikat ennå. Filen kommer fra menighetens egen
nedlasting, ikke fra en tredjepart.

## Gi appen tilgang til mikrofonen (og kamera, hvis dere filmer)

Første gang appen skal ta opp lyd, spør operativsystemet om lov. **Trykk
«Tillat» / «Allow».** Sier du nei ved et uhell, eller ser appen ikke noen
lyd i det hele tatt (metervisning ligger dødt flatt selv om du snakker), er
tilgangen trolig avslått:

- **Mac:** Systeminnstillinger → Personvern og sikkerhet → Mikrofon (og
  Kamera) → skru på SundayRec, og start appen på nytt.
- **Windows:** Innstillinger → Personvern og sikkerhet → Mikrofon (og
  Kamera) → skru på tilgang for apper, og at SundayRec står som tillatt.

## Velg lyd (kontrollrommet)

Appen har tre knapper nederst — **Opptak · Redigering · Eksportering** — og
et tannhjul for Innstillinger helt til høyre. Du gjør nesten alt fra
**Opptak**.

1. Trykk **«Velg lyd»** (eller **«Endre»**, hvis noe allerede er valgt) på
   kortet til venstre. Kortet folder seg ut med enhetene appen finner:
   maskinens egen mikrofon, en USB-mikrofon, eller et miksebord med flere
   kanaler.
2. Velg enheten. Snakk eller send lyd inn på mikseren — ordet under måleren
   forteller deg det du faktisk trenger å vite: **«Vi hører ingenting»** →
   **«Vi hører lyd»** → **«For høyt»**. Har mikseren flere kanaler, velg
   paret dere sender lyd på.
3. Trykk **«Bruk denne»**. Du får ÉN kvittering, «Lagret ✓», når valget er
   tatt vare på.

Gjør dette én gang per oppsett — appen husker valget til noen bytter mikser
eller mikrofon.

## Ta opp

1. Trykk **«Start opptak»** på Opptak. Er ikke lyd valgt ennå, forteller
   knappen deg akkurat det i stedet for bare å være grå.
2. Statuslinjen nederst til venstre blir rød og sier **«Tar opp»** —
   rødt betyr alltid nettopp det, ingenting annet, i denne appen.
3. Når gudstjenesten er ferdig: trykk **«Stopp»**-knappen i opptaksbildet.
   Du blir spurt om du er sikker — **Enter/Retur fortsetter opptaket**, du må
   aktivt velge «Stopp» for å avslutte. Det er med vilje: den som trykker feil
   skal ikke miste opptaket.
4. Vent til kortet **«Opptaket er lagret»** vises. Ikke lukk appen før du ser
   det — filen skrives ferdig i denne pausen.

**Finn fila:** gå til **Redigering** — den nye opptaksraden ligger øverst,
med dato og klokkeslett. Trykk **«Vis i Finder»** på raden for å åpne mappen
filen faktisk ligger i — samme knappetekst på Windows også, den åpner der
Utforsker (File Explorer).

Lukker du hele appen mens den tar opp, fortsetter opptaket i bakgrunnen (se
etter ikonet i menylinjen/systemstatusfeltet) — men ikke gjør det med vilje;
la appen stå åpen til opptaket er stoppet og lagret.

## Når noe ser feil ut

Appen viser en **stripe øverst** når noe trenger oppmerksomhet, og den blir
stående til noen har sett den — den forsvinner ikke av seg selv:

- **Gul/oransje stripe:** noe bør ordnes før neste søndag, men ingenting er
  ødelagt ennå (typisk: lite diskplass igjen, eller en enhet som falt ut og
  koblet seg til igjen).
- **Rød stripe:** noe gikk tapt, og teksten sier når det skjedde. Les den —
  klokkeslettet er der for at dere skal vite hvor mye av gudstjenesten som
  faktisk kom med.

Er du usikker på hva som er galt, eller vil sende oss et konkret bevis:

1. Gå til tannhjulet (**Innstillinger**) → **Avansert** → **Diagnose** →
   trykk **«Kjør»**. Du får fem svar (enheter, valgt enhet, mikrofontilgang,
   motor, lydprøve) — et rødt kryss viser hva som er galt.
2. Trykk **«Kopier full rapport»**. Rapporten inneholder ingen lyd og ingen
   passord — bare tekniske fakta om maskinen og appen.
3. Lim den inn der du ber om hjelp: et GitHub-issue, eller en e-post til
   **dev@sundaysuite.app**. Rapporten alene forklarer sjelden alt — skriv
   gjerne med hva dere opplevde også.

## Hvem ringer du?

**Ring menighetens egen faste kontakt for lyd/opptak først** — den som satte
opp appen kjenner rommet, mikseren og hva som pleier å svikte. Skriv navn og
nummer inn her, så alle frivillige finner det samme stedet:

> Fast kontakt: **\_\_\_\_\_\_\_\_\_\_\_\_\_\_\_\_\_\_\_\_\_\_** — telefon: **\_\_\_\_\_\_\_\_\_\_\_\_\_\_**

Får dere ikke tak i noen, og det haster ikke i sekunder (gudstjenesten går
fint videre uten appen — det er lyden i rommet som teller, ikke opptaket):
skriv til **dev@sundaysuite.app**. En ekte person leser meldingen, men
neppe midt i søndagsformiddagen — dette er ikke en akuttlinje.

(Har du funnet en **sikkerhetssårbarhet** — ikke en vanlig feil — er det en
annen, privat kanal: se [`SECURITY.md`](../SECURITY.md). De fleste
frivillige trenger aldri den siden.)

## Viktig: sikkerhetskopi er IKKE en funksjon

**Fila på disken er alt som finnes.** SundayRec laster ikke opp noe sted,
lager ingen podkast-feed, og deler ikke opptaket automatisk med noen. Det er
ikke en mangel vi glemte — skylagring, podkast-publisering og deling fantes
i den gamle appen og ble bevisst fjernet. Grunnen: et verktøy som later som
det tar sikkerhetskopi for deg, er farligere enn ett som er ærlig om at det
ikke gjør det.

Det betyr, konkret:

- **Slett aldri opptaksmappen manuelt uten å ha kopiert det dere trenger ut
  først.** Det finnes en papirkurv i appen (opptak flyttes dit, aldri
  slettet direkte) — men den tømmer seg selv etter 30 dager.
- **Vil menigheten ha en sikkerhetskopi, må dere lage den selv** — kopiér
  mappen til en ekstern disk, en skytjeneste dere allerede har, eller lignende,
  med jevne mellomrom. Det er ikke noe appen kan gjøre for dere.
- Bytter dere maskin, må dere flytte filene selv. De ligger i den mappen
  dere valgte i kortet **«Hvor skal opptakene?»** på Opptak — samme kort
  viser gjeldende mappe hvis dere er usikre.

---

Eier korrekturleser denne siden før den lenkes bredt — meld fra om noe her
ikke stemmer med det appen faktisk gjør.
