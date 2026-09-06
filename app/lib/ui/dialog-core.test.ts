import { describe, expect, it } from "vitest";
import {
  buildAlert,
  buildConfirm,
  buildPrompt,
  buildSelect,
  cancelButton,
  defaultButton,
  nextFocusIndex,
} from "./dialog-core";

describe("buildConfirm", () => {
  it("puts cancel first and confirm last", () => {
    const spec = buildConfirm({ title: "Slett?" });
    expect(spec.buttons.map((b) => b.id)).toEqual(["cancel", "ok"]);
  });

  it("makes the confirm button the Enter default on a normal confirm", () => {
    const spec = buildConfirm({ title: "Fortsett?" });
    expect(defaultButton(spec)?.id).toBe("ok");
    expect(cancelButton(spec)?.id).toBe("cancel");
  });

  it("moves the Enter default to CANCEL when the action is destructive", () => {
    // The whole point of the danger variant: a stray Enter must not delete
    // anything. This is the single most important assertion in the file.
    const spec = buildConfirm({ title: "Slett opptak?", danger: true });
    expect(defaultButton(spec)?.id).toBe("cancel");
    expect(spec.buttons.find((b) => b.id === "ok")?.variant).toBe("danger");
    expect(spec.danger).toBe(true);
  });

  describe("escapeConfirms — F2-T3", () => {
    // `pages/record/stop.ts` puts the ACTIVE choice ("stop the recording")
    // behind `cancelLabel` for styling only (ghost, non-default, no red) —
    // reproduced here without danger's paint. Without `escapeConfirms`,
    // Escape/backdrop resolve through whichever button holds `isCancel`,
    // which is exactly the destructive one: proven once, live, in
    // `e2e/record.spec.ts` (Escape called `stop_recording`).
    it("without it, isCancel still sits on the cancel id (today's default)", () => {
      const spec = buildConfirm({
        title: "Stoppe opptaket?",
        confirmLabel: "Fortsett å ta opp",
        cancelLabel: "Stopp",
      });
      expect(cancelButton(spec)?.id).toBe("cancel");
      expect(cancelButton(spec)?.label).toBe("Stopp");
    });

    it("moves isCancel to the OK button, so Escape lands on the safe choice", () => {
      const spec = buildConfirm({
        title: "Stoppe opptaket?",
        confirmLabel: "Fortsett å ta opp",
        cancelLabel: "Stopp",
        escapeConfirms: true,
      });
      expect(cancelButton(spec)?.id).toBe("ok");
      expect(cancelButton(spec)?.label).toBe("Fortsett å ta opp");
    });

    it("never touches styling or the Enter default — only which button Escape hits", () => {
      const plain = buildConfirm({ title: "x" });
      const flipped = buildConfirm({ title: "x", escapeConfirms: true });
      // Labels, ids and variants are byte-identical…
      expect(
        flipped.buttons.map((b) => ({ ...b, isCancel: undefined })),
      ).toEqual(plain.buttons.map((b) => ({ ...b, isCancel: undefined })));
      // …Enter still hits "ok" either way…
      expect(defaultButton(flipped)?.id).toBe(defaultButton(plain)?.id);
      // …only Escape's target moved.
      expect(cancelButton(plain)?.id).toBe("cancel");
      expect(cancelButton(flipped)?.id).toBe("ok");
    });

    it("is a no-op combined with danger — that cancel button is already safe", () => {
      const spec = buildConfirm({
        title: "Slett opptak?",
        danger: true,
        escapeConfirms: true,
      });
      expect(cancelButton(spec)?.id).toBe("cancel");
      expect(defaultButton(spec)?.id).toBe("cancel");
    });

    it("exactly one button carries isCancel, either way", () => {
      for (const escapeConfirms of [false, true]) {
        const spec = buildConfirm({ title: "x", escapeConfirms });
        expect(spec.buttons.filter((b) => b.isCancel)).toHaveLength(1);
      }
    });
  });

  it("uses caller labels verbatim", () => {
    const spec = buildConfirm({
      title: "x",
      confirmLabel: "Ja, stopp",
      cancelLabel: "Fortsett opptak",
    });
    expect(spec.buttons.map((b) => b.label)).toEqual([
      "Fortsett opptak",
      "Ja, stopp",
    ]);
  });

  it("carries the message through", () => {
    expect(buildConfirm({ title: "t", message: "m" }).message).toBe("m");
  });
});

