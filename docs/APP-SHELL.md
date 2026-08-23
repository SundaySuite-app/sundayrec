# `app/` — the new shell

Started in S0 of «Frivilligen først». Short on purpose; S1 fills it in.

## What it is

A **second** frontend, in `app/`, built **beside** the shipped one rather than
inside it. The shipped shell is still `legacy/renderer/` — the verbatim port of
the old Electron renderer — and every release is still cut from it. Nothing in
`app/` can affect a release until somebody deliberately points Tauri at it.

The two shells share exactly one thing: the **IPC layer and everything pure
underneath it**, reached through the `@lib/*` alias (`@lib/api-shim`,
`@lib/i18n`, the `*-core.ts` modules). They share no DOM, no stylesheet and no
bundle.

The dependency runs **one way**. `app/` may import from `legacy/` through
`@lib/*`; `legacy/` may never import from `app/` (ESLint enforces it). That is
what keeps `legacy/` something that can eventually be deleted in one piece.

## Why `--mode app` and not a second config

One `vite.config.ts`, one branch, decided by the command you type:

|                    | default                        | `--mode app` |
| ------------------ | ------------------------------ | ------------ |
| root               | `legacy/renderer`              | `app`        |
| dev port           | 1420                           | 1430         |
| build output       | `dist/` (what a release ships) | `dist-app/`  |
| Playwright project | `chromium`                     | `app`        |

There is **no runtime branch** anywhere — no `if (newShell)` in shipped code,
no double bundle. A second config file would have been a second place for the
alias, the CSP assumptions and the build settings to drift apart.

## Why **not** `@preact/preset-vite`

Vite here is **8.x, i.e. rolldown + oxc — not esbuild**. `@preact/preset-vite`
is a Babel plugin, and it is unverified on that stack. So JSX is the
**compiler's**, configured once in `tsconfig.json`:

```jsonc
"jsx": "react-jsx",
"jsxImportSource": "preact"
```

`vite.config.ts` carries `plugins: []` in both modes. `npm run build:app` and
`app/App.test.tsx` are what prove the transform actually resolves.

## The rule

> **`app/**` never imports `@tauri-apps/api/core`.**

The backend is reached through `window.api`, which `@lib/api-shim` installs.
One door into Tauri is what makes the fixture seam (`__SUNDAYREC_FIXTURES__`),
the IPC failure ring and the reachability measurement mean anything at all; a
second door is invisible to all three. Enforced in two places:

- ESLint `no-restricted-imports` on `app/**` — fails at the call site.
- `scripts/check-command-reachability.mjs` — fails if any `app/` file imports it,
  **and** if anyone tries to exempt an `app/` file by adding it to
  `KNOWN_INVOKE_IMPORTERS`.

Three more rules hold in `app/**` and nowhere else, because the old renderer is a
port that had to be excused from them: `any` is an error, `t`/`tf`/`tn` may not
take a fallback argument (a fallback hides a missing key behind correct-looking
Norwegian), and prose written straight into JSX is an error.

## CSP

`app/index.html` carries a `<meta http-equiv="Content-Security-Policy">` that
is **byte-identical** to `app.security.csp` in `src-tauri/tauri.conf.json`.
`legacy/security-sync.test.ts` fails CI if either shell's tag drifts from it.

That is what makes `vite --mode app` in a plain browser enforce the same policy
the shipped WKWebView does — including `script-src 'self'`, which forbids
dynamic code evaluation. CI builds `dist-app` and greps it for exactly that;
`e2e/app/boot.spec.ts` asserts zero `securitypolicyviolation` events at runtime
**and** that the policy is present, so "no violations" can never quietly mean
"no policy".

No inline `<script>`: `app/index.html` loads `/main.tsx` as a module and nothing
else.

## The three commands

```sh
npm run dev:app     # vite --mode app        → http://localhost:1430
npm run build:app   # tsc && vite build      → dist-app/
npm run tauri:app   # the shell in a REAL WKWebView window
```

`tauri:app` is `tauri dev --config src-tauri/tauri.app-shell.conf.json`. That
overlay is three lines — `beforeDevCommand: npm run dev:app` and
`devUrl: http://localhost:1430` — merged over the real `tauri.conf.json` by
Tauri 2's `--config`. The main config is untouched, so `npm run tauri dev` and
every release path still get the legacy shell.

