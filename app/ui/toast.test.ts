/**
 * Toast queue — a permanent error toast must not pile up without bound
 * (audit 2026-10-05: «Feil-toaster er permanente og ubegrensede»).
 */

import { afterEach, describe, expect, it } from "vitest";

import { clearToasts, MAX_TOASTS, toast, toasts } from "./toast";

afterEach(() => clearToasts());

describe("toast — tak og innfolding", () => {
  it("identiske meldinger foldes inn i én", () => {
    for (let i = 0; i < 50; i++) toast("error", "Disken er full");
    expect(toasts.value).toHaveLength(1);
  });

  it("ulike meldinger går aldri over taket", () => {
    for (let i = 0; i < 50; i++) toast("error", `Feil ${i}`);
    expect(toasts.value).toHaveLength(MAX_TOASTS);
    // Newest survive.
    expect(toasts.value.at(-1)?.msg).toBe("Feil 49");
  });

  it("en feil fortrenger en info-toast før en annen feil", () => {
    toast("info", "Lagret");
    for (let i = 0; i < MAX_TOASTS - 1; i++) toast("error", `Feil ${i}`);
    toast("error", "Ny feil");
    expect(toasts.value.some((x) => x.msg === "Lagret")).toBe(false);
    expect(toasts.value.filter((x) => x.kind === "error")).toHaveLength(
      MAX_TOASTS,
    );
  });

  it("toaster med handling foldes aldri (hver har sin egen callback)", () => {
    const action = { label: "Vis", onClick: () => {} };
    toast("warn", "Samme", { action });
    toast("warn", "Samme", { action });
    expect(toasts.value).toHaveLength(2);
  });
});
