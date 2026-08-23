// `?goto=<page>[:<tab>]` — the PARSE half, pure and node-testable.
//
// The hook itself is old (dev/verification only, inert without the query param)
// but it has grown into load-bearing test infrastructure: `e2e/harness.ts`
// boots every spec through `/?goto=…`, so its normalisation rules are what the
// whole Playwright tier navigates by. Until now those rules lived as four lines
// inside `api-shim.ts`'s bottom-of-file boot block — unreachable from the unit
// gate, and about to be needed by a SECOND shell (`app/`) that has its own
// router and must deep-link the same way or the two shells disagree about what
// `?goto=settings:audio` means.
//
// So the parse moves here. What stays in the shim is the DATA half — the
// `onboardingDone` override in `loadSettingsFromBackend` — because that is a
// settings side effect, not a URL question.
//
// ## The rules, and why each one is here
//
//   `?goto=home`                  → { page: "home" }
//   `?goto=settings:audio`        → { page: "settings", tab: "settings-audio" }
//   `?goto=settings:settings-audio` → same thing; an already-qualified tab id
//                                   is passed through rather than doubled
//   `?goto=settings:notifications` → { page: "settings", tab:
//                                    "settings-notifications" }, a retired id
//                                   from before the 7→5 tab fold that
//                                   `navigateTo`'s TAB_ALIASES maps onward
//   no param, or `?goto=` (empty) → null
//
// The empty case matters and is easy to get wrong: the old code kept the raw
// string and branched on `if (VERIFY_GOTO)`, so `?goto=` (present but empty)
// was FALSY and therefore did nothing at all — no navigation AND no
// onboarding skip. Returning `null` for it keeps both halves exactly as they
// were.
//
// Percent-encoding is handled for free: `harness.ts` writes
// `/?goto=${encodeURIComponent("settings:audio")}`, i.e. `settings%3Aaudio`,
// and `URLSearchParams` decodes it back before the split.

/** Where a `?goto=` wants the app to land. */
export interface GotoTarget {
  /** Page id, e.g. `home`, `search`, `editor`, `settings`. */
  page: string;
  /** Fully-qualified inner tab id (`settings-audio`), when one was asked for. */
  tab?: string;
}

/**
 * Parse a location search string (`location.search`, with or without the
 * leading `?`). Returns `null` when there is nothing to do — no param, or an
 * empty one.
 */
export function parseGoto(search: string): GotoTarget | null {
  const raw = new URLSearchParams(search).get("goto");
  if (!raw) return null;
  // Destructure exactly like the original: anything after a second `:` is
  // ignored rather than being an error.
  const [page, rawTab] = raw.split(":");
  if (!rawTab) return { page };
  // `settings:audio` and `settings:settings-audio` mean the same thing.
  const tab = rawTab.startsWith(`${page}-`) ? rawTab : `${page}-${rawTab}`;
  return { page, tab };
}
