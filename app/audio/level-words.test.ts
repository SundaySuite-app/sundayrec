/**
 * Nivå i ord, og fyllet i stolpen — begge som tabeller.
 *
 * Tersklene er de eneste tallene en frivillig noensinne møter i denne formen
 * («Vi hører lyd»), så de skal ikke kunne flytte seg ved et uhell.
 */

import { describe, expect, it } from "vitest";

import { VU_FLOOR_DB } from "@lib/audio/vu-feed-core";

// Gjennom aliaset, ikke en relativ sti: ESLint tillater bare `@legacy/*` som
// vei inn i `legacy/`, så den dagen katalogene flytter er det én sti å endre.
// (`@legacy` er `legacy/`, `@lib` er det porterte inventaret i `app/lib/` —
// to aliaser fordi det er to trær, og ingen av dem nås relativt.)
import no from "@legacy/locales/no.json";

import {
  HEARD_DB,
  LEVEL_WORDS,
  LOUD_DB,
  levelFraction,
  levelWord,
  levelWordFor,
} from "./level-words";

describe("levelWord", () => {
  const cases: Array<[number, string, string]> = [
    [VU_FLOOR_DB, "nothing", "digital stillhet"],
    [-60, "nothing", "gulvet"],
    [HEARD_DB, "nothing", "akkurat PÅ stillhetsgrensen er fortsatt stillhet"],
    [HEARD_DB + 0.1, "hear", "så vidt over grensen hører vi noe"],
    [-20, "hear", "vanlig taleområde"],
    [LOUD_DB - 0.1, "hear", "rett under klippemarginen"],
    [LOUD_DB, "loud", "akkurat på klippemarginen er for høyt"],
    [0, "loud", "fullskala"],
  ];
  for (const [db, expected, why] of cases) {
    it(`${db} dBFS → ${expected} (${why})`, () => {
      expect(levelWord(db)).toBe(expected);
    });
  }

  it("en pakke uten tall er ikke bevis for lyd", () => {
    expect(levelWord(NaN)).toBe("nothing");
    expect(levelWord(Infinity)).toBe("nothing");
  });
});

describe("levelWordFor", () => {
  it("den høyeste av de to bestemmer — en kanal som klipper er klipping", () => {
    expect(levelWordFor(-40, -1)).toBe("loud");
    expect(levelWordFor(-1, -40)).toBe("loud");
  });

  it("stille på begge er stille", () => {
    expect(levelWordFor(-60, -58)).toBe("nothing");
  });

  it("et manglende tall leses som gulvet, ikke som lyd", () => {
    expect(levelWordFor(NaN, -55)).toBe("nothing");
    expect(levelWordFor(NaN, -10)).toBe("hear");
  });
});

describe("levelFraction", () => {
  it("gulvet er 0 og fullskala er 1", () => {
    expect(levelFraction(VU_FLOOR_DB)).toBe(0);
    expect(levelFraction(0)).toBe(1);
  });

  it("er lineær i dB — halve skalaen er −30, ikke −6", () => {
    // En amplitudeskala ville klemt all tale inn i de nederste prosentene.
    expect(levelFraction(-30)).toBeCloseTo(0.5, 6);
  });

  it("klemmer utenfor skalaen i stedet for å tegne utenfor stolpen", () => {
    expect(levelFraction(-200)).toBe(0);
    expect(levelFraction(12)).toBe(1);
    expect(levelFraction(NaN)).toBe(0);
  });
});

describe("LEVEL_WORDS", () => {
  it("er nøyaktig `app.vu`-subtreet i katalogen", () => {
    // `VuMeter` slår opp `tDyn("app.vu", word)`. Blir de to ulike, er
    // resultatet en TOM etikett — den ene feilen som overlever en hel
    // testrunde fordi den ser ut som «denne er visst tom». (I DEV kaster
    // `tDyn` på et bom, men det hjelper bare den som faktisk kjører DEV.)
    expect([...LEVEL_WORDS].sort()).toEqual(Object.keys(no.app.vu).sort());
  });
});
