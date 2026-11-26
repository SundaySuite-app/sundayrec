/**
 * Steinberg-attribusjonen er en LISENSFORPLIKTELSE, ikke en flate.
 *
 * Den bodde i `legacy/renderer/index.html` og forsvant da fila ble slettet i
 * fase B. Ingen test fanget det, ingen bruker savnet det, og en manglende
 * varemerkenotise ser nøyaktig ut som ingenting. Denne testen er grunnen til
 * at det ikke kan skje en gang til: den pinner ORDLYDEN og at kortet faktisk
 * er der på Windows.
 *
 * Node-miljø + `preact-render-to-string`, som resten av komponenttestene.
 */

import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import { AsioAttribution } from "./AsioAttribution";

// Ordrett fra ASIO SDK-lisensen. Endres denne, skal testen si fra — det er en
// avtaletekst, ikke produktkopi.
const NOTICE =
  "ASIO Driver Interface Technology by Steinberg Media Technologies GmbH. " +
  "ASIO is a trademark and software of Steinberg Media Technologies GmbH.";

describe("Steinberg ASIO attribution", () => {
  it("står på Windows, med lisensteksten ordrett", () => {
    const html = render(<AsioAttribution os="win" />);
    expect(html).toContain('data-testid="advanced-asio-attribution"');
    // Preact escaper ingenting her (teksten har ingen HTML-tegn), så en ren
    // delstreng er den samme sammenligningen brukeren gjør med øynene.
    expect(html).toContain(NOTICE);
  });

  it("er borte på macOS og Linux — ASIO finnes ikke der", () => {
    for (const os of ["mac", "linux", "other"] as const) {
      expect(render(<AsioAttribution os={os} />)).toBe("");
    }
  });
});
