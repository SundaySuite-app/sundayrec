import { test, expect } from "@playwright/test";

import { boot, BOOT_FIXTURES, SETTLED_SETTINGS } from "./harness";

// Retensjonspasset — «Slettes automatisk etter {n} dager», endelig sant.
//
// Oppkoblingen av V1/PR3-funnet: `recordings_prune` var appens eneste
// implementasjon av auto-slettingen og hadde ingen kallere. Eierbeslutningen
// 2026-08-31 sa papirkurv (som begge UI-tekstene lover), og passet kjører nå
// uspurt ved hver oppstart (`initRetention` i `app/main.tsx`).
//
// Det dette nivået legger til over node-testene (`app/state/retention.test.ts`)
// er SKJØTEN sett utenfra: at oppstarten faktisk spør, at en flytting blir en
// lesbar toast med en vei til papirkurven, og at en stille oppstart er stille.

/** Papirkurven slik `trash_list` svarer ETTER at passet har flyttet noe. */
const MOVED_ENTRIES = [1, 2, 3].map((n) => ({
  id: `t${n}`,
  originalPath: `/Users/test/Opptak/2026-05-0${n} Gudstjeneste.mp3`,
  trashedPath: `/Users/test/Opptak/.sundayrec-trash/${n}.mp3`,
  name: `2026-05-0${n} Gudstjeneste.mp3`,
  deletedAt: Date.now(),
  related: [],
  byteSize: 86_000_000,
}));

test.describe("retensjonspasset ved oppstart", () => {
  test("en flytting sier fra — toasten teller opptakene og «Vis papirkurven» går dit", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        recordings_prune: { moved: 3, disabled: false },
        trash_list: MOVED_ENTRIES,
      },
      settings: { ...SETTLED_SETTINGS, autoDeleteDays: 90 },
      goto: "home",
    });

    const host = page.getByTestId("toast-host");
    await expect(host).toContainText(
      "3 gamle opptak ble flyttet til papirkurven",
    );

    // Handlingen går til papirkurven — og radene der er de som ble flyttet.
    await host.getByRole("button", { name: "Vis papirkurven" }).click();
    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
    await expect(page.getByTestId("trash-row")).toHaveCount(3);
  });

  test("en oppstart uten noe å flytte er stille", async ({ page }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        recordings_prune: { moved: 0, disabled: false },
      },
      settings: { ...SETTLED_SETTINGS, autoDeleteDays: 90 },
      goto: "home",
    });

    // Skallet er ferdig vekket (statuslinja står) — og ingen toast kom.
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );
    await expect(page.getByTestId("toast-host")).toHaveAttribute(
      "data-empty",
      "true",
    );
  });
});