describe("buildAlert", () => {
  it("has exactly one button that is both default and cancel", () => {
    // An alert has no cancel affordance, so Escape has to resolve through OK —
    // otherwise Escape would do nothing and the dialog would trap the user.
    const spec = buildAlert({ title: "Feil" });
    expect(spec.buttons).toHaveLength(1);
    expect(defaultButton(spec)?.id).toBe("ok");
    expect(cancelButton(spec)?.id).toBe("ok");
  });

  it("marks error-tone alerts as danger for the red accent", () => {
    expect(buildAlert({ title: "x", tone: "error" }).danger).toBe(true);
    expect(buildAlert({ title: "x" }).danger).toBe(false);
  });
});

describe("buildPrompt", () => {
  it("seeds the input from defaultValue and defaults to single-line", () => {
    const spec = buildPrompt({ title: "Navn", defaultValue: "Gudstjeneste" });
    expect(spec.input).toEqual({
      value: "Gudstjeneste",
      placeholder: "",
      multiline: false,
    });
  });

  it("honours multiline", () => {
    expect(buildPrompt({ title: "x", multiline: true }).input?.multiline).toBe(
      true,
    );
  });

  it("defaults to OK on Enter, cancels on Escape", () => {
    const spec = buildPrompt({ title: "x" });
    expect(defaultButton(spec)?.id).toBe("ok");
    expect(cancelButton(spec)?.id).toBe("cancel");
  });
});

describe("buildSelect", () => {
  const options = [
    { id: "a", label: "Skjerm 1", detail: "2560×1440" },
    { id: "b", label: "Skjerm 2" },
  ];

  it("carries the options through untouched", () => {
    expect(buildSelect({ title: "Velg skjerm", options }).options).toEqual(
      options,
    );
  });

  it("offers only a cancel button — picking a row is the confirmation", () => {
    const spec = buildSelect({ title: "x", options });
    expect(spec.buttons.map((b) => b.id)).toEqual(["cancel"]);
    expect(defaultButton(spec)).toBeNull();
    expect(cancelButton(spec)?.id).toBe("cancel");
  });

  it("accepts an empty option list without inventing buttons", () => {
    const spec = buildSelect({ title: "x", options: [] });
    expect(spec.options).toEqual([]);
    expect(spec.buttons).toHaveLength(1);
  });
});

describe("nextFocusIndex", () => {
  it("advances forwards and wraps at the end", () => {
    expect(nextFocusIndex(3, 0, false)).toBe(1);
    expect(nextFocusIndex(3, 2, false)).toBe(0);
  });

  it("advances backwards and wraps at the start", () => {
    expect(nextFocusIndex(3, 2, true)).toBe(1);
    expect(nextFocusIndex(3, 0, true)).toBe(2);
  });

  it("re-enters at the edge the user is travelling towards when focus escaped", () => {
    expect(nextFocusIndex(3, -1, false)).toBe(0);
    expect(nextFocusIndex(3, -1, true)).toBe(2);
  });

  it("treats an out-of-range index as escaped rather than wrapping past it", () => {
    expect(nextFocusIndex(3, 9, false)).toBe(0);
    expect(nextFocusIndex(3, 9, true)).toBe(2);
  });

  it("reports -1 when there is nothing to focus", () => {
    expect(nextFocusIndex(0, 0, false)).toBe(-1);
  });

  it("stays put on a single focusable element", () => {
    expect(nextFocusIndex(1, 0, false)).toBe(0);
    expect(nextFocusIndex(1, 0, true)).toBe(0);
  });
});
