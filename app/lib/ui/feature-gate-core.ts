/**
 * feature-gate-core — deciding what a section is allowed to claim.
 *
 * SundayRec can ship a panel whose backend is not in this build, or not set
 * up yet. Until this module such a panel looked exactly like a working one — a
 * «Send test» that reported a failure it invented. (The e-mail card and the
 * cloud-backup card were the mechanism's first consumers; both features are
 * gone. The reminder on the notify page is the one that remains.)
 *
 * A volunteer cannot tell "you configured this wrong" from "this does not exist
 * yet", and will spend a Saturday evening trying. So each such section states
 * its status once, at the top, and turns its controls off.
 *
 * The mapping from a backend fact to a user-facing status lives here, pure.
 */

/** What the section can actually do right now. */
export type GateStatus =
  /** Backed, configured, usable — no banner, nothing disabled. */
  | "ok"
  /** The feature exists in this build but has not been set up. */
  | "unconfigured"
  /** Not present in this build at all. Nothing the user can do about it. */
  | "unavailable";

export interface GateInput {
  status: GateStatus;
  /** Short badge, e.g. «Ikke konfigurert». Defaults per status. */
  chipText?: string;
  /** One or two sentences saying what is missing and who can fix it. */
  explanation?: string;
  /** Optional extra line — where to look, what to ask for. */
  docsHint?: string;
}

/** What the renderer should paint. */
export interface GateView {
  showBanner: boolean;
  /** Set `inert` on the section's controls. */
  disabled: boolean;
  variant: GateStatus;
  chipText: string;
  explanation: string;
  docsHint?: string;
}

/** Norwegian defaults; the DOM layer passes translated strings in `GateInput`. */
const DEFAULT_CHIP: Record<GateStatus, string> = {
  ok: "",
  unconfigured: "Ikke konfigurert",
  unavailable: "Ikke tilgjengelig",
};

const DEFAULT_EXPLANATION: Record<GateStatus, string> = {
  ok: "",
  unconfigured: "Denne funksjonen er ikke satt opp ennå.",
  unavailable: "Denne funksjonen er ikke bygget inn i denne versjonen.",
};

/**
 * Turn a status into a render plan.
 *
 * `ok` renders nothing and disables nothing — a gate must be invisible when the
 * feature works, or it becomes the wallpaper users learn to ignore.
 */
export function mapGate(input: GateInput): GateView {
  const { status } = input;
  if (status === "ok") {
    return {
      showBanner: false,
      disabled: false,
      variant: "ok",
      chipText: "",
      explanation: "",
    };
  }
  return {
    showBanner: true,
    disabled: true,
    variant: status,
    chipText: input.chipText?.trim() || DEFAULT_CHIP[status],
    explanation: input.explanation?.trim() || DEFAULT_EXPLANATION[status],
    docsHint: input.docsHint?.trim() || undefined,
  };
}