Running the new shell in WKWebView is not optional diligence. Chromium is a
different engine with a different UA string, and SundayEdit's E5 measured a 42×
regression in the real WKWebView that was invisible in Chromium. Anything that
looks right in `npm run dev:app` is unproven until `npm run tauri:app` shows it.

## Tests

- `app/**/*.test.{ts,tsx}` run in the ordinary `npm run test` (vitest, **node
  env**). A component that needs a DOM should be reduced to a pure core the way
  the legacy renderer's `*-core.ts` modules are, rather than dragging jsdom in.
- `e2e/app/*.spec.ts` run in Playwright's `app` project against :1430, started
  by `npm run e2e` alongside the legacy one.

---

# S1a — the foundation

Everything below landed in S1a: the two i18n gates, the state model, the
reactive i18n, the router and `useSetting`. S1b builds the component library on
top of these contracts, so they are the ones that must be right rather than
approximately right.

## i18n: the language is a signal

`app/i18n/index.ts` wraps `t` / `tf` / `tn` / `tArr` so each one reads
`locale.value` on the way past. A component that calls `t()` therefore
**subscribes without knowing it does**, and a language change re-renders exactly
those components.

The legacy shell instead **writes into the DOM**: every node carries
`data-i18n`, and `applyTranslations()` walks the document setting `textContent`
again. That works in a tree nobody else touches. In a Preact tree the next
render erases it — silently, and only sometimes. So `data-i18n` is **forbidden**
in `app/` (gate 2 below), and so are `applyTranslations` / `onLocaleApplied`.

Four rules, each enforced by something rather than remembered:

| rule                                       | enforced by                          |
| ------------------------------------------ | ------------------------------------ |
| no fallback argument — `t(key)`            | ESLint arity + `check-i18n-keys.mjs` |
| every key exists, in the right **form**    | `check-i18n-keys.mjs`                |
| no prose written straight into the tree    | `check-i18n-hardcoded-tsx.mjs`       |
| catalogue is swapped **before** the signal | `app/i18n/i18n.test.ts`              |

That last one is the ordering that cannot be reversed: `setLocale` awaits
`loadLocaleCatalogue` and only then assigns `locale.value`. Flipping the signal
first would give one frame of Norwegian text under an English locale, on every
switch — a bug nobody reports and nobody finds.

`loadLocaleCatalogue` is the one **additive** export S1a added to
`legacy/renderer/i18n.ts`: the data half of `loadLocale`, without the
`document`-touching `applyTranslations()` pass. `loadLocale` calls it and
behaves exactly as before.

### `tDyn` — the one door for a dynamic key

```ts
tDyn("app.page", route.value.page); // → "Ta opp" / "Opptakene" / "Oppsett"
```

One helper, not zero and not many. Zero would mean dynamic keys get written as
template strings inside `t()`, where no gate can see what is being looked up.
The **prefix must be a literal** — the gate resolves it and requires a non-empty
object subtree in both catalogues — and the **suffix is the half no gate can
know**, so a miss **throws in DEV** rather than returning empty text. An empty
label survives a whole test round because it looks like "that one is just
empty".

This is also why `hydrateError` holds a key **suffix** (`"settingsLoadFailed"`)
rather than a whole key: a variable holding `"error.settingsLoadFailed"` would
be invisible to the gate.

### Paused parity

`ACTIVE_LOCALES` is `["no", "en"]` for the duration of the redesign. The other
five catalogues are **paused, not dropped**: `PAUSED_LOCALES` and `PAUSED_KEYS`
in `legacy/locales/parity.test.ts` excuse them **only** for keys the redesign
adds, and only for missing ones — "no extra keys" still holds everywhere, and
every key that existed before is still required in all seven. Translating the
same screen four times while it is still moving is how a translator learns to
stop reading carefully. Fase B empties the list.

A stored language outside `ACTIVE_LOCALES` picks the nearest active one
(`resolveStartupLocale`: sv/da → no, everything else → en) instead of rendering
the redesigned strings as blanks. Nothing is written back to settings.

## The two new gates

