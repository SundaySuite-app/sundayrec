/**
 * `inert` på `#app` — med en teller, så to eiere ikke tråkker på hverandre.
 *
 * To ting slår av resten av appen: bekreftelsesdialogen (`DialogHost`) og
 * opptaksoverlegget (`RecordingOverlay`, der Tab skal gå mellom Hold, +30 og ∞
 * og ingenting bak). Hadde hver satt og fjernet attributtet selv, ville en
 * dialog som lukkes midt i et opptak åpnet hele skallet bak overlegget igjen.
 * Hver eier tar en lås og slipper den; attributtet står så lenge noen har en.
 */

let locks = 0;

function apply(): void {
  const host = document.getElementById("app");
  if (!host) return;
  if (locks > 0) host.setAttribute("inert", "");
  else host.removeAttribute("inert");
}

/** Slå av `#app`. Returverdien slipper låsen (en gang; flere slipp er harmløse). */
export function lockApp(): () => void {
  locks++;
  apply();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    locks = Math.max(0, locks - 1);
    apply();
  };
}

/** Antall låser nå (for tester). */
export function appLockCount(): number {
  return locks;
}
