import { describe, expect, it } from "vitest";
import { mapGate } from "./feature-gate-core";

describe("mapGate", () => {
  it("is invisible when the feature works", () => {
    const view = mapGate({
      status: "ok",
      chipText: "ignored",
      explanation: "ignored",
    });
    expect(view).toEqual({
      showBanner: false,
      disabled: false,
      variant: "ok",
      chipText: "",
      explanation: "",
    });
  });

  it("disables and explains when unconfigured", () => {
    const view = mapGate({ status: "unconfigured" });
    expect(view.showBanner).toBe(true);
    expect(view.disabled).toBe(true);
    expect(view.chipText).toBe("Ikke konfigurert");
    expect(view.explanation).not.toBe("");
  });

  it('distinguishes "not built" from "not set up"', () => {
    expect(mapGate({ status: "unavailable" }).chipText).toBe(
      "Ikke tilgjengelig",
    );
    expect(mapGate({ status: "unavailable" }).explanation).not.toBe(
      mapGate({ status: "unconfigured" }).explanation,
    );
  });

  it("prefers caller-supplied (translated) copy and trims blanks away", () => {
    const view = mapGate({
      status: "unconfigured",
      chipText: "  Ikke satt opp  ",
      explanation: " Be utvikleren om en build med e-post. ",
      docsHint: "  ",
    });
    expect(view.chipText).toBe("Ikke satt opp");
    expect(view.explanation).toBe("Be utvikleren om en build med e-post.");
    expect(view.docsHint).toBeUndefined();
  });
});