Both walk **TypeScript's own AST**, not a regex. In TSX a key can sit in a JSX
attribute, an object literal or behind a template string; a regex over that
answers with false hits where there is nothing and stays quiet where there is
something. They share one walk (`scripts/lib/tsx-i18n-scan.mjs`) so "what is a
call to `t()`" cannot mean two things.

**`check-i18n-keys.mjs`** — every `t/tf/tn/tArr/tDyn` call in `app/` points at a
key that exists in **both** `no.json` and `en.json`, in the form the call
assumes: a string for `t`/`tf`, a CLDR plural group for `tn`, an array for
`tArr`, a non-empty object subtree for `tDyn`. All four failure modes are silent
today — empty text, the singular form for every count, an empty list, a key that
misses. `--unused` lists catalogue keys no `app/` file reads; informative now,
**failing in fase B**, when `app/` is the only reader left.

**`check-i18n-hardcoded-tsx.mjs`** — baseline **0 from day one**, because
`app/` has no debt to pay down. The PROSE regex is copied **verbatim** from
`check-i18n-hardcoded.mjs`: one definition of "prose" in the repo. It counts JSX
text, prose in JSX attributes **and in object properties** (the dialog texts are
properties, not attributes — a gate that only saw JSX would miss every string a
volunteer reads when it matters), and any `data-i18n*`.

Both carry a self-test with a TSX fixture and a known answer, one case per
failure class, and exit 2 before the gate gets to speak. Both run inside the
**existing** i18n steps in `ci.yml`, in `npm run check` and in `ci-local.sh`.

## State: signals at module scope

`app/state/` holds one signal per truth: `settings`, `isRecording`,
`nextRecording`, `prerollActive`, `globalError`.

**Why signals.** A module-scope signal is the closest 1:1 port of the legacy
shell's mutable module variable that exists — same single instance, same
"import it and read it" — except the read is **tracked**. That is what removes
`applyXSettingsToUI()`, `window.loadSettings`, `resyncBoundSettings()` and
`window.__isRecording`: they are all names for the same problem, two places each
believing they know the current value.

**Why module scope and not a context.** Much of this app lives _outside_ the
render tree — the VU meter's rAF loop, recorder events, the pre-roll
reconciliation. They read `signal.peek()` or set up an `effect()` without being
a component, and without anyone building a bridge between "inside the tree" and
"outside" it.

`app/` deliberately does **not** recreate `window.__isRecording`,
`window.loadSettings` or `window.showOnboarding`. `window.showPage` is the one
global it does install, because the tray, the deep links and `e2e/harness.ts`
all rest on it.

### Settings

`hydrateSettings()` reads through `window.api`, so the fixture seam works
unchanged. It then asks the **IPC failure ring** whether `settings_get` actually
failed: api-shim answers a failed read with `SETTINGS_DEFAULTS` so the UI still
renders, which makes a broken store look exactly like a factory-fresh app.
`hydrateError` is what stops that from being silent.

Saving is trailing-debounced and coalescing, and the decisions live in
`settings-save-core.ts` — including the R4 invariant: `payloadFor` sends the
**whole vocabulary**, never a selection. A curated payload is how a field gets
silently re-defaulted in the store (the #113/#115 family), and
`e2e/settings-seam.spec.ts` pins the same thing from outside.

### The three scheduler events

`scheduler://next`, `scheduler://missed` and `scheduler://preflight` are
subscribed **directly**, exactly as `legacy/renderer/status/next-recording.ts`
does and for the reason it writes down: `EVENT_MAP` is the compatibility layer
for **old Electron channel names**, and these three never had one. The rule
`app/` actually has — _the backend is reached through `window.api`_ — is about
**commands**, which is where the fixture seam, the failure ring and the
reachability gate live. An event subscription is not a command.

⚠️ `window.api.on` reaches `__TAURI_INTERNALS__` directly and attaches no
`.catch`, so in a **plain browser with no harness** (`npm run dev:app`) each
subscription logs an unhandled rejection. It does not happen under Tauri, and
`e2e/harness.ts` supplies that runtime — which is why the boot spec goes through
the harness. Worth fixing in the shim one day; it is a legacy question.

## `useSetting` — one save model

```ts
const bitrate = useSetting("bitrate", {
  kind: "select",
  confirmIf: recordingImminentGuard(t("audio.changeBitrate")),
});
// → { value, draft, set, commit, receipt, error, busy, events }
```

The sequence is `validate → guard → confirmIf → apply → persist → receipt |
revert`, and it lives in `use-setting-core.ts` with **every effect injected**, so
the node gate drives all five paths — including the ones that only exist when
something fails, which are exactly the ones nobody tests by hand.

The decisions are **imported, not copied**: `planCommit`, `coerceValue`,
`isRealChange`, `validateNumber`, `guardReasonFor`, `SAVE_COALESCE_MS` and
`SAVED_CHIP_MS` all come from `@lib/ui/bind-setting-core`.

Two deliberate differences from `legacy/renderer/ui/bind-setting.ts`:

- **Revert on failure.** Legacy leaves the value standing when `settings_save`
  fails. The screen then claims one thing and sqlite says another, and the
  change "disappears" at the next launch. `app/` rolls back to what is actually
  stored and toasts `general.saveFailed`. Pinned end to end by
  `e2e/app/settings-revert.spec.ts`.
- **No `resyncBoundSettings`.** The baseline is always `settings.value[key]` —
  the stored value itself. There is nothing to re-sync, and therefore nothing to
  forget.

`useDraftForm(read, write)` is the explicit-save exception, for the two places
where a half-typed value is actively harmful (the schedule slot editor, the one
alert e-mail address). There a failed save does **not** revert: that is something
the user typed, and throwing it away because a disk write failed punishes the
user for the app's problem.

`app/ui/toast.ts` and `app/ui/dialog.ts` carry the **same signatures** as their
legacy counterparts but are queue/signal models with no DOM. S1b mounts the
hosts; until then a toast is invisible but real, and `confirmDialog` is injected
into `useSetting` rather than hard-wired.

## Router: three pages, and one table for everything old

`route` is a signal of `{ page, tab?, anchor?, highlight?, firstRun? }` over
`"record" | "library" | "setup"`. `PAGE_ALIASES` and `TAB_ALIASES` translate
every old id — `?goto=settings:audio` appears in a dozen e2e specs, in the
screenshot passes, in the tray and in links we have not found. Legacy keeps the
same kind of table, and the comment above it says why: it is cheaper than
hunting every call site each time the information architecture moves, and a deep
link that silently opens the wrong tab is worse than one that fails loudly. Every
`?goto=` form in the repo is a row in `app/router/router.test.ts`.

Tray actions set a `pendingAction` signal and navigate; there is no
`getElementById(...).click()`. The legacy hooks synthesise a click on a button
that must exist, on a page that must be showing, in a DOM that must be
finished — three assumptions that have each failed separately.

### Boot order

`app/main.tsx` documents it line by line. The short version: shim →
`setShimNotifier` → render → `installGlobalNavigation` (the contract
`e2e/harness.ts` waits on) → `hydrateSettings` → `setLocale` → stores →
`?goto=` → onboarding gate. The `?goto=` navigation is done here **as well as**
in api-shim's own polling block, so the first frame is already the right page
instead of TA OPP flashing past on the way to OPPSETT; both land on the same
route.

## What is still `TODO(S1b)`

- `app/Shell.tsx` renders the route as a heading and nothing else. `PageShell`,
  the component library and the real screens are S1b's.
- `app/dev/setting-probe.tsx` and the `?probe=` branch exist only so
  `e2e/app/settings-revert.spec.ts` can drive `useSetting` through the real
  seam. They are deleted the moment a real `SettingRow` exists.
- The toast and dialog **hosts** are not mounted yet.
- The tab names in `TAB_ALIASES` (`sound`, `addons`, `files`, `advanced`,
  `schedule`, `edit`) are S1a's placeholders for the new IA. Renaming them is a
  one-file change, on purpose.

---

# S1b — the component library and the shell

S1a built the foundation; S1b builds the only pieces the new UI is allowed to
be assembled from, and the shell that holds them. There are still **no
screens** — those are fase P. What exists is the rail, the three destinations,
the status line, the two overlay hosts, and every building block a page will
need.

The design canvas from fase D is committed at
`docs/design/canvas/FASE-D-UTKAST-1.html`. The tokens and the shapes in it are
the source of truth: the owner may adjust colours later, so **everything
visual goes through `app/styles/tokens.css`**, never a literal.

## The colour gate

`scripts/check-app-css-tokens.mjs` rejects `#rrggbb`, `rgb()`, `rgba()`,
`hsl()` and `hsla()` in every `app/**/*.css` **except `app/styles/tokens.css`**.
One finding fails it, from day one, because `app/` has no debt to pay down.

The reason is not tidiness. `legacy/renderer/styles.css` has a perfectly good
`:root` block with 52 variables — and over a thousand hardcoded colours
underneath it. Each one was reasonable in the moment ("almost that blue, a
little darker"); together they mean the owner cannot recolour the app in one
place, because most of the colours do not live there. That is an ownership
failure, not a style one.

The gate carries a self-test with a known answer (including a colour inside a
CSS **comment**, which must NOT count — a gate that shouts at its own
documentation gets switched off within a week). It runs in `npm run check`,
`scripts/ci-local.sh` and `ci.yml`, in the same step group as the i18n gates.

## The font

**System stack for now.** `--font` in `tokens.css` is the legacy shell's
`-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif`.

Canvas set 0 says «Hanken Grotesk overalt i appen», and the face is SIL OFL
1.1, i.e. free to bundle. But the app is offline and CSP-locked
(`default-src 'self'`), so a Google Fonts `<link>` would be **blocked in
WKWebView and work in the browser** — the worst of all failure modes. The only
route is local `.woff2` files in the repo, and pulling binaries into the tree
is a decision the owner takes, not one an agent takes while they are away (it
also brings the OFL licence text along as a file we then maintain).

Switching is one change here plus four files in `app/styles/fonts/`:

```css
@font-face {
  font-family: "Hanken Grotesk";
  src: url("./fonts/HankenGrotesk-400.woff2") format("woff2");
  font-weight: 400;
  font-display: swap;
}
/* …500, 600, 700… */
--font:
  "Hanken Grotesk", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui,
  sans-serif;
```

Budget for the swap: < 400 kB for 400/500/600/700 combined.

`font-variant-numeric: tabular-nums` is on `body`, so every time, level and
duration in the app has non-shifting digits without anyone remembering.

## The contract every component keeps

- `testId` in, `data-testid` on the **root**.
- Composite components derive: `-control`, `-receipt`, `-error`, `-label`,
  `-banner`, `-title`, `-description`, `-row-<id>`. Convention:
  `<area>-<thing>[-<qual>]`.
- `class`, never `className` — lint-enforced (`no-restricted-syntax` in the
  `app/**` block). Preact accepts both, which is exactly the problem: two
  spellings mean every grep for a class name misses half the hits.
- Variants and states are also **attributes** (`data-variant`, `data-tone`,
  `data-state`, `data-status`, `data-word`), so a test asserts the contract
  rather than a hashed CSS-module class name.

`app/ui/library.test.tsx` is one table with a row per component. A component
without a row fails the count assertion — twenty near-identical test files
would test the same sentence twenty times and let the twenty-first component
slip through, because nobody wrote its file.

## The library

| component     | notes                                                                                                                                                                   |
| ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Button`      | 5 variants (`primary` / `secondary` / `ghost` / `danger` / `record`), 2 sizes, `busy`. **`disabled` requires `disabledReason`** — see below.                            |
| `Card`        | `title` / `description` / `actions` / `tone` (neutral, warn, bad, good, selected) / `anchor` for navigate-highlight.                                                    |
| `SettingRow`  | Owns the four fixed places: label, receipt **at the label**, description, control, error line **under the control**. Hands ids to the control through a child function. |
| `Toggle`      | `<button role="switch" aria-checked>`. No hidden checkbox.                                                                                                              |
| `Select`      | A real `<select>` — the OS list, keyboard, type-ahead and scrolling for free.                                                                                           |
| `RadioCards`  | Real `<input type=radio>` in a `radiogroup`; each option gets a description, at most one gets `recommended`.                                                            |
| `TextField`   | `onInput` per keystroke, `onCommit` on blur/Enter — `useSetting` needs both.                                                                                            |
| `NumberField` | `rule` → `validateNumber` from `bind-setting-core`. Exports `checkNumberInput` so field, hook and page ask exactly one question.                                        |
| `Slider`      | `onInput` while dragging, `onChange` on release (= `planCommit('slider')`).                                                                                             |
| `Receipt`     | `role="status"`, four states, holds its space when idle.                                                                                                                |
| `Gate`        | `mapGate` from `feature-gate-core`; children get `inert`; the reason stays outside the inert region so it can still be read.                                            |
| `EmptyState`  | Title, why it matters, **one** action.                                                                                                                                  |
| `StatusDot`   | good / warn / rec / listen / neutral. `rec` pulses; reduced-motion turns it off and nothing is lost.                                                                    |
| `Chip`        | A fact next to something else. Never a button.                                                                                                                          |
| `Tabs`        | `role=tablist`, one tab stop, arrow keys + Home/End. For choices **within** a screen — never navigation.                                                                |
| `ProgressBar` | `formatEta` + `formatPercent` from `progress-core`.                                                                                                                     |
| `Banner`      | `bad` (`role=alert`) / `warn` (`role=status`). No `info` tone.                                                                                                          |
| `DialogHost`  | See below.                                                                                                                                                              |
| `ToastHost`   | Bottom-right stack, `aria-live="polite"`, house durations (an **error toast never auto-dismisses**).                                                                    |
| `PageShell`   | Rail + three destinations + status line + version, `<main id="main">`.                                                                                                  |
| `VuMeter`     | See below.                                                                                                                                                              |
| `Bound*`      | `BoundToggle` / `BoundSelect` / `BoundRadioCards` / `BoundTextField` / `BoundNumberField` = `useSetting` + `SettingRow` + control.                                      |

### A disabled button must say WHY

`disabled` alone does not exist here. A button that is off takes
`disabledReason`, and that reason lands in three places at once: `title`,
`aria-describedby`, and a visually-hidden `<span>`.

And it is `aria-disabled`, not the `disabled` attribute. A real `disabled`
takes the button out of the tab order — so a keyboard user cannot even reach
it to **hear** why it is off. The click is stopped in the handler instead.

The legacy shell has grey buttons everywhere; «Start opptak» is the worst of
them: grey because no audio source is chosen, and that is written nowhere.

### DialogHost lives OUTSIDE `#app`

`app/index.html` has **two** mount points: `#app` and `#overlays`. `main.tsx`
renders `<Shell>` into the first and `<Overlays>` (DialogHost + ToastHost) into
the second.

The reason is `inert`. While a dialog is open the rest of the app must be
completely unreachable — not merely dimmed — and that is done by setting
`inert` on `#app`. A dialog rendered _inside_ `#app` would switch itself off.

`inert` is set in an effect there, and that is safe **because `#app` is the
mount point**: Preact renders children into it and never touches the element's
own attributes, so no re-render can strip it. Everywhere `inert` lands on an
element Preact actually owns (see `Gate`) it is a JSX prop, because there an
imperative write _would_ be stripped by the next render — silently, and only
sometimes.

Decisions are borrowed, not rewritten: `buildConfirm` and `nextFocusIndex` come
from `@lib/ui/dialog-core`. That gives us `data-dialog-button="ok" | "cancel"`
(which a dozen specs rest on), the variants, and the one that matters — **on a
dangerous dialog CANCEL is the Enter choice and confirm is a red SECONDARY
button, never a red primary** (canvas set 7).

Focus return is harder than `document.activeElement`: on macOS a `<button>`
does **not** take focus from a click, so at open time `activeElement` is often
`<body>`. The host therefore also tracks the last `pointerdown` and uses that
element as the fallback — the "explicit trigger ref", without every call site
having to remember to pass one.

### VuMeter

- `acquireVuFeed` in `useEffect`, `release()` on unmount. Rust owns the device;
  the renderer only ever listens. That is the Qu-5 failure class (2026-07-31),
  so the release is the whole contract, not tidiness.
- **One stable canvas ref**, never swapped. Drawing runs in a rAF loop outside
  render; ~30 packets/s go into a `ref`, and the only thing that triggers a
  re-render is the WORD changing.
- Smoothing is `createLevelSmoother` from `@lib/audio/smoothing` — the house's
  single law of motion.
- **The fill is relative to the WHOLE bar.** The colour band is fixed (green to
  72 %, amber to 90 %, red above) across the bar's FULL width, and the fill
  clips it at the level. A gradient across the fill itself looks almost the
  same and is wrong: a low level would then show red at its own right edge,
  i.e. "too loud" at −40 dB. The canvas's first draft made exactly that mistake.
- **No numbers at level 1** (canvas set 0). `showNumbers` exists for Avansert
  and the recording overlay.
- The words come from `app/audio/level-words.ts`: thresholds as constants
  (`HEARD_DB` = −50, the recorder's own silence default; `LOUD_DB` = −3, the
  margin against clipping), table-tested, and read from PEAK rather than RMS
  because "too loud" is about the peaks.

### `WaveformHost` — the contract for P4

Not written in S1b. When fase P builds the editor it must keep to this shape,
for the same reasons `VuMeter` does:

1. **One stable `<canvas>` ref** plus one for the minimap. Never remounted: a
   canvas that remounts loses its context, and a draw loop holding the old
   context paints into an element nobody sees — the waveform "freezes" without
   anything erroring.
2. **`ResizeObserver` → `syncCanvasSize`**, not a `window.resize` listener. The
   waveform also gets narrower because something beside it grew, and `resize`
   never sees that. Writing `canvas.width` clears it, so only write it when the
   value actually changed.
3. **`effect()` subscribes the draw scheduler** to the signals it reads
   (selection, zoom, playhead), and the scheduler coalesces into one rAF. A
   component that redrew per signal write would paint three times per frame.
4. **Unmount cancels**: `cancelAnimationFrame`, `observer.disconnect()`, and the
   effect's own dispose. A live rAF after unmount is how a "closed" editor keeps
   costing a frame budget the recording needs.
5. Decoding and peak extraction stay **outside** the component, keyed by
   recording, so switching tabs does not throw away work.

## The status line

One sentence, always true, never free text. Five possibilities and no more:

| kind      | dot   | means                                               |
| --------- | ----- | --------------------------------------------------- |
| `rec`     | red   | it is recording NOW — red never means anything else |
| `lowdisk` | amber | under two hours of room left                        |
| `nosound` | amber | no source chosen                                    |
| `next`    | grey  | automatic recording is on, and the time is known    |
| `ready`   | green | source chosen, space on disk                        |

Priority: **`rec` > `lowdisk` > `nosound` > `next` > `ready`.**

`rec` first because a running take is the most important fact on screen.
`lowdisk` **before** `nosound` — both are amber, but a full disk stops the
recording in the middle of the service while a missing source stops it before
it starts, and the first is the one you have least time to discover. `ready`
last, and only when none of the others apply: the app does not say "All set"
while something is not.

`app/state/status-line.ts` is a pure function, table-tested over every
combination and both sides of the disk threshold. `formatNextWhen` is kept out
of it: formatting depends on which ICU build node/WebKit carries, and mixing
that into the priority table would make the table brittle for a reason that has
nothing to do with priority.

`roomMinutes` comes from `app/state/disk.ts`. ⚠️ Its arithmetic (kbps per
format, `bytes / (kbps · 125)`) is a **copy** of the private `loadDiskSpace()`
in `legacy/renderer/pages/home.ts`, because the original sits inside a
1500-line DOM module `app/` cannot import. The copy is deliberate, pure and
tested; fase P folds the two back into one when the home page is ported.

## What each destination shows today

There are no screens yet, so each destination shows **the part of itself that
is already true** — and only that part. Nothing says «coming later», and no
button exists that does not do something. A dead button teaches a volunteer
that the buttons in this app cannot be trusted, and that lesson outlives the
button.

- **OPPTAK** — no source chosen → the card that says so, and a button to
  OPPSETT. Source chosen → the real `VuMeter`, answering "do we hear it?".
- **BIBLIOTEK** — the count is read for real (`recordings_list` via
  `getHistory`). `null` → no claim at all. `0` → the empty state. More → where
  they are. Never "no recordings yet" on a machine with twelve.
- **OPPSETT** — the five questions with the answer that stands now, «Ikke satt
  opp» where nothing has been answered, and the row turns amber. No «Endre»
  button, because the screens it would open do not exist yet.

The route detail (`tab`, `anchor`, `firstRun`) that S1a rendered as small
paragraphs is now **attributes** on `<main>` (`data-page`, `data-tab`,
`data-anchor`, `data-first-run`). It was debugging text in an app a volunteer
is meant to read; it is now something only e2e sees.

## The one legacy change

`window.api.on()` in `legacy/renderer/api-shim.ts` attached no `.catch` to
`listen(...)`, and `listen` reaches `__TAURI_INTERNALS__` directly. Outside
Tauri — a plain `npm run dev:app`, or any page without the e2e harness — every
subscription rejected, so each one became an **unhandled promise rejection**:
four red lines on boot in a console people are supposed to read for real
problems, and (because `app/state/global-error.ts` listens for
`unhandledrejection`) a shell that reported a global error before it had
finished waking up.

Inside Tauri `listen` never rejects, so **the shipped app is unchanged**. The
only visible difference is that a browser boot stops shouting, and that `on()`
keeps its promise either way: it always returns an unsubscribe that is safe to
call. The warning is once per **channel**, not per call.

Pinned two ways: `legacy/renderer/api-shim-listen.test.ts` (node, with the
internals deliberately absent) and a bare-`page.goto` case in
`e2e/app/boot.spec.ts` — S0's boot spec had to go through the harness for
exactly this reason, and no longer does.

## i18n

All new keys live under **`app.*`** in `legacy/locales/{no,en}.json` — 38 keys,
only the ones actually rendered today. They are listed in `PAUSED_KEYS`
(`legacy/locales/parity.test.ts`) so the five paused languages are excused until
fase B, and both gates stay at their baselines (`check-i18n-keys` green,
`check-i18n-hardcoded-tsx` at 0).

`app.page.*` changed value: the destinations are **Opptak · Bibliotek ·
Oppsett** (canvas set 1's decide block), where S1a's placeholders read «Ta
opp» / «Opptakene».

Two shapes are worth knowing:

- A product name (`SundayRec`) or a unit (`dBFS`) is a **module constant**, not
  JSX text. Neither is translated, but the prose gate cannot know that and
  should not have to guess.
- A dynamic label goes through `tDyn` with a **literal prefix**:
  `tDyn("app.vu", word)`, `tDyn("app.page", page)`,
  `tDyn("app.setup.quality", format)`. A `Record<Word, "app.vu.x">` lookup table
  is exactly the shape `check-i18n-keys.mjs` cannot see into.

## Proven in a real WKWebView

Chromium is not the engine this app ships in, and SundayEdit's E5 measured a 42×
regression in the real WKWebView that was invisible in Chromium. So S1b was run
once through `npm run tauri:app` and asked what it could actually see.

`screencapture` is unreliable under TCC on this machine, so the evidence is a
**probe** instead of a screenshot: a temporary Vite plugin (never committed)
serves a module — `script-src 'self'` forbids an inline one — that collects DOM
state, computed styles, CSP violations and errors, clicks through the three
destinations, and POSTs the result back to the dev server (`connect-src 'self'`
allows same-origin). What came back:

```jsonc
{
  "ua": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)",
  "hasSafariToken": false, // ⚠️ see below
  "railPresent": true,
  "railHasDragRegion": true,
  "navButtons": ["Opptak", "Bibliotek", "Oppsett"],
  "heading": "Opptak",
  "church": "Ikke satt opp ennå", // warn colour — churchName is empty
  "statusText": "Alt er klart",
  "statusKind": "ready",
  "statusDotTone": "good",
  "overlaysRootIsSibling": true, // #overlays is NOT inside #app
  "goldToken": "#ebb84b",
  "bodyBg": "rgb(14, 19, 33)", // --bg  #0E1321
  "railBg": "rgb(21, 27, 43)", // --surface #151B2B
  "cssModulesApplied": true,
  "vuCanvasSized": "1716x72", // ResizeObserver + DPR 2 → 858×36 CSS px
  "destinations": ["library=Bibliotek", "setup=Oppsett", "record=Opptak"],
  "cspViolations": [],
  "errors": [],
  "rejections": [],
  "consoleErrors": [],
}
```

⚠️ **`hasSafariToken: false`.** The UA string this WKWebView sends carries no
`Safari` token. That is the exact fact behind SundayEdit's 42× PixiJS
regression — libraries that sniff for Safari to pick a code path see "unknown
engine" here and take their slowest one. Nothing in S1b depends on it, but any
future dependency that branches on the UA must be measured **in this engine**,
not in `npm run dev:app`.
