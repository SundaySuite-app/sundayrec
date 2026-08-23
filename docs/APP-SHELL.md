# `app/` — the shell

> **⚠️ THE PARALLEL PERIOD IS OVER.** Fase B made `app/` the shipped frontend:
> PR A deleted `legacy/renderer/`'s shell, PR B moved what was left of it to
> `app/lib/`. Everything below from «S1a» down was
> written while there were two, and is kept as the record of WHY each contract
> is shaped the way it is — the reasoning is still load-bearing even where the
> tense has moved on. What changed at the switch is written here, at the top,
> and the standing list of what is still owed is the last section of this file,
> **«Etter byttet»**.

## What it is

**The** frontend. Vite's root is `app`, the dev server is :1420, `npm run build`
writes `dist/`, and `dist/` is what `tauri.conf.json`'s `frontendDist` bundles.
There is one shell, one Playwright project, one place a screen can live.

It was a SECOND frontend for the length of S0–P4, built beside the shipped one
rather than inside it, so that nothing here could affect a release until
somebody deliberately pointed Tauri at it. That is what fase B did — by moving
the root and the port rather than by adding a switch, so there is still no
runtime branch anywhere and never was.

What survived the delete is the **inventory**: the IPC layer and everything
pure underneath it, reached through the `@lib/*` alias (`@lib/api-shim`,
`@lib/i18n`, the `*-core.ts` modules). 40 source files, down from 132.

**PR B moved it to `app/lib/`** — 76 files (the sources and their tests) by
`git mv` and nothing else, plus the one line in `vite.config.ts` the alias
existed to make possible. What is left under `legacy/` is generated code and
data: `bindings/`, `locales/`, `types/`, `shared/`, reached from the shell
through a second alias, **`@legacy/*`**.

> `@legacy/*` is new, and it is the one import spelling PR B had to change:
> the shell used to reach the bindings as `@lib/../bindings/X` — walking OUT of
> one alias with `..` to land in a directory that only happened to be its
> neighbour. 24 files did it, and all 24 broke the moment `@lib` moved. An alias
> written relative to another alias is a dependency on where the other one
> points, not on what is being imported.

The dependency still runs **one way**, and now says so from inside `app/`:
`app/lib/` may never import the shell around it. ESLint enforces it per
directory depth, since inside `app/` there is no longer an `app/` segment for a
glob to match on.

`app/lib/` is a **verbatim port**, and PR B kept it one rather than promoting it
by moving it:

- its own ESLint block (`any` and `no-unused-vars` loosened, exactly the rules
  the `legacy/**` block gave it), so nobody reads a file under `app/` and assumes
  it has been through the review the shell has;
- still outside `prettier` (`.prettierignore` carries `app/lib`), because a
  reformat is a whole-file diff on 76 files and would have buried the proof that
  the move changed nothing;
- and outside the two i18n AST gates, which forbid the fallback argument
  (`t(key, 'norsk')`) that IS the port's signature — a gate that doubled its
  reach because a directory moved would be a rule change dressed as a rename.

Each is undone **file by file, on touch**: when one of these is opened for a real
reason, it leaves the ignore and the block in the same PR as the change that made
you open it.

## Why **not** `@preact/preset-vite`

Vite here is **8.x, i.e. rolldown + oxc — not esbuild**. `@preact/preset-vite`
is a Babel plugin, and it is unverified on that stack. So JSX is the
**compiler's**, configured once in `tsconfig.json`:

```jsonc
"jsx": "react-jsx",
"jsxImportSource": "preact"
```

`vite.config.ts` carries `plugins: []`. `npm run build` and the component tests
are what prove the transform actually resolves.

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
`legacy/security-sync.test.ts` fails CI if the tag drifts from it — and its
covers-every-shell assertion is kept rather than flattened, because what it
guards against is a SECOND `index.html` arriving with no policy check at all,
which looks exactly like green.

That is what makes `npm run dev` in a plain browser enforce the same policy the
shipped WKWebView does — including `script-src 'self'`, which forbids dynamic
code evaluation. CI builds `dist` and greps it for exactly that;
`e2e/boot.spec.ts` asserts zero `securitypolicyviolation` events at runtime
**and** that the policy is present, so "no violations" can never quietly mean
"no policy".

No inline `<script>`: `app/index.html` loads `/main.tsx` as a module and nothing
else.

## The three commands

```sh
npm run dev        # vite                → http://localhost:1420
npm run build      # tsc && vite build   → dist/  (what a release bundles)
npm run tauri dev  # the shell in a REAL WKWebView window
```

There were three parallel ones (`dev:app` / `build:app` / `tauri:app`, on 1430,
into `dist/`, through the overlay config
`src-tauri/tauri.app-shell.conf.json`) for as long as both shells existed. Fase
B deleted all four: with one shell there is nothing to select between, and a
second spelling of "run the app" is a second place for the alias, the CSP
assumptions and the build settings to drift apart.

Running the shell in WKWebView is not optional diligence. Chromium is a
different engine with a different UA string, and SundayEdit's E5 measured a 42×
regression in the real WKWebView that was invisible in Chromium. Anything that
looks right in `npm run dev` is unproven until `npm run tauri dev` shows it.

## Tests

- `app/**/*.test.{ts,tsx}` run in the ordinary `npm run test` (vitest, **node
  env**). A component that needs a DOM should be reduced to a pure core the way
  the shared `*-core.ts` modules are, rather than dragging jsdom in.
- `e2e/*.spec.ts` run in Playwright's one `chromium` project against :1420,
  started by `npm run e2e`. They lived under `e2e/app/`, in a second project on
  :1430, until fase B; the move needed no path change in `docs/SMOKE-TEST.md`,
  because every copied test kept its title byte-identical for exactly that day.

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

`loadLocaleCatalogue` is the one **additive** export S1a added to the locale
loader (`legacy/renderer/i18n.ts` then, `app/lib/i18n.ts` since PR B): the
data half of `loadLocale`, without the `document`-touching
`applyTranslations()` pass. `loadLocale` calls it and
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
stop reading carefully.

✅ **Fase B did half of it, and the useful half first.** 653 keys nothing reads
were deleted from all seven catalogues before any translating starts — the five
paused languages went from 895 keys to 244. `PAUSED_KEYS` is 442 (the `app.*`
surface). PR B is done; the translation round itself is what comes next.

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
**failing since fase B**, when `app/` became the only reader left — and it
counts a key named by a shared `@lib/*` core as read, because those modules
answer with keys and the page then calls `t(k)` with a variable the AST walk
cannot resolve.

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
`.catch`, so in a **plain browser with no harness** (`npm run dev`) each
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
  `e2e/settings-revert.spec.ts`.
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
  `e2e/settings-revert.spec.ts` can drive `useSetting` through the real
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

`window.api.on()` in the IPC shim (`legacy/renderer/api-shim.ts` then,
`app/lib/api-shim.ts` since PR B) attached no `.catch` to
`listen(...)`, and `listen` reaches `__TAURI_INTERNALS__` directly. Outside
Tauri — a plain `npm run dev`, or any page without the e2e harness — every
subscription rejected, so each one became an **unhandled promise rejection**:
four red lines on boot in a console people are supposed to read for real
problems, and (because `app/state/global-error.ts` listens for
`unhandledrejection`) a shell that reported a global error before it had
finished waking up.

Inside Tauri `listen` never rejects, so **the shipped app is unchanged**. The
only visible difference is that a browser boot stops shouting, and that `on()`
keeps its promise either way: it always returns an unsubscribe that is safe to
call. The warning is once per **channel**, not per call.

Pinned two ways: `app/lib/api-shim-listen.test.ts` (node, with the
internals deliberately absent) and a bare-`page.goto` case in
`e2e/boot.spec.ts` — S0's boot spec had to go through the harness for
exactly this reason, and no longer does.

## i18n

All new keys live under **`app.*`** in `legacy/locales/{no,en}.json` — 38 keys,
only the ones actually rendered today. They are listed in `PAUSED_KEYS`
(`legacy/locales/parity.test.ts`) so the five paused languages are excused until
fase B's translation round, and both gates stay at their baselines
(`check-i18n-keys` green,
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
once through `npm run tauri dev` and asked what it could actually see.

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
not in `npm run dev`.

---

# P1a — Oppsett, den første ekte siden

S1b bygde delene; P1a er det første stedet de settes sammen til noe en
frivillig kan bruke. **65 kontroller i fem faner er foldet til fem spørsmål**,
med en skjerm bak hvert av dem, og to tillegg som utvider siden når de slås på.
Alt ligger i `app/pages/setup/`.

|                        | før (`legacy/renderer`)                          | nå (`app/pages/setup`)                  |
| ---------------------- | ------------------------------------------------ | --------------------------------------- |
| «Hvilken lyd?»         | enhetsliste + 32-ruters rutenett + måler på Hjem | én skjerm: enhet, kanalpar, hørselstest |
| «Hvor skal opptakene?» | Filer-fanen, 15 kontroller                       | sti + plass i TIMER + «Velg mappe …»    |
| «Hvilken kvalitet?»    | 4 formater × 3 bitrater, ingen forklaring        | tre kort med en begrunnelse hver        |
| «Hvilken kirke?»       | System-fanen, mellom logg og telemetri           | navn + språk, og ingenting annet        |
| «Hvem får beskjed?»    | Deling-fanen, 16 kontroller                      | ett varsel, én adresse, én test         |

## Fem spørsmål, én tabell — `decisions-core.ts`

Om et spørsmål er besvart, hva svaret ER og hvorfor det ikke holder, avgjøres i
en REN fil med en test per rad. Ingen i18n, ingen DOM: kjernen svarer med DATA
(`{ key: "deviceMissing", name }`), og siden oversetter.

Grunnen står i atlaset. Dagens enhetskort maler **«Innebygd mikrofon ·
Tilkoblet ✓» når `deviceId` er `null`** — altså «alt er i orden» om en
innstilling ingen har satt. En slik regel kan ikke bo i en `&&` inne i en
JSX-linje; der leses den aldri to ganger.

**Tre tilstander, ikke to.** `done` og `todo` er canvasens. Den tredje,
`unknown`, finnes fordi enhetslisten og ledig diskplass leses ASYNKRONT etter
første maling: regelen «ikke funnet ⇒ todo» ville gjort hver kaldstart til et
gult kort som blir nøytralt etter 100 ms, og et gult kort som forsvinner av seg
selv er nettopp det som lærer folk å ignorere gult. `unknown` er nøytralt, sier
ingenting, og er ALDRI `answered`.

**`needsSetUp` er ikke `answered`.** Knappen sier «Sett opp» bare når det ikke
STÅR et svar. En mappe som er valgt, men der disken ikke har rukket å svare, er
noe man endrer.

## Skjøten som ble lukket: statuslinjen og spørsmål 1

`statusLine`s `nosound` spurte bare om det sto noe i `deviceName`. Skinnen kunne
derfor si «Alt er klart» på nøyaktig den samme skjermen der spørsmål 1 sto gult
og sa «Finner ikke Behringer X32» — to sanne halvdeler som er uenige i skjøten,
side om side, synlig i ett blikk.

Den PURE `statusLine` er urørt; det er INPUTEN som er rettet, gjennom
`soundChosen(settings, devices)` i `app/state/devices.ts`: valgt betyr valgt OG
til stede. `devices === null` (ikke lest ennå) faller tilbake på det lagrede —
TA OPP leser ikke enhetslisten, så der er svaret det samme som før.

## Overskriften bytter, destinasjonen gjør det ikke

`PageShell` tar nå imot en `heading`. Skinnen står på OPPSETT hele veien, men
`<h1>` blir spørsmålet: skjermen HANDLER om «Hvilken lyd?», og siden fokus
flyttes til `<h1>` ved hvert rutebytte er det også det første en
skjermleserbruker hører. Fokuseffekten ser nå på `route.tab` i tillegg til
`route.page`, fordi de fem spørsmålene er egne skjermer.

## `TAB_ALIASES` har ekte navn nå

S1a satte plassholdere fordi informasjonsarkitekturen ikke fantes. Hver rad
peker på en skjerm som er bygget:

| gammel id                                                          | nytt mål                 |
| ------------------------------------------------------------------ | ------------------------ |
| `settings-audio`                                                   | `sound` — spørsmål 1     |
| `settings-files`                                                   | `folder` — spørsmål 2    |
| `settings-sharing` · `settings-publish` · `settings-notifications` | `notify` — spørsmål 5    |
| `settings-video`                                                   | anker `camera` på nivå 1 |
| `schedule`                                                         | anker `auto` på nivå 1   |
| `settings-general`                                                 | `advanced` ⚠️ (P1b)      |

De tre gamle delings-id-ene lander samme sted fordi de BESKRIVER samme sted:
etter #139 inneholder den gamle Deling-fanen bare seksjonen «Varsler».

⚠️ `advanced` er den ENESTE raden som peker på noe som ikke finnes ennå.
`SetupPage` rendrer nivå 1 for den — siden Avansert nås FRA — og `data-tab`
står likevel på `<main>`, så dyplenken er intakt den dagen P1b bygger den.
`app/router/router.test.ts` har en vakt som slipper `advanced` og `edit` og
ingenting annet, så en sjette plassholder ikke kan sige inn ubemerket.

## Shimmens `?goto=`-gjentakelse er ikke idempotent

api-shimmens egen `?goto=`-blokk navigerer PÅ NYTT 150 ms etter modullast.
Det var dokumentert som «en idempotent gjentakelse», og det er det — helt til
noe navigerer i mellomtiden. Da river gjentakelsen skjermen tilbake til
dyplenken, og brukeren står et sted hun ikke valgte. (Et menneske rekker det
sjelden; et e2e-spec rekker det hver gang, og det var slik det ble funnet.)

`app/main.tsx` gir derfor shimmen — og `window.showPage`, som shimmen bruker
når dyplenken ikke har en fane — en `navigate` som slipper ÉN gjentakelse av
den dyplenken som allerede har landet, og ingenting annet. En engangsbillett og
ikke en tidsgrense: shimmen gjentar nøyaktig én gang, så det er den samme
grensen uten å gjette et tall. `installGlobalNavigation(nav?)` tar imot
overstyringen. Legacy er urørt.

## Tre steder auto-anvend IKKE er svaret

`useSetting` eier én nøkkel og har én kvittering. Disse tre skriver FLERE
nøkler som må lande sammen, og gjør det med én eksplisitt handling og én
lagring — akkurat som legacy `selectDevice` gjør:

- **enhetsvalget** (`deviceId` + `deviceName` + `deviceChannels[id]` +
  `channels`) — «Bruk denne», med `recordingImminentGuard` bare ved BYTTE,
- **kameravalget** (`videoDeviceName` + `videoDeviceIndex`),
- **OS-varselet** (`notifyStart` + `notifyStop` bak ÉN bryter).

`useDraftForm` er de to stedene et halvskrevet felt er aktivt skadelig:
varsel-adressen og den ukentlige tiden.

⚠️ `scheduler_reschedule` kalles ALDRI fra en side: `window.api.saveSettings`
gjør det selv etter hver skrivning. Det samme gjelder OS-innloggingselementet
(`syncLaunchAtLogin`).

## ⚠️ «Ta opp automatisk» av/på sletter tiden — en eiersak

`Settings` har ingen `enabled`-flagg, verken på en slot eller på planen:
bakenden kjenner bare `slots: ScheduleSlot[]`, og en tom liste ER «av». Så det
er det bryteren skriver, og «av» fjerner tidspunktet fra basen.

Skjermen demper det den kan uten å lyve: den siste planen huskes i ØKTEN, og en
profil med FLERE tidspunkter får et spørsmål med antallet i seg før de
forsvinner. Å løse det ordentlig krever en ny nøkkel i Rust, og det er eierens
valg — ikke noe en skjerm legger til fordi en bryter gjerne vil oppføre seg
penere.

## To usanne setninger fra canvasen som ikke ble med

- **«E-posten sendes via SundaySuite.»** Det finnes ikke noe slikt relé.
  Sendingen går gjennom menighetens EGEN SMTP-server, og uten en slik server
  kommer ingenting fram uansett hva som står i adressefeltet. Bryteren står
  derfor bak en `Gate` som sier det, og gaten er trygg her fordi SMTP-feltene
  bor under Avansert — `feature-gate-core` advarer mot det motsatte.
- **«Varsel på maskinen … Alltid på.»** Ved siden av en bryter som kan slås av.
  Teksten sier nå hva bryteren gjør.

Kirkekortet arvet heller ikke atlasets tredje døde påstand: `churchName` brukes
IKKE i filnavn, og podkast-RSS ble fjernet i #139.

## «Avansert» finnes ikke ennå, og det står det ingenting om

Canvasen har en «Avansert»-lenke nederst på nivå 1. Den er IKKE med: skjermen
den skulle åpne bygges i P1b, og en lenke til en tom side — eller til en tekst
som sier «kommer senere» — lærer en frivillig at lenkene i denne appen ikke er
til å stole på. Den legges til sammen med siden den åpner. Av samme grunn er
«Avansert lyd» borte fra spørsmål 1.

## Ingen nye `tn()`-nøkler

`check-i18n-plurals.mjs` krever hver flertallsgruppe i **alle sju** språk med
riktige CLDR-kategorier, og har INGEN unntak for de pausede fem — i motsetning
til `parity.test.ts`. En ny `tn()`-nøkkel ville altså krevd polske og franske
flertallsformer midt i en pause som finnes for å slippe akkurat det. Så: `tf()`
med en formulering som er riktig for tallområdet den faktisk viser
(«Miksebord · {n} kanaler» vises bare for n ≥ 3).

## e2e: de fire re-pekte, med byte-identiske titler

`e2e/{settings,settings-seam,settings-migration,i18n-live-surfaces}.spec.ts`
er kopier av legacy-versjonene med **hver test-tittel uendret**, fordi
`docs/SMOKE-TEST.md` peker på dem ved navn: den dagen legacy slettes skal
pekeren flytte seg ved å bytte filbanen og ingenting annet. Legacy-filene står
urørt og grønne.

`settings-seam` er den ene fila der også assertionene er ordrette. Den tester
ikke UI, den tester R4-invarianten på SØMMEN — den ene tingen begge skallene
deler — og halve invarianten ville vært udekket hvis bare det ene skallet ble
sjekket.

Kontrollene er byttet der legacy driver noe P1b eier: `opt-ask-open-editor` ble
«Ta med kamera» (`videoEnabled`), og `askOpenEditor` er med videre som et
URØRT felt med den samme assertionen på den samme verdien.

---

# P1b — Avansert, første gang, og nøkkelen eieren sa ja til

P1a foldet 65 kontroller til fem spørsmål. P1b bygger det SJETTE stedet — det
alt som ikke er et spørsmål bor — og den ene skjermen som kommer før alle de
andre.

|                      | før (`legacy/renderer`)                                  | nå (`app/`)                                       |
| -------------------- | -------------------------------------------------------- | ------------------------------------------------- |
| Avansert             | spredt over Lyd, Filer, Deling og System                 | én liste, ett ord per rad (`AdvancedPage.tsx`)    |
| E-postserver (SMTP)  | `<details>` inne i kortet gaten selv slo av              | eget kort på Avansert — gaten på 5.3 åpner det    |
| Tidsplan, avansert   | månedskalender + dagsdetalj + vekke-diagnosekort (23 kt) | to lister og én setning (`advanced/ScheduleCard`) |
| Første gang          | 521 linjer veiviser med sine egne skjermer               | de fem ekte skjermene i sekvens (`FirstRun.tsx`)  |
| Diagnostikk-samtykke | steg 5 av 6 i veiviseren                                 | ett kort på OPPTAK (`ui/ConsentCard`)             |

## `autoRecordEnabled` — P1as eierspørsmål, besvart

`Settings` hadde ingen `enabled`-nøkkel, verken på en slot eller på planen, så
«av» kunne bare staves `slots: []` — og bryteren på nivå 1 måtte SLETTE
tidspunktet for å slå seg av. P1a dempet det (økt-hukommelse, et spørsmål med
antallet i) og skrev ned at å fikse det ordentlig er eierens valg.

Eieren sa ja. Feltet er `auto_record_enabled: bool`, og det leses ÉTT sted:

```rust
pub fn active_slots(&self) -> &[ScheduleSlot] {
    if self.auto_record_enabled { &self.slots } else { &[] }
}
```

Scheduleren leser slots seks steder (neste start, vekke-horisonten,
påminnelses-eventene, sen-start-vinduet, tapt-sjekken og status-kommandoen). Et
flagg som ble hedret i fem av seks er en maskin som våkner 10:50 på en søndag
for et opptak den så nekter å gjøre — derfor én funksjon, ikke seks sjekker.

To valg det er verdt å kunne begrunne igjen:

- **`#[serde(default = "default_true")]`.** En profil skrevet før feltet fantes
  har ingen nøkkel å lese, og `false` ville stille avvæpnet hver menighet som
  allerede hadde en søndagstid. En fersk profil har ingen slots uansett.
- **Spesialopptak gates IKKE.** De er datoer noen skrev inn for én konsert.
  Nivå 1-bryteren handler om den UKENTLIGE planen, og en bryter som avlyste
  konserten ville slettet noe den aldri viste.

Ikke i telemetriens `WireSettings`: Workeren krever hvert felt den kjenner, og
et nytt felt der er en skjøt som brekker på serversiden først.

## Forhåndsbufferen er ÉN kontroll nå

Atlaset §2.6 fant tre steder som var uenige om det samme: `prerollEnabled` (som
ingen i Rust leser), `preRollSeconds` (som bakenden porter bufferen på), og
telemetrien, som UTLEDER `preroll_enabled` fra tallet. En profil med «30
sekunder» og bryteren av rapporterte «pre-roll på» og bufret ingenting.

Avansert viser sekundene, og bare dem. Da MÅ sekundene også være det som
avgjør, ellers står det «15 sekunder» på en skjerm der ingenting blir bufret —
så `app/state/preroll.ts` utleder `enabled` fra `seconds > 0`. Standarden er 15
i både Rust og `settings-defaults.ts` (eiervalget «pre-roll på og usynlig»); en
profil som allerede har et tall beholder sitt. `prerollEnabled` er urørt i basen
og fortsatt legacy-skallets bryter — de to skallene kjører aldri samtidig.

## `narrowToStored` — en skjøt som ville avvist HELE lagringen

Et `<select>` leverer alltid en streng. Fem av innstillingene bak en select er
`i32` i Rust, og `Settings` deserialiseres strengt: `"30"` der serde venter et
tall avviser hele `settings_save`, ikke bare det ene feltet — skjermen ville
sagt «Lagret ✓» for en skrivning som aldri landet, og tatt med seg alt annet i
samme byge.

P1a løste det ett kallsted om gangen (`reminder.set(Number(next))`). Nå gjør
`useSetting` det selv, ut fra typen på den LAGREDE verdien: `bitrate` er
strengen `"256"` i Rust og skal bli en streng, så det er ikke «ser det ut som et
tall?» som avgjør. Tabelltestet i `use-setting-core.test.ts`.

## Det P1b IKKE tok med

- **«Oppdater automatisk» og den timesvise sjekken.** ✅ **Tatt i P3.** `autoUpdate`
  er en av de fire uten bakendleser (ATLAS §2.6); timeren bodde i
  `general-page.ts`, og canvasens sett 5.4 lot raden være ute. Konsekvensen var
  reell — **det nye skallet sjekket ikke etter oppdateringer av seg selv**, og
  det er den samme veien beta-ringens kill-switch når folk. Nå: én fil
  (`app/state/auto-update.ts` over `auto-update-schedule-core`) og én rad på
  Avansert, fordi PRIVACY.md lover at den KAN slås av og timeren og bryteren
  derfor hører sammen. Se P3 under.
- **Diagnose-modalen** (`btn-audio-diagnose` → `run_diagnostics`). Ingen plass i
  den nye informasjonsarkitekturen ennå; legacy-specen dekker den fortsatt.
- **Mikser og lydbehandling.** Canvasen tegner raden med en «Åpne»-knapp;
  mikseren bygges i P4, så knappen ville ikke åpnet noe.
- **`silenceThreshold` (dBFS).** Rust leser den, men −50 dBFS er ikke et tall en
  frivillig kan ha en mening om, og feil verdi stopper opptaket midt i
  gudstjenesten. Standarden står.
- **Månedskalenderen, kirkeårets helligdager og vekke-diagnostikken** — se
  toppen av `advanced/ScheduleCard.tsx` for hva som ble en liste i stedet.
- **Flere DAGER per fast tid.** `ScheduleSlot.days` er en liste; her er én rad
  én dag, og en profil som allerede har flere vises med den første og røres ikke.

## Dialogkøen fikk en andre form

`alertDialog(opts)` — én knapp, og en `preformatted`-blokk. «Vis hva som
sendes» er hele telemetri-nyttelasten som JSON, og den bor i den SAMME køen og
den samme verten som bekreftelsene. Grunnen er `inert`: verten er det ene stedet
som slår av resten av appen, og en andre modal-mekanisme ville vært et andre
sted det kunne bli glemt.

## e2e: fire re-pekte, to nye

`e2e/{update-channel,telemetry-preview,system-support,auto-update,onboarding}.spec.ts`
er kopier med **hver test-tittel uendret**, fordi `docs/SMOKE-TEST.md` peker på
dem som `sti::tittel`. To describes fulgte ikke med, og begge står forklart i
fila si: `system-support`s «diagnose» (skjermen finnes ikke) og `auto-update`s
«auto-update toggle» (mekanismen finnes ikke — se over). Legacy-filene er urørte
og grønne.

Nye: `e2e/advanced.spec.ts` (flagget beholder tiden, SMTP åpner gaten,
opptaksradene skriver riktige typer) og `e2e/first-run.spec.ts` (porten,
nødutgangen, den gule raden).

⚠️ **En fikstur som lyver om bakenden.** `ConsentStatus` er
`#[serde(rename_all = "kebab-case")]` i Rust, altså `"never-asked"` — men
`e2e/{onboarding,telemetry-preview}.spec.ts` fikstureres med `"neverAsked"`,
en form ingenting i prod produserer. Bare `promptCopyFor` forgrener seg på
literalen, så konsekvensen er at legacy-specen viser oppstartskortets
GJENTATT-tekst til en fersk installasjon. App-kopiene er rettet, med grunnen
ved siden av; legacy-filene er urørt fordi de skal stå som de er til de
slettes.

---

# P2 — Opptak, jobben appen finnes for

P1 bygde stedet man svarer på spørsmål. P2 bygger stedet man tar opp en
gudstjeneste, og de tre flatene rundt det. Alt ligger i `app/pages/record/`.

|                    | før (`legacy/renderer`)                                         | nå (`app/pages/record`)                           |
| ------------------ | --------------------------------------------------------------- | ------------------------------------------------- |
| Start              | «Start opptak» → `#modal-manual` (kilde, kamera, filnavn) → ny  | ÉN knapp, sperret til en kilde er valgt           |
| Hjem-flaten        | hero, VU, tre infokort, videostripe, «Siste opptak» (21 kt)     | kilde · hørsel · Start · to kort                  |
| Opptaksoverlegget  | `#recording-overlay`, alltid i DOM-en, vist/skjult              | en montering i `#overlays`, bare mens det tas opp |
| Stopp-bekreftelsen | `#modal-confirm-stop`, «Avbryt» primær                          | dialogkøen, «Fortsett å ta opp» primær            |
| Kvitteringen       | `#editor-prompt-toast` — «vil du redigere?», forsvinner         | et kort som blir stående (`record-done`)          |
| Feil               | `#global-error-banner` (overskrives av neste feil av alle slag) | nøklede bannere i `state/banners.ts`              |

## Regelen hele settet hviler på

**Start er sperret til en lydkilde er valgt eksplisitt** — også når valget er
maskinens egen mikrofon. Det er den største adferdsendringen i hele programmet,
og grunnen står i atlaset §3a: en fersk installasjon tar i dag opp på
laptop-mikrofonen uten å si fra, fordi `deviceId: null` betyr «systemets
standardinngang» for opptakeren og «Innebygd mikrofon · Tilkoblet ✓» for
skjermen.

`record-core.ts` er tabellen, og den har tre tilstander:

| `kind`           | når                                  | Start   | kortet                             |
| ---------------- | ------------------------------------ | ------- | ---------------------------------- |
| `no-source`      | `deviceId` tom                       | SPERRET | «Du har ikke valgt hvor lyden …»   |
| `source-missing` | valgt, men ikke i enhetslisten       | tillatt | «Finner ikke {navn}» + nødutgangen |
| `ready`          | valgt og til stede — eller ikke lest | tillatt | «Lyd fra {navn} · kanal N–M»       |

Den fjerde raden finnes ikke med vilje: `devices === null` er «ikke sett etter
ennå» og faller tilbake på `ready`. Regelen «ikke funnet ⇒ borte» ville gjort
hver kaldstart til et gult «Finner ikke Behringer X32» som blir borte igjen
etter 100 ms — samme lærdom som `decisions-core`s `unknown`.

Knappen bærer grunnen sin (`disabledReason`), og det er `aria-disabled`, ikke
`disabled`: et ekte `disabled` tar knappen ut av tabrekkefølgen, og da kan en
tastaturbruker ikke engang komme fram til den for å HØRE hvorfor.
`e2e/record.spec.ts` klikker den med `force` — Playwright REGNER
`aria-disabled` som av, en ekte mus gjør ikke det — og krever at
`__E2E_CALLS__` er tomt etterpå. Fjern `canStart: false` i kjernen, og både
tabellen og det specet blir rødt.

## Bekreftelsen er snudd, og det er ikke en feil

Eiervalget (canvas sett 2): primærknappen er «Fortsett å ta opp». `buildConfirm`
gir BEKREFT-knappen primærplassen og Enter når dialogen ikke er `danger` — så
«fortsett» ER bekreftelsen her, og «stopp» går den veien som ellers heter
avbryt. `confirmAndStop()` (i `stop.ts`, egen fil fordi BÅDE overleggets knapp
og menylinjens «Stopp opptak» skal gjennom det samme spørsmålet) stopper altså
når svaret er `false`.

Alternativet var `danger: true`, som gir avbryt Enter-plassen — men det maler
også stopp-knappen RØD, og rødt betyr én ting i denne appen: at det tas opp. En
rød stoppknapp midt i et rødt overlegg er nøyaktig den fargekollisjonen sett 0
låste bort.

`protectRecording` leses IKKE. Den har null Rust-lesere (ATLAS §2.6), det nye
Avansert viser den ikke, og bekreftelsen er en designbeslutning i sett 2 — ikke
en innstilling. Legacy-skallet har fortsatt bryteren sin.

## ✅ «Du kan lukke vinduet» er nå SANN — Rust-endringen er gjort (P3)

Setningen var usann, og derfor fjernet: `src-tauri/src/lib.rs` hadde ingen
`on_window_event`-håndterer i det hele tatt, så siste vindu lukket ⇒
`RunEvent::ExitRequested` ⇒ `state::<RecorderEngine>().stop()`. En frivillig som
lukket vinduet midt i prekenen mistet resten av gudstjenesten.

P3 la inn nøkkelen:

- `sundayrec_core::window::close_action(RecorderState) -> CloseAction` er
  beslutningen, ren og uttømmende matchet (ingen `_`-arm, så en ny
  `RecorderState` TVINGER et valg i stedet for stille å avslutte midt i en
  gudstjeneste).
- `src-tauri/src/window.rs` er det tynne skallet: `hide()` FØRST, og
  `api.prevent_close()` bare hvis skjulingen faktisk lyktes — motsatt rekkefølge
  kan etterlate et vindu som verken lukkes eller forsvinner.
- `Preparing` / `Recording` / `Reconnecting` → skjul. `Stopping` → skjul også:
  den tilstanden sendes FØR `finalize_pending`, så hele concat + leveranse-
  transkoding + historikkraden skjer inni den, og en 90-minutters gudstjeneste
  bruker minutter der. Å avslutte akkurat der er det ene øyeblikket som fortsatt
  kan ødelegge et ellers ferdig opptak.
- `Idle` / `Stopped` / `Failed` → uendret: lukk betyr avslutt, som før.

Veien tilbake til vinduet: menylinjas «Åpne SundayRec» (fantes fra før,
`TrayAction::OpenWindow`, oversatt til alle sju språkene i
`sundayrec_core::tray`), Dock-ikonet på macOS (`RunEvent::Reopen` med
`has_visible_windows: false`), eller å starte SundayRec på nytt (single-instance
løfter fram den kjørende). Alle tre går gjennom `window::show_main`.

Ett OS-varsel per skjuling forklarer hva som skjedde («SundayRec tar fortsatt
opp — vinduet er skjult, ikke lukket. Hent det tilbake fra menylinja.», og en
egen «lagrer opptaket»-ordlyd under `Stopping`). Det varselet er BEVISST ikke
styrt av `notifyStart`/`notifyStop`: de bryterne demper «det du ba om skjedde»,
mens dette er «det du nettopp gjorde gjorde ikke det du trodde, og noe du ikke
har råd til å miste kjører fortsatt» — samme klasse som feilvarslene, som også
overser bryterne. Flagget står i `NOTICE_PENDING` og re-armeres av
`window::show_main`, så to `CloseRequested` for samme klikk gir ett varsel, mens
skjul → åpne → skjul gir to.

Det finnes fortsatt ingen `ActivationPolicy::Accessory` — Dock-ikonet blir
stående med vilje, så appen ikke _forsvinner_ for den frivillige.

⚠️ Selve setningen er ENNÅ ikke lagt tilbake på skjermen: `ov.hint` står fortsatt
ikke i katalogen, og topp-kommentaren i `app/pages/record/RecordingOverlay.tsx`
påstår fortsatt at den er usann. Det er P4/B sitt bord — P3 er en ren
`src-tauri`/`crates`-endring og rører ikke `app/`. Fra og med denne endringen er
det trygt å legge den inn.

## ✅ RESTANSEN ER LUKKET — Cmd+Q under opptak spør nå én gang til (P3b)

Restansen sa: «en ekte avslutning stopper fortsatt opptaket, og
`RecorderEngine::stop()` er ikke-blokkerende — prosessen venter ikke på at
containeren lukkes». Begge deler er nå håndtert, og underveis dukket det opp en
tredje ting som var verre enn restansen selv.

### Beslutningen er ren, som lukkingen

`sundayrec_core::window::quit_action(RecorderState, prior_refusal_age_ms) ->
QuitAction`, uttømmende matchet (ingen `_`-arm):

| Tilstand                               | Ingen fersk nekt | Nekt < 300 ms siden | Nekt 0,3–10 s siden |
| -------------------------------------- | ---------------- | ------------------- | ------------------- |
| `Preparing`/`Recording`/`Reconnecting` | `Refuse`         | `Refuse`            | `StopThenWait`      |
| `Stopping`                             | `WaitOnly`       | `WaitOnly`          | `WaitOnly`          |
| `Idle`/`Stopped`/`Failed`              | `ExitNow`        | `ExitNow`           | `ExitNow`           |

- **`Refuse`** — første trykk avslutter ikke. Et OS-varsel sier «SundayRec tar
  opp — trykk Avslutt igjen innen 10 sekunder for å stoppe opptaket og
  avslutte», og tidspunktet huskes.
- **`StopThenWait`** — andre trykk innen `QUIT_CONFIRM_WINDOW_MS` (10 000 ms) er
  bekreftelsen: `stop()` kalles, og så VENTER prosessen på at fila lander.
- **⚠️ `QUIT_REPEAT_FLOOR_MS` (300 ms)** er den andre kanten av det samme
  vinduet, og den er ikke pynt: et Cmd+Q som HOLDES nede auto-repeterer, og
  repetisjonene kommer millisekunder fra hverandre — uten et gulv ville «nekt
  første trykk» blitt «nekt, og bekreft med én gang», altså nøyaktig det regelen
  finnes for å hindre, nådd ved å lene seg på en tast. (Samme klasse som at en
  plattform kan levere én menyaktivering to ganger — grunnen til at
  `NOTICE_PENDING` finnes på lukke-siden.) Et trykk under gulvet nekter på nytt
  OG flytter tidsstempelet, så et holdt Cmd+Q skyver bekreftelsen foran seg for
  alltid; varselet fyres ikke på nytt for en repetisjon.
- **`WaitOnly`** — under `Stopping` kreves ingen dobbeltbekreftelse. Den
  frivillige har alt bedt om stoppen; det eneste som mangler er tålmodigheten.
- **`ExitNow`** — uten opptak avslutter Cmd+Q som før, på første trykk.

Ti sekunder er hele designet: langt nok til at man rekker å lese varselet og
handle, kort nok til at neste Cmd+Q — minutter senere, av en helt annen grunn —
blir nektet på nytt i stedet for å avslutte en gudstjeneste på et tastetrykk
ingen husker. Tallet i teksten fylles fra konstanten (`{seconds}`), så regelen og
forklaringen ikke kan gli fra hverandre; alle sju språk har egen ordlyd, og
`the_refusal_wording_is_pinned_to_ten_seconds` sier fra hvis noen retuner vinduet
uten å se på tekstene (polsk «10 sekund» er genitiv flertall — 2–4 s ville krevd
«sekundy»).

Dialogen som ble vurdert og forkastet i forrige runde ble forkastet igjen, og av
samme grunn: `blocking_*`-dialogene skal ikke kalles fra kjøresløyfa, og den
ikke-blokkerende varianten gjør en feilende dialog om til en app som ikke kan
avsluttes. En nekt + et varsel degraderer motsatt vei — verste fall ser den
frivillige ikke varselet og trykker igjen, som er nøyaktig det de ville.

### Ventingen har et tak, og det er utledet

`QuitAction::StopThenWait`/`WaitOnly` spawner én oppgave som poller
`current_state` hvert 250. ms i en `tokio::select!` mot et tak. **Begge armer
ender i `app.exit(0)`** — en app som ikke kan dø er verre enn en finalisering
ingen venter på lenger.

Taket er `RecorderTimeouts::QUIT_WAIT_CAP_MS` = `STOP_ABORT_BACKSTOP_MS + 30 s`
(20,5 min), utledet og ikke valgt: ventingen MÅ overleve supervisorens eget
siste-utvei-avbrudd, ellers gir avslutningen opp mens finalize-kjeden fortsatt
kjører lovlig og den frivillige mister akkurat fila de ventet på. Normalt varer
ventingen sekunder — taket er en sperre, ikke en tidsplan.

**Et nytt Avslutt-trykk mens ventingen pågår avslutter umiddelbart**
(`WAITING`-flagget kortslutter beslutningen). Vente-varselet sier det: «Lagrer
opptaket — SundayRec avslutter når fila er trygg. Ett trykk til avslutter med én
gang.»

### ⚠️ Den skarpe kanten: `app.exit(0)` kommer TILBAKE hit

`RunEvent::ExitRequested` fyres også av vår egen `app.exit(0)` — fra ventingen,
fra menylinja og fra `update::relaunch`. Forskjellen er `code`:

- `code: None` ⇒ mennesket ba om det (siste vindu lukket, vindusbehandlerens
  avslutt),
- `code: Some(_)` ⇒ programmatisk `AppHandle::exit`/`restart`.

Verifisert i kilden, ikke fra hukommelsen: tauri 2.11.5 `src/app.rs` dokumenterer
`RunEvent::ExitRequested`s `code` som «`None` when the exit is requested by user
interaction, `Some` when requested programmatically via `AppHandle::exit` og
`restart`», og `tauri-runtime-wry` 2.11.4 reiser `code: None` fra
siste-vindu-destruert-stien (`lib.rs`, `TaoWindowEvent::Destroyed`) mens
`Message::RequestExit(code)` bærer `Some(code)`. `RESTART_EXIT_CODE` (`i32::MAX`)
ignorerer dessuten `prevent_exit()` helt.

Skallet slipper derfor **hver `code.is_some()` rett gjennom** til dagens
opprydding (`RecorderEngine::stop()` + `VuEngine::stop()`, uendret). Å kjøre
beslutningen på vår egen avslutning ville nektet vår egen avslutning.

### ⚠️⚠️ Det som var verre enn restansen: Cmd+Q nådde aldri `ExitRequested` på macOS

Restansen antok at Cmd+Q gikk gjennom `ExitRequested` og bare manglet en
bekreftelse. Det gjorde den ikke.

Tauri installerer `Menu::default` på macOS når appen ikke setter sin egen meny
(`AppBuilder::build`: `if self.menu.is_none() && self.enable_macos_default_menu`).
Den menyens Avslutt er `PredefinedMenuItem::quit`, og muda kobler en predefinert
Quit til AppKit-selektoren `terminate:`
(`muda/src/platform_impl/macos/mod.rs`, `PredefinedMenuItemType::selector`).
`terminate:` spør delegatens `applicationShouldTerminate:` — som tao **ikke**
implementerer (`tao/src/platform_impl/macos/app_delegate.rs` registrerer
`applicationWillTerminate:` og ingen `ShouldTerminate`) — så svaret er
standardens `NSTerminateNow`, og når `applicationWillTerminate:` når tao er
prosessen alt på vei ut: `AppState::exit()` reiser `LoopDestroyed`, ikke
`ExitRequested`.

Konsekvens: på macOS ga Cmd+Q / app-menyens Avslutt **ingen `ExitRequested` i det
hele tatt**. De kunne ikke forhindres — og de nådde ikke engang håndtereren som
stopper opptaket. Prosessen forsvant midt i gudstjenesten, og opptakets eneste
redning var gjenopprettingsskanningen ved neste oppstart. Enhver
`prevent_exit()`-basert bekreftelse ville vært død kode.

`src-tauri/src/menu.rs` bygger derfor `Menu::default` **punkt for punkt** på nytt
fra tauri 2.11-kilden, med ÉN endring: App-undermenyens Avslutt er et eget
`MenuItem` (`sundayrec:quit`) med samme etikett («Quit SundayRec») og samme
`Cmd+Q`-akselerator, og menyhendelsen går til `window::request_quit`.
Undo/Redo/Cut/Copy/Paste/Select All beholder sine predefinerte selektorer — en
håndskrevet Edit-meny er nøyaktig slik en Tauri-app mister Cmd+C på macOS — og
Window/Help beholder id-ene tauri leter etter (`WINDOW_SUBMENU_ID`,
`HELP_SUBMENU_ID`). Menyen ser identisk ut; bare ledningen er ny.

Modulen er `#[cfg(target_os = "macos")]`: Windows og Linux får ingen standardmeny
fra tauri, så det finnes ingen Cmd+Q å fange, og å legge til en menylinje ville
vært en synlig regresjon. Deres avslutninger kommer som `ExitRequested` (siste
vindu destruert) eller via systemstatusfeltet — begge dekket.

⚠️ **Fortsatt ikke fangbart:** alt som sender `terminate:` utenom menyen — Dock-
ikonets «Avslutt», Force Quit, og utlogging/omstart/avstenging. De er akkurat som
før; gjenopprettingsskanningen er sikkerhetsnettet. Å tette det hullet krever
`applicationShouldTerminate:`, altså en endring i tao/tauri, ikke i appen.

### Tre dører, én beslutning

`window::request_quit` er den ene inngangen, kalt fra:

1. `lib.rs` `RunEvent::ExitRequested` med `code: None`,
2. `tray::emit_action` `TrayAction::Quit` (var `app.exit(0)` rett ut),
3. `menu::handle_event` (macOS Cmd+Q / app-menyen).

`Proceed` betyr «gjør det du ellers ville gjort» (stå til side i `ExitRequested`,
`app.exit(0)` i meny/systemstatusfelt); `Handled` betyr «appen skal leve — jeg
har tatt over».

### 🆕 Den fjerde døra: oppdateringens omstart

Dørene over dekket ikke `update::relaunch`. Den kalte `RecorderEngine::stop()`
og drepte prosessen rett etterpå — altså nøyaktig utfallet fra før vernet, nådd
med ETT klikk på «Start på nytt og installer». `stop()` blokkerer ikke, så
concat, leveransetranskodingen og historikkraden døde med prosessen.

Beslutningen ligger nå i den rene
`sundayrec_core::window::relaunch_plan(RecorderState) -> RelaunchPlan`, med
samme statsdeling som `quit_action`, uten nekt-og-bekreft-dansen:

- **`StopThenWait`** (`Preparing`/`Recording`/`Reconnecting`) — stopp pent, vent
  på fila, start så på nytt.
- **`WaitOnly`** (`Stopping`) — ingenting å stoppe, alt å miste: vent, start så.
- **`Now`** (`Idle`/`Stopped`/`Failed`) — som før, umiddelbart.

⚠️ **Hvorfor ventingen må skje FØR omstarten og ikke kan angres etterpå:**
avslutninger kan tas tilbake i `ExitRequested` (`api.prevent_exit()`), men
omstarter kan ikke. Tauri dokumenterer OG implementerer
`ExitRequestApi::prevent_exit` som en no-op når koden er `RESTART_EXIT_CODE`
(`if self.code != Some(RESTART_EXIT_CODE)`, 2.11.5 `src/app.rs`), og
`AppHandle::restart()` på hovedtråden hopper over hendelsen helt. Å be om
omstarten ER omstarten. Derfor: `relaunch` tar beslutningen, armerer den SAMME
avgrensede ventingen som den bekreftede Cmd+Q (`window::arm_wait_then`, én
venter om gangen, samme `QUIT_WAIT_CAP_MS`-tak), og først når fila er trygg
kalles `update::relaunch_now` — helperen/`app.restart()` som faktisk bytter ut
prosessen. Er en avslutning allerede i vente, står omstarten ned og logger det:
oppdateringen ligger alt på disk, så neste oppstart er den nye versjonen
uansett.

### ⚠️ Rigg-test (GUI-UNVERIFIED)

Ingenting av dette kan verifiseres uten en ekte skrivebordsøkt, og et opptak må
faktisk gå. Når eier tester på rigg:

1. Start et opptak. Lukk vinduet (rødt kryss / Cmd+W). **Forventet:** vinduet
   forsvinner, menylinje-ikonet har fortsatt rød prikk, ett OS-varsel dukker
   opp, og opptaket fortsetter (sjekk filstørrelsen vokse).
2. Menylinja → «Åpne SundayRec». **Forventet:** vinduet kommer tilbake med
   opptaket i gang, tidtakeren har telt videre.
3. macOS: skjul igjen, klikk Dock-ikonet. **Forventet:** vinduet kommer tilbake.
4. Stopp opptaket, vent til historikkraden er der, lukk vinduet.
   **Forventet:** appen avslutter, som før.
5. **Cmd+Q under opptak (P3b).** Start et opptak, trykk Cmd+Q ÉN gang.
   **Forventet:** appen lever, opptaket går videre, og et varsel sier «trykk
   Avslutt igjen innen 10 sekunder». Vent 15 s og trykk Cmd+Q igjen:
   **forventet** nekt på nytt (vinduet var utløpt), ikke avslutning.
   Så: **hold Cmd+Q nede i fem sekunder.** **Forventet:** appen lever fortsatt,
   og det kom ikke ett varsel per repetisjon (gulvet på 300 ms).
6. **Bekreftelsen.** Trykk Cmd+Q to ganger innen 10 s. **Forventet:** varselet
   «Lagrer opptaket», appen blir stående mens fila skrives, og forsvinner først
   når finaliseringen er ferdig. Sjekk etterpå at historikkraden OG
   leveransefila finnes — dette er hele poenget.
7. **Overstyringen.** Gjenta punkt 6, men trykk Cmd+Q en tredje gang mens
   «Lagrer opptaket» står. **Forventet:** appen avslutter umiddelbart.
8. **Menylinja + menyen ellers.** Samme to-trinns-oppførsel fra
   systemstatusfeltets «Avslutt». Og bekreft at macOS-menyen fortsatt er hel:
   Cmd+C / Cmd+V i et tekstfelt, Cmd+W, fullskjerm, og «Om SundayRec».
9. **Uten opptak.** Cmd+Q og «Avslutt» avslutter på første trykk, som før.
10. **🆕 Oppdateringens omstart under opptak** (krever en signert utgivelse å
    oppdatere TIL — se docs/NEEDS-RICHARD.md). Start et opptak, last ned
    oppdateringen, trykk «Start på nytt og installer». **Forventet:** opptaket
    stoppes pent, appen blir stående mens fila skrives (samme venting som punkt
    6), og først når historikkraden OG leveransefila finnes starter appen på nytt
    i den nye versjonen. `<app_data>/update-relaunch.log` skal vise linjene
    «recorder state … → plan StopThenWait», «live capture stopped — waiting for
    the file», «restart armed behind the finalisation wait» og til slutt
    «restarting now» — i den rekkefølgen.

## Måleren under et opptak leser opptaket

`start_recording` stopper VU-strømmen selv, og opptaksmotoren eier enheten.
Overlegget leser derfor motorens EGEN `recording://levels`
(`recording-levels.ts`), akkurat som legacy-overlegget gjør: mikrofonen åpnes
NØYAKTIG én gang.

`VuMeter` fikk to nye innganger for å slippe en andre canvas-implementasjon:

- `source` — en alternativ pakkekilde med samme kontrakt som `acquireVuFeed`
  (abonner, få en avslutter). `mono` i pakken tegner ÉN stolpe; en andre stolpe
  som viser den samme kanalen én gang til er en påstand om en høyrekanal som
  ikke finnes.
- `off` — ingen strøm i det hele tatt. For 2.2: å måle «systemets
  standardinngang» når ingen kilde er valgt ville vært å åpne nøyaktig den
  mikrofonen dette settet finnes for å slutte å ta opp fra uten å spørre.

⚠️ `@lib/audio/vu-feed` avstår fra `start_vu` ved å lese
`window.__isRecording`, og `app/` gjenskaper ikke den globalen. Her er det
derfor MONTERINGEN som er vakten: `RecordPage` gir måleren `off` når et opptak
går eller er i ferd med å starte, og overlegget har sin egen kilde. Ingen måler
i treet, ingen `start_vu`.

## Ett ordforråd for nivå

Canvasen skriver «Alt ser bra ut» / «Lyden er borte!» i akkurat den slissen der
2.1 skriver «Vi hører lyd». Måleren har allerede ordet (`app.vu.*`,
`audio/level-words.ts`), og to setninger om det samme ved siden av hverandre kan
bli uenige — de blir det den dagen tersklene flyttes ett sted. Så måleren
beholder sitt ord også i overlegget. Motorens EGET stillhetsvarsel
(`recording://silence`, som fyrer før auto-stoppen) er noe annet enn «måleren
ser lavt nivå», og får sitt eget banner.

## Én økt, ett sett lyttere

Alt overlegget tegner — klokken, størrelsen, auto-stoppen, gjenkoblingen,
stillheten — kommer fra de samme eventene som `isRecording`, så de bor i den
samme modulen (`state/recording.ts`) med ETT `initRecording()`. To lyttere på
`recording://state` som kan bli uenige om hvilken økt som går er skjøtefeilen
`reference-seam-bugs` handler om, i den ene flaten der den koster en
gudstjeneste.

Start og stopp markeres også LOKALT (`markSessionStarted`, `enterFinalizing`),
akkurat som legacy viser overlegget rett etter `res.ok`: `recording://started`
bærer ingen opts, og i nettleser-nivået kommer det aldri. Et overlegg som venter
på et event som ikke kommer er en app som påstår at ingenting skjer mens motoren
tar opp.

## Skjøten mot statuslinjen, andre halvdel

P1a lukket «skinnen sa Alt er klart mens spørsmål 1 sto gult» ved å la
`soundChosen` bety valgt OG til stede. Der sto det at «TA OPP leser ikke
enhetslisten, så der er svaret det samme som før». Nå gjør den det — den MÅ, for
å kunne si «Finner ikke Behringer X32» — så begge sider av skjøten leser den
samme lista. Konsekvensen er synlig i e2e: et spec som seeder `deviceId` uten å
seede `list_audio_devices` får nå (med rette) «Lyden er ikke koblet til», og
`e2e/shell.spec.ts` har derfor `CHOSEN_FIXTURES`.

## Den stille forhåndssjekken

`scheduler://preflight` fyrer 30 minutter FØR et planlagt opptak — for sent for
den som åpner appen fem minutter før gudstjenesten, og aldri for den som tar opp
manuelt. `state/preflight.ts` kjører derfor den samme sjekken én gang per
oppstart, med `buildHealthFindings` GJENBRUKT fra
`@lib/status/health-findings` (ikke portet: ordlyden og «denied er blokkert,
notDetermined er det ikke» er tabelltestet der). Funnene legges foran
`run_preflight` sine, fordi en tillatelse OS-et nekter slår en nesten full disk.

⚠️ `run_preflight` svarer med `Vec<PreflightFinding>` DIREKTE; det er shimmen
som pakker det i `{ findings }`. En fikstur som pakker det selv gir
`{findings:{findings:[…]}}` og et banner som aldri kommer.

## Det P2 IKKE tok med, og hvorfor

- **«Åpne i Rediger»** på kvitteringen og på «Siste opptak». Redigeringsflaten
  er P4. En knapp til en side som ikke finnes lærer en frivillig at knappene i
  denne appen ikke er til å stole på. «Vis i Finder» står der i stedet, og den
  gjør noe i dag.
- **Brikkene «Redigert» og «Eksportert»** (canvas 2.1). `recordings_list` bærer
  ingen slik status — raden er `id, file_path, device_name, started_at,
duration_ms, byte_size, created_at, note` og ikke noe mer. Et merke som gjettes
  er verre enn ingen merke.
- **«Sist sett i går 19:42»** i «Finner ikke {navn}» (canvas 2.3). Ingenting
  lagrer når en enhet sist ble sett. Setningen ville vært oppdiktet.
- **`askOpenEditor`.** Ingen Rust-leser (ATLAS §2.6), så kvitteringen vises
  uansett hva den står på.
- **«+30 min» og «Avbryt auto-stopp»** i overlegget. Kommandoene finnes
  (`recording_extend_autostop` / `recording_cancel_autostop`), men canvasens 2.4
  har dem ikke, og en nedtelling med to knapper er en beslutning eieren ikke er
  spurt om. Fristen VISES (`app.overlay.autoStop`), lest fra motorens egen
  `scheduled_stop_ms`.
- **Bølgeformen** i overlegget (legacy `RecordingWaveform`). Canvasens 2.4 har
  stolper og en klokke; en rullende bølgeform er en andre canvas med sin egen
  rAF-løkke over et opptak som går.
- **`run-diagnostics` fra menylinjen.** Ruteren armer den mot OPPSETT, og ingen
  skjerm plukker den opp ennå — diagnose-modalen er fortsatt legacy-skallets
  (samme forbehold som P1b skrev ned).

## Menylinjen

`pendingAction` er et signal, ikke et syntetisk klikk. `RecordPage` plukker opp
de tre som hører hjemme der (`start-recording`, `stop-recording`,
`run-preflight`), og `Shell` plukker opp den ene som ikke hører til noen side
(`open-recordings-folder` → `window.api.openFolder`). En handling ingen flate
kjenner blir stående i signalet i stedet for å bli spist.

⚠️ `start-recording` fra menylinjen starter bare når en kilde ER valgt.
Ruteren navigerer til OPPTAK uansett, og kortet der sier hvorfor ingenting
skjedde — å starte på en kilde ingen har valgt fra menylinjen ville vært den
samme løgnen, bare et annet sted.

## To typer i `legacy/renderer/main.ts` som løy

`openFolder` og `revealFile` er annotert `Promise<void>` mens shimmen har svart
`Promise<boolean>` hele tiden (den fanger og returnerer `false`). Rettet til
`boolean`, så «Vis i Finder» kan si fra når fila ikke ble funnet i stedet for
stille ikke å gjøre noe. Type-only; ingen oppførsel er endret.

## e2e

`e2e/{recorder,no-live-surface}.spec.ts` er kopier med **hver test-tittel
uendret**, fordi `docs/SMOKE-TEST.md` peker på dem som `sti::tittel`.
`__E2E_CALLS__`-tellerne er ordrette: sømmen flyttet seg ikke —
`startRecordingNow` betyr fortsatt `plan_recording_opts` og så
`start_recording`, én gang hver. Legacy-filene står urørt og grønne.

«the modal» i den andre tittelen er nå opptakssiden selv: `#modal-manual` er
borte (eiervalg), og det som «blir stående og sier hvorfor» er siden med
knappen på.

Ny: `e2e/record.spec.ts` (de tre kilde-tilstandene inkl. mutasjonsprøven,
overlegget løftet av et emittert `recording://state`, den snudde bekreftelsen,
kvitteringen, og de fire bannerne).

`e2e/events.ts` er verktøyet som gjør det mulig: to veier inn, fordi appen
har to slags abonnement. `__emit(kanal, …)` treffer `window.api.on`-kanalene
(de gamle Electron-navnene shimmen kartlegger), `__emitEvent(navn, …)` treffer
bakendens EGNE eventnavn, som `status/next-recording.ts` abonnerer på direkte.
Begge installeres ved å avlytte TILDELINGEN av `window.api` og
`window.__TAURI_INTERNALS__` — de finnes ikke ennå når et init-skript kjører.
⚠️ `__emit` hopper over api-shimmens `EVENT_ADAPTERS`; send formen handleren
leser.

## Stillheten rydder etter seg selv

Motoren fyrer ingen «stillheten er over»-hendelse, så NIVÅENE er fasiten:
`state/recording.ts` lytter på `recording://levels` og tømmer varselet når
`levelWordFor` ikke lenger sier «vi hører ingenting» — de samme tersklene
måleren bruker. Regelen bor der flagget bor, ikke i overlegget: to skrivere på
ett flagg er den skjøten dette skallet er skrevet for å unngå. Et varsel som
overlever sin egen årsak er et varsel folk lærer seg å overse.

Gjenkoblingen og stillheten er TO bannere. Legacy skrev begge inn i det samme
`#rec-reconnect`-elementet, så den som fyrte sist visket ut den andre — en
enhet som falt ut og kom tilbake stille viste bare én av de to tingene som var
galt med opptaket.

## Bevist i en ekte WKWebView

Samme metode som S1b (skjermbilder er upålitelige under TCC på denne maskinen):
en midlertidig Vite-plugin, aldri innsjekket, serverer en MODUL — `script-src
'self'` forbyr en inline en — som leser DOM-tilstand og POSTer svaret tilbake
til dev-serveren. Kjørt på eierens EGEN profil, uten å røre Start:

```jsonc
{
  "ua": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 …",
  "hasSafariToken": false, // uendret fra S1b — se der
  "heading": "Opptak",
  "sourceCard": true,
  "sourceValue": "MacBook Pro-mikrofon", // eierens FAKTISKE valg, ikke en påstand
  "startPresent": true,
  "startEnabled": true,
  "startLabel": "Start opptak",
  "vuFeed": "live",
  "vuWordText": "Vi hører lyd",
  "vuWordChanged": true, // ordet fulgte rommet gjennom prøven
  "vuCanvasSized": "1716x72", // ResizeObserver + DPR 2
  "statusText": "Alt er klart", // og den er ENIG med kilde-kortet
  "banners": [],
  "overlayPresent": false,
  "overlaysRootIsSibling": true,
  "cameraCard": "FaceTime HD-kamera",
  "cspViolations": [],
  "errors": [],
  "rejections": [],
  "consoleErrors": [],
}
```

To ting proben fant som ingen test ville ha funnet:

1. **`lastRecording: "Lørdag 8. august · 0 min"`.** `rowToEntry` gjør en ukjent
   `duration_ms` til `durationSec: 0`, så 0 er tvetydig — enten et opptak uten
   lyd, eller en rad som aldri fikk en varighet. «0 min» er en påstand vi ikke
   kan stå for. Kortet og kvitteringen behandler nå 0 som UKJENT og sier
   ingenting om varighet i stedet.
2. **«Lytter»-brikka står ikke** på eierens maskin. Den er ærlig:
   `preroll_start` svarer `false` når bakendens egen kopi sier av, og eierens
   profil har `preRollSeconds` på 0 (Avansert viser «Av»). Brikka er derfor
   ikke sett live — den er bevist i Playwright i stedet, med begge svar fra
   `preroll_start`.

---

# P3 — Bibliotek, og den timesvise sjekken P1b lot være

P2 bygde stedet man tar opp. P3 bygger stedet man finner opptaket igjen — og
lukker den ene konsekvensen P1b skrev ned som en eiersak.

|                       | før (`legacy/renderer`)                                     | nå (`app/`)                                              |
| --------------------- | ----------------------------------------------------------- | -------------------------------------------------------- |
| Historikk             | tabell m/ 5 kolonner, 2 sorterbare, 3 filterbrikker (11 kt) | én liste, nyeste først (`pages/library/LibraryPage`)     |
| Radens tittel         | filnavn i kolonne 3, dato i kolonne 1                       | «Søndag 16. august 2026 · 11:00»                         |
| Slett                 | ikon i en rad m/ fire ikoner                                | «Slett», og en toast med «Angre»                         |
| Papirkurven           | en lenke som SKJULER SEG når kurven er tom                  | en inngang som alltid er der (`pages/library/TrashPage`) |
| Notat                 | modal + `recording_update_note`                             | vises på raden, redigeres ikke (eiervalg)                |
| «Oppdater automatisk» | bryter + timer i `general-page.ts`                          | rad på Avansert + `state/auto-update.ts`                 |

## Raden er en ØKT, ikke en fil

Et opptak med kamera skriver TO historikkrader (`{stem}.mp4` og lyd-sidevognen
`{stem}.wav`). `pairRecordings` i `@lib/pages/history-core` folder dem til én
rad på den delte grunnstien, og den avgjørelsen er GJENBRUKT — kommentaren over
den forklarer hvorfor nabolagsheuristikken den erstattet ikke kunne virke under
Tauri-shimmen.

Det gir også den ene brikka som overlevde: **Video**. `historyTotals` er derimot
IKKE gjenbrukt, og det er et funn: den summerer `durationSec` per OPPFØRING, så
en økt med kamera bidrar med sin egen lengde to ganger. Legacys statistikklinje
gjør nettopp det. `totalSeconds` summerer over radene.

## ⚠️ Datoen kom fra feil felt — `startedAt` er lagt til i shimmen

`rowToEntry` satte `timestamp: created_at ?? started_at`. `created_at` stemples
av `insert_recording` når RADEN skrives, altså når gudstjenesten er ferdig. Så
lenge tabellen bare viste en dato spilte det ingen rolle; canvasens 3.1 setter
klokkeslettet i radens tittel, og der er «12:05» ikke en unøyaktighet — det er
feil tid, med hele opptakets lengde.

api-shimmen bærer derfor `startedAt` videre, ADDITIVT (`legacy/types`
`RecordingEntry` fikk et valgfritt felt, `pairRecordings` ble generisk over
radtypen — begge type-only). Ingen legacy-adferd er endret; `rowToEntry` legger
én nøkkel til på et objekt alle andre lesere leser ved navn.

## De tre brikkene som ikke finnes

Canvasens 3.1 har «Eksportert», «Redigert» og «Avbrutt», og «manuelt» som et
dempet tillegg i tittelen. `recordings_list`-raden er `id, file_path,
device_name, started_at, duration_ms, byte_size, created_at, note` og ikke noe
mer, og `rowToEntry` setter `status: "ok"` KONSTANT («recordings_list only holds
completed recordings»). Det finnes altså ingen kilde til noen av dem — verken
til «Avbrutt» eller til skillet manuelt/planlagt. Et merke som gjettes er verre
enn ingen merke, samme regel som P2 skrev ned.

Papirkurv-raden er magrere av samme grunn: `TrashEntry` er `id, originalPath,
trashedPath, name, deletedAt, related, byteSize`. Historikkraden ligger igjen i
basen med både starttid og varighet — det er dét som gjør at en gjenoppretting
gir tilbake notatet — men `getHistory` filtrerer bort alt som ligger i kurven,
på originalstien, og det filteret er det som hindrer at en slettet fil dukker
opp som et opptak som finnes. Så raden sier filnavnet, når den ble slettet, og
hvor lenge det er igjen. Canvasens dato + varighet i 3.3 er ikke med.

## Papirkurven har alltid en inngang

Atlaset §5, funn 9: `refreshTrashButton()` setter `display:none` på
«Papirkurv»-lenken når `trash_list` er tom, og lukker samtidig visningen hvis
den står åpen. En frivillig som slettet noe i går og leter etter det i dag
finner ingen dør hvis sveipen har vært innom i mellomtiden — og tilstanden er
derfor ikke engang fotograferbar i atlaset.

Bunnlinja i Bibliotek har nå tre former: «Papirkurv» (ikke lest ennå — et tall
vi ikke har er ikke null), «Papirkurven er tom», og «Papirkurv (N)». Alle tre
er en knapp som går samme sted. `e2e/library.spec.ts` har mutasjonsprøven.

De 30 dagene er ekte: `AUTO_PURGE_DAYS = 30` i `src-tauri/src/trash/mod.rs`, og
`trash::sweep::spawn` armes fra `setup` — første tikk etter 90 sekunder, så hver 12. time — og sletter det som er eldre sammen med historikkradene.
`TRASH_KEEP_DAYS` i `trash-core` speiler tallet, og skjermen sier det.

## To tellende setninger uten en ny `tn()`

«Slettes om {n} dager» og «Slettes automatisk etter {n} dager» er begge
tellende, og `check-i18n-plurals.mjs` krever hver flertallsgruppe i ALLE sju
språk uten unntak for de fem som er pauset. Så kjernen velger FORMEN
(`dueLine`, `autoDeleteLine`) og hver form har en `tf()`-nøkkel som er riktig
for tallområdet den vises for — samme mønster som `spanOfMinutes` i P2.

Alt papirkurven ellers sier er GJENBRUKTE `trash.*`-nøkler, som finnes i alle
sju språk fra før: «Papirkurven er tom», «Slett for godt», «Slett {n} opptak
for godt?», «i går», «+ {n} tilhørende filer».

## Slett spør ikke — og den ene raden som likevel ikke er angrbar

Et slett flytter fila, sidevognene og videosøsteren til papirkurven, og toasten
tilbyr «Angre» i 9 sekunder (legacys eget vindu — husets `info`-standard er 3,2
sekunder, som er for kort til å rekke å lese at det finnes en vei tilbake).

⚠️ Unntaket er raden hvis fil allerede var borte fra disken. `trash_move` hopper
over det som ikke er der, så det er ingenting å flytte og ingenting å legge
tilbake; historikkraden ryddes bort (`recordings_delete`) slik legacy også gjør
det, og toasten kommer da UTEN «Angre» i stedet for med en knapp som ikke kunne
gjort noe.

De to permanente handlingene — «Slett nå» og «Tøm papirkurven» — er de eneste
med en dialog, og den er `danger`: AVBRYT får Enter, og bekreft er RØD SEKUNDÆR,
aldri en rød primær (canvas sett 7).

## Den timesvise sjekken, og hvorfor den er en sikkerhetssak

`app/state/auto-update.ts` er `applyAutoUpdateSchedule` fra `general-page.ts`
over den samme rene kjernen (`auto-update-schedule-core`): arm = sjekk ÉN gang
nå og så hver `AUTO_UPDATE_INTERVAL_MS` (60 min), og planen rapporterer bare
OVERGANGER, så en re-anvendelse aldri kan stable en andre timer.

Gaten er `autoUpdate`, og raden er tilbake på Avansert fordi PRIVACY.md lover
at den kan slås av: «Slår du den av, tar appen ikke kontakt med serveren —
verken ved oppstart eller den vanlige sjekken hver time.» Den manuelle knappen
er med vilje ugatet — det er PRIVACY.mds eget unntak.

`initAutoUpdate()` står ETTER `await hydrateSettings()` i `main.tsx`, og den
rekkefølgen er løftet: revisjonsfunn #11 var at gaten ble lest FØR den lagrede
blobben landet, så `undefined !== false` kontaktet serveren på hver oppstart
uansett hva eieren hadde valgt.

**Kill-switchen har ingen klient-bryter å respektere.** Den virker ved at
Workerens feed slutter å tilby en versjon; `docs/ROLLBACK.md` regner med at en
kjørende installasjon nås «within the hour» nettopp fordi appen spør omtrent
hver time. Det eneste klienten skylder den, er å spørre — og fram til nå gjorde
ikke det nye skallet det.

**Sjekken laster ikke ned.** `update_check` spør; `update_download_install`
kjøres bare når noen trykker. Radens forklaring sier derfor «Sjekker hver time
om det finnes en nyere versjon. Ingenting lastes ned før du sier ja.»

### Én lytter, ikke to

De sju `update-*`-kanalene abonneres ÉN gang, i butikken, og fasen bor i et
signal. `UpdateRow` leste dem selv i P1b — riktig da den var eneste leser, og
feil nå som banneret er den andre: to lyttere med hver sin tilstand er
skjøtefeilen `reference-seam-bugs` handler om. `update-core.ts` flyttet derfor
fra `pages/setup/advanced/` til `state/`, ved siden av `status-line.ts` og
`disk.ts`.

Fasen overlever også at man forlater Avansert, som er den lille gevinsten: en
nedlasting som fortsetter mens man ser på OPPTAK kan raden gjøre rede for når
man kommer tilbake.

### Banneret, ikke en toast

Tre av de sju fasene reiser et gult banner over den siden man er på —
`available`, `downloading`, `ready`. `checking`, `upToDate` og `failed` gjør
ikke: en frivillig fem minutter før gudstjenesten skal ikke se en gul stripe om
at en sjekk pågår, og et banner per fase er hvordan folk lærer å lukke bannere
uten å lese dem.

Det er `warn` og ikke `bad` (`role="status"`, ikke `role="alert"`): en
oppdatering som venter er ikke noe som er galt. Og aldri en egen toast —
canvasens sett 7 har ÉN toast-form, og «det finnes en oppdatering» er ikke en
kvittering som skal forsvinne av seg selv.

Banneret bor i den DELTE køen (`state/banners.ts`), med nøkkelen `update`, så
«tilgjengelig» → «laster ned 40 %» → «klar» oppdaterer ÉN stripe i stedet for å
stable tre. Skallet rendrer den ene; `RecordPage` filtrerer nå eksplisitt på
sine egne to nøkler i stedet for å ha en else-gren som ville malt en tredje som
et kvalitetsbanner.

## e2e

`e2e/history.spec.ts` er `e2e/history.spec.ts` re-pekt, med hver TITTEL
uendret. Fire av legacys ni fulgte ikke med, og hver av dem er en skjerm som
ikke finnes lenger — sorterbare kolonner, filterbrikkene, den chip-filtrerte
tomtilstanden og notat-modalen. Alle fire står forklart i fila, og legacy-filen
er urørt og grønn.

Én fikstur måtte endre form, og forskjellen er verdt å kjenne: legacys
`trash_move` svarer statisk, fordi den gamle siden splicer raden ut av sin egen
kopi. Det nye skallet LESER LISTA PÅ NYTT etter enhver endring, så fiksturen må
modellere invarianten lesningen hviler på — et flyttet opptak ligger i
papirkurven. Nærmere appen, og den eneste måten «raden forsvant» kan bety det
den skal.

`e2e/library.spec.ts` er ny: tellelinja, datoen fra `startedAt`, «—» for en
ukjent varighet, Video-brikka på en foldet økt, papirkurv-inngangen ved tom kurv
(med mutasjonsprøven), «Legg tilbake», og begge de farlige dialogene med rød
sekundær og AVBRYT på Enter.

`e2e/auto-update.spec.ts` fikk «auto-update toggle»-describen TILBAKE — de
fire titlene byte-identiske, `spyIntervals`/`armedUpdateIntervals`/
`delaySettingsLoad` ordrett fra legacy-specen, fordi det er den samme sømmen:
`update_check`-invoken telt på fikstur-grensen, og timerregisteret i stedet for
å vente en time på at ingenting skjer.

## Bevist i en ekte WKWebView

Samme metode som S1b og P2 (skjermbilder er upålitelige under TCC på denne
maskinen): en midlertidig Vite-plugin, aldri innsjekket, serverer en MODUL —
`script-src 'self'` forbyr en inline en — som leser DOM-tilstand og POSTer
svaret tilbake til dev-serveren. Kjørt på eierens EGEN profil, med 26 ekte
opptak, uten å røre Slett:

```jsonc
{
  "ua": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 …",
  "hasSafariToken": false, // uendret fra S1b — se der
  "heading": "Bibliotek",
  "sub": "Opptak: 26 · 10 min",
  "rowCount": 26,
  "rows[0]": "Lørdag 8. august 2026 · 22:12 · Under 1 min · 2026-08-08.flac",
  "unknownSpans": 0,
  "zeroSpans": 0, // ⚠️ var 5 i første runde — se under
  "searchPresent": true,
  "autoDelete": "Opptak slettes ikke automatisk",
  "trashLinkPresent": true,
  "trashLinkText": "Papirkurven er tom", // funn 9, live
  "updateChecks": 1, // ⬅ den timesvise sjekken FYRTE, én gang, ved oppstart
  "banners": [],
  "trash": {
    "heading": "Papirkurv",
    "lede": "Opptak her slettes for godt etter 30 dager.",
    "empty": true,
    "backPresent": true,
    "railStillLibrary": true,
  },
  "overlaysRootIsSibling": true,
  "cspViolations": [],
  "errors": [],
  "rejections": [],
  "consoleErrors": [],
}
```

To ting proben fant som ingen test ville ha funnet:

1. **⚠️ «0 min» på fem av eierens rader.** `spanOfSeconds` runder til nærmeste
   minutt, og fem testopptak fra Qu-5-runden varte under et halvt minutt. Det er
   den samme setningen P2 fjernet fra «Siste opptak»-kortet — men med motsatt
   årsak: der var 0 UKJENT, her er den KJENT og likevel usann, for opptaket
   varte ikke null sekunder. `rowSpan` har derfor en tredje form, `under`, med
   grensen nøyaktig der `spanOfSeconds` runder, så de to aldri kan bli uenige
   om et opptak på 45 sekunder. Andre runde: `zeroSpans: 0`.

2. **Den timesvise sjekken fyrer for ekte.** `updateChecks: 1` er
   `window.api.checkForUpdates` talt i selve webviewet — altså at
   `initAutoUpdate` armet, at gaten leste eierens lagrede `autoUpdate`, og at
   sjekken gikk én gang og ikke to. Ingen banner, fordi `update_check` i en
   dev-build svarer `upToDate` uten å ta kontakt med noen.

---

# P4a — Rediger, steg 1: «Klipp»

P3 bygde stedet man finner opptaket igjen. P4a bygger det man kom for å gjøre
med det: finne prekenen. Canvasens sett 4, artboard 4.1 — ÉN skjerm med ett
spørsmål og ett klikk som svar.

|                | før (`legacy/renderer/pages/editor*`)                        | nå (`app/editor/`)                                    |
| -------------- | ------------------------------------------------------------ | ----------------------------------------------------- |
| Kontroller     | 47 i tre faner + 25 i eksportmodalen                         | forslagskort, bølgeform, transport, kuttliste         |
| Å finne preken | fane «Klipp-verktøy» → «Marker preken automatisk» (2 klikk)  | kortet står ved åpning, ett klikk                     |
| Justering      | dra kuttgrensene i bølgeformen etter at kuttene er lagt      | to gullhåndtak, FØR og ETTER anvendelsen              |
| Resultat       | «Resultat 28m 10s (fjerner 33m 50s)», skjult til det er kutt | «Resultat: 28 min 10 s (av 1 t 2 min)», alltid        |
| Avspilling     | to knapper: «spill» og «forhåndslytt»                        | én, og den hopper alltid over kuttene                 |
| Angre          | Cmd+Z, og en «Fjern alle kutt» som dukket opp                | «Angre»/«Gjør om» i kuttlista, som åpnes av ett klikk |

## `E` er PORTET, ikke oversatt

Planens arkitekturkontrakt, holdt til punkt og prikke: `app/editor/model.ts`
har det samme muterbare objektet, med de samme feltnavnene og den samme
betydningen som `@lib/pages/editor/state`. Det er ikke nostalgi —
det er den ene hete stien i appen. Tegneløkka leser `cuts`, `peaks`, `vpStart`
og `playStartSec` opptil seksti ganger i sekundet, og et signal per felt ville
betydd seksti sporede lesninger per frame og en re-render av treet hver gang
spillehodet flyttet seg en piksel.

Ved siden av står SPEILENE, ett signal per ting treet viser, og regelen er én
linje lang:

```ts
E.cuts = neste; //  sannheten, for tegneløkka
cuts.value = E.cuts.slice(); // speilet, for treet
```

Legacy har allerede nøyaktig det paret, bare med `drawWaveform()` som andre
halvdel («anvend, så tegn»). Her ER andre halvdel signalet, og tegningen
abonnerer på det.

| ble et signal                                                                                                                                                                                                                                                                                                            | forble muterbart i `E`                                                                                                                                             |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `cuts` · `segments` · `suggestion` · `applied` · `dismissed` · `duration` · `filePath` · `fileName` · `startedAtMs` · `dirty` · `playing` · `playheadSec` · `viewport` · `loadState` · `loadPhase` · `loadProgress` · `loadError` · `playbackSource` · `activeStep` · `manualMode` · `analyzing` · `canUndo` · `canRedo` | `peaks` · `cutHistory`/`cutHistoryIdx` · `vpStart`/`vpEnd` · `playStartSec` · `isPlaying` · `playerEl` · `loadSeq` · `hoverSec` · dragfeltene · `canvas`/`minimap` |

To ting ingen bryter: **ingenting inne i en rAF-løkke leser et signal** (det er
`E` eller `peek()` der — `@preact/signals` sporer lesninger uansett hvor dypt i
kallstakken de skjer), og **ingen skriver et speil uten å ha skrevet `E`
først**.

`playheadSec` er det ene signalet løkka skriver, og den skriver det på husets
tegne-kadens (`@lib/ui/frame-gate`), ikke per frame: «0:21:08» endrer seg ett
sekund om gangen.

## Gjenbrukt · portet · ikke bygget ennå

**Importert uendret gjennom `@lib/pages/editor/*`** — de er allerede
enhetstestet i legacy, og de er stedene en feil ville vært stille:

`cut-ops` (klemming, minstelengde, fletting) · `cut-history` (angre/gjør om som
en ren tilstandsmaskin) · `keep-segments` (hva som blir igjen) ·
`draft-scheduler` (debouncen som ikke kan skrive til feil fil) ·
`sermon-candidates` (den ENE lista både tilbudet og korreksjonen bygges på) ·
`sermon-feedback` (E8-nyttelasten, bygget FØR forfremmelsen) · `haptics` ·
`play-regions` (`routePlayback`).

**Portet til `app/editor/`** — de rører DOM eller lerret, så algoritmen er
legacys og oppkoblingen er ny: `waveform.ts` (stolpene, linjalens tikk-tetthet,
«det som er spilt av er dempet») · `canvas-input.ts` (måle-cachen, snappingen
mot analysens grenser, hjul-zoomens forankring) · `viewport.ts` · `playback.ts`
(de-klikket, `canplay`-stien, generasjonstelleren) · `loader.ts` (fire steg med
`loadSeq` over alle) · `sermon.ts`.

**Skrevet nytt, og node-testet**: `editor-core.ts` — prekenvinduet, veien fra
vindu til kutt, `sermonCutRegions` (ordrett legacys `applySermonTrim`, som selv
speiler Rustens `sermon_cut_regions`) og de tre tidsformene.

### Prekenvinduet er ÉN idé, ikke to

De to gullhåndtakene gjør to forskjellige ting: FØR «Behold bare prekenen»
justerer de FORSLAGET, etterpå justerer de KUTTGRENSENE. Det er én idé — «hvor
begynner og slutter prekenen» — med to representasjoner under seg, og hvis de to
representasjonene får hver sin kode blir de uenige. `sermonWindow()` svarer
derfor alltid med det samme vinduet uansett hvilken side av knappen man står på,
og `windowToCuts()` er den ene veien tilbake (og tar med seg kuttene som lå
INNE i vinduet, så et håndtaksdrag ikke sletter musikk-kuttet anvendelsen la
inn).

## Den utvidede tidslinja er IKKE portet

Legacys `geometry.ts` deler lerretet i tre regioner — intro, hovedopptak,
outro — og lar `playStartSec` gå NEGATIV for å bety «inne i intro-jingelen».
Alt det finnes for jinglene, og jingler er ikke i sett 4 i det hele tatt.

Så `app/editor/geometry.ts` er fire funksjoner: `secToX`, `xToSec`,
`clampToFile` og `grabThreshold`. `clampPlayable` og `clampMain` er den samme
funksjonen når intro og outro er null, og da skal den ha ett navn og ikke to som
en dag rekker å bli uenige.

⚠️ Jinglene er ikke FJERNET — legacy-skallet har dem fortsatt, og det er det som
sendes ut. De er ikke bygget ennå. Det samme gjelder **videobildet**: en
videofil åpnes og klippes som før (tidslinja, kuttene og eksporten er lydens
uansett), men P4a har ingen `<video>`, så den som redigerer en mp4 ser bare
bølgeformen. Begge er eierbeslutninger for P4b.

## Stegstripa har ett steg

Canvasen har tre — 1 Klipp · 2 Lyd · 3 Eksporter — og P4a bygger det første.
De to andre står IKKE i stripa, hverken som knapper eller som dempede
plassholdere: S1bs husregel er at ingenting sier «kommer senere» og at ingen
knapp finnes uten å gjøre noe, og en dempet «2 Lyd» ville vært begge deler på én
gang.

Å droppe stripa helt til alle tre finnes ville skjult FORMEN, og formen er halve
poenget: en frivillig skal se at dette er en vei med et endepunkt. Så stripa
står med ett steg. `Tabs` var allerede bygget for det (`TabItem.done`,
«editorens steg»); P4a la til `TabItem.step` — tegnet i sirkelen, `aria-hidden`,
fordi `role="tab"` allerede sier «1 av 3».

## Tre steder kuttverktøyene åpner seg selv

`manualMode` er den ENE sannheten om hvorvidt kuttlista står. «Klipp manuelt»
slår den på og av; tre andre steder slår den PÅ, og hvert av dem er en setning
som ellers hadde vært usann:

1. **Etter «Behold bare prekenen».** Det man nettopp fjernet skal være synlig,
   og ANGRE skal være innen rekkevidde. Ett klikk skal kunne tas tilbake med ett
   klikk — uten å måtte lete etter «Klipp manuelt» først.
2. **Når analysen ikke fant noen preken.** Da er dette den eneste veien videre,
   og et klikk for å avsløre den eneste veien videre er et klikk for ingenting.
   (Kortet vises ikke da. Ikke et tomt kort, ikke en unnskyldning — ingenting.)
3. **Når et kutt-utkast ble lagt tilbake.** Hele poenget med gjenopprettingen er
   at man ser hva forrige økt rakk. Legacy gjorde det stille, og stille er greit
   — usynlig er det ikke.

Alle tre SKRIVER flagget i stedet for å legge til en betingelse i komponenten.
To skrivere på ett flagg er greit; to REGLER om det samme flagget er skjøten
dette skallet er skrevet for å unngå.

## Avspilling: én knapp, og den hopper over kuttene

Legacy har to — «spill» (hele fila) og «forhåndslytt» (hopp over kutt). En
frivillig som nettopp trykket «Behold bare prekenen» vil høre RESULTATET; å
måtte vite hvilken av to knapper som viser det er nøyaktig valget sett 4 fjerner.

`<audio>`-elementet eies av MODELLEN og ikke av JSX: et element i treet ville
mistet `src`-en sin hver gang noe over det rendret, og avspilling som stopper
uten at noe feiler er den verste formen for feil. Ett element, gjenbrukt for
hver fil (legacys `persistentPlayerEl` — et nytt per fil lekket en dekoder og et
åpent filhåndtak hver gang).

De-klikket er portet ordrett: tone ned over tre frames, hopp over kuttet, tone
opp. `volume` er den eneste knappen et medieelement gir, og det er akkurat det
et de-klikk trenger.

**Tre ærlige tilstander**, og de to siste har legacys egne tekster (og finnes
derfor i alle sju språk): `original` sier ingenting · `proxy` sier at
avspillingen går via en mellomfil · `none` sier at den ikke er tilgjengelig, og
da er avspillingsknappen `aria-disabled` med den samme setningen som grunn.
Atlasets forbehold består: i en ren nettleser er `asset://` død, så `none` er
det man ser der, og det er sant.

## Lastingen sier hvilken fase den er i

«Analyserer …» alene er ikke sant nok. Fire steg, `loadSeq` over alle
(hver `await` sjekker den på nytt), og de to som tar ekte tid har hver sin tekst
OG en ekte bar fra bakendens egne tikk (`editor-peaks-progress`,
`editor-proxy-progress`): `editor.analyzingWaveform` og
`editor.preparingPlayback`.

## Inngangene

- **`window.openEditorWithFile(path, seekToSec?)`** — samme kontrakt, samme
  signatur, samme `declare global` som legacy. `e2e/editor.spec.ts` og
  atlas-scenene åpner editoren gjennom den. Installeres i `main.tsx` ved siden
  av `showPage`, fordi den hører til samme klasse: noe UTENFOR treet hviler på
  at den finnes.
- **`library/edit`** — `TAB_ALIASES` hadde raden fra S1a (`editor` → `library` /
  `edit`); P4a er den andre halvdelen av den.
- **«Rediger» på biblioteksraden** er nå PRIMÆR (canvas 3.1). P3 satte «Vis i
  Finder» der fordi flaten ikke fantes; nå gjør den det, og Finder er sekundær.
  Raden sender med DATOEN sin — editoren kan ikke lese den ut av fila, og den er
  overskriften begge steder.
- **«Åpne i Rediger» på kvitteringen** (`editor.promptOpen`, en gammel nøkkel) —
  P2 utelot den av samme grunn, og den er primær nå.
- **Slippsonen** er ETT element som ALLTID står. Tauri fanger OS-drag selv, og
  api-shimmen sender dem tilbake som syntetiske `dragover`/`drop` mot
  `document.elementFromPoint(…)` med `File.path` satt — en overlay som først
  dukker opp ved `dragenter` finnes ikke å treffe når hendelsen kommer.
  Filtypen sjekkes IKKE: lasteren prøver fila og sier ærlig fra. En fjerde kopi
  av lista over lydformater (api-shimmen har én, legacys editor to) ville vært
  en fjerde ting å drifte fra hverandre.

## i18n

26 nye nøkler under `app.editor.*`, i `PAUSED_KEYS`. Ti setninger er GJENBRUKTE
legacy-nøkler som finnes i alle sju språk fra før, og det er ikke gjerrighet —
det er de samme ordene om de samme tingene: `nav.editor` («Rediger»),
`editor.openFile`, `editor.promptOpen`, `editor.closeFile`,
`editor.sermonPickerLabel`, `editor.cutsTitle`, `editor.cutsNone`,
`editor.deleteCut`, `editor.dragHint`, `editor.dropFile`,
`editor.analyzingWaveform`, `editor.preparingPlayback`, `editor.qualityFallback`,
`editor.playbackViaProxy`, `editor.confirmClose`, `dialog.discardEdits*`,
`trash.undo`, `tooltip.play`, `tooltip.unsavedChanges`, `app.record.last`.

Ingen ny `tn()`. «45 s» / «28 min 10 s» / «1 t 2 min» er tre `tf()`-nøkler, av
samme grunn som `span-text.ts`: `check-i18n-plurals.mjs` krever hver
flertallsgruppe i ALLE sju språk uten unntak for de fem som er pauset, og «s»,
«min» og «t» er invariante forkortelser i hele tallområdet de vises for.

⚠️ **Klokka er `timecode()`, ikke `formatTime()`.** Legacys formatter dropper
timetallet under en time, så en gudstjeneste på 1 t 2 min hopper fra «59:59» til
«1:00:00» og gjør talltegnene ustabile midt i avspillingen. Her er formen den
samme hele veien.

## e2e

`e2e/editor.spec.ts`, 15 tester. Åtte titler er ORDRETT legacys, fordi de
beskriver den samme oppførselen på en ny flate — og `docs/SMOKE-TEST.md` peker
på to av dem som `sti::tittel`.

Tre av legacys titler er ikke med, og det er ikke forglemmelse: «the three tabs
switch, and switching does not redo the work» og «the chosen tab is remembered
across a reopen» beskriver tre faner som ikke finnes (stegstripa har ett steg),
og «the export modal is honest about destination and level» er P4b.

`editorFixtures` bor nå i `e2e/editor-fixtures.ts` og leses av BEGGE
skallenes spec. En andre kopi ville vært en andre ting å drifte fra hverandre:
poenget med fiksturene er at de står for det bakenden legger på ledningen, så de
to skallene må fôres med den samme ledningen. `e2e/editor.spec.ts` er ellers
urørt — hver test-tittel og hver assertion står.

Mutasjonsprøven: slutter «Behold bare prekenen» å anvende forslaget, går TRE
spec-er røde (kuttlista, forskyvnings-regresjonen og ett-klikk-testen).

## Bevist i en ekte WKWebView

Samme grunn som S1b, P2 og P3: Chromium er ikke motoren denne appen sendes ut
i, og SundayEdits E5 målte en 42× regresjon i den ekte WKWebView-en som var
usynlig i Chromium.

⚠️ **Metoden måtte endres, og det er verdt å vite hvorfor.** De tidligere
probene kjørte `npm run tauri dev` mot eierens egen profil. Det går ikke lenger:
eierens installerte SundayRec kjørte, og **enkeltinstans-vakta** (FIKS 1,
`tauri_plugin_single_instance` som FØRSTE plugin i `src-tauri/src/lib.rs`)
blokkerer en andre oppstart — `a second SundayRec launch was blocked — focusing
the existing window`. Å avslutte eierens app for å komme rundt den er ikke en
beslutning en agent tar mens eieren er borte: SundayRec er opptaksappen, og en
avsluttet app er et opptak som ikke starter.

Så proben kjørte i en **ekte WKWebView uten Tauri**: en liten Swift-vert
(aldri innsjekket) laster `npm run dev` og injiserer fikstursømmen som et
`WKUserScript` ved `documentStart` — nøyaktig den samme sømmen `e2e/harness.ts`
bruker. Det gir den ekte motoren, den ekte CSP-en og den ekte tegningen, med et
fikstureid opptak i stedet for eierens. Eierens app, database og
opptaksmappe ble aldri rørt (mappelista er bit for bit uendret før og etter).

```jsonc
{
  "ua": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 …",
  "hasSafariToken": false, // uendret fra S1b — se der
  "editButton": "Rediger", // biblioteksradens PRIMÆRknapp åpner editoren
  "editorState": "ready",
  "heading": "Tirsdag 9. juni 2026", // datoen fra RADEN, ikke fra fila
  "sub": "2026-06-14 Gudstjeneste.mp3 · 10 min",
  "total": "0:10:00",
  "result": "Resultat: 10 min 0 s (av 10 min 0 s)",
  "step": "1Klipp", // stegstripa, med sirkelen
  "suggestion": "Vi tror prekenen er her",
  "suggestionDetail": "Fra 0:03:30 til 0:07:00 — 3 min 30 s. Dra i håndtakene …",
  "keepWindow": true,
  "keepLabel": "Preken · 3 min 30 s",
  "handles": 2,
  "pickerOptions": 3, // «Er ikke dette prekenen?» — tre troverdige blokker
  "canvasSized": "1996x490", // ResizeObserver + DPR 2
  "waveformPixels": 7436, // ⇦ lerretet er TEGNET, ikke tomt
  "drawMsPerFrame": 0.3, // 60 fulle tegninger av et 10-min opptak
  "minimapSized": "1996x66",
  "playbackNotice": "Avspilling er ikke tilgjengelig for denne filen — …",
  "playDisabled": "true",
  "cspViolations": [],
  "errors": [],
  "rejections": [],
  "consoleErrors": [],
}
```

**Tegne-ytelsen er ikke et problem i denne motoren.** 0,3 ms for en full
tegning av et ti minutters opptak på et 1996 px bredt lerret — altså under to
prosent av et 16,7 ms frame-budsjett. Bølgeformen kan tegnes hver frame uten at
det merkes, og gaten i `playback.ts` er derfor et budsjett vi ikke bruker opp,
ikke et plaster.

**To ting proben fant som ingen test ville ha funnet:**

1. **Gullvinduet og forslagskortet sa forskjellige tall.** Etiketten i
   bølgeformen rundet til hele minutter («Preken · 4 min») mens kortet rett over
   var sekundpresist («… — 3 min 30 s»). To tall om det samme, samtidig, på
   samme skjerm. Begge går nå gjennom `spanLabel` i `app/editor/span.ts`.
2. **Undertittelen brukte resultatlinjas form.** «2026-06-14 Gudstjeneste.mp3 ·
   10 min 0 s» — sekunder på et spørsmål som ikke er et sekundspørsmål, og en
   annen setning enn den samme raden i Bibliotek. Den bruker nå
   `spanText(spanOfSeconds(…))`, som er bibliotekets egen.

👤 **Gjenstår for eieren:** den samme runden gjennom `npm run tauri dev` mot et
EKTE opptak — altså med `editor_peaks` og `editor_segments` fra den virkelige
bakenden, og med `asset://`-avspilling som faktisk kan gå. Kommandoen er
`npm run tauri dev`, og den krever at SundayRec ikke allerede kjører.

---

# P4b — Rediger, stegene «Lyd» og «Eksporter»

P4a bygde steg 1 og lot stegstripa stå med ett steg i. P4b bygger de to andre,
og med dem blir eiervalget fra canvasens sett 4 til kode: **mix/master finnes
ÉN gang.**

|                      | før (`legacy/renderer/pages/editor*`)                                | nå (`app/editor/`)                              |
| -------------------- | -------------------------------------------------------------------- | ----------------------------------------------- |
| Mix/master-innganger | **fem** (ATLAS §3c), tre flater, to ulike utfall                     | ÉN: steget «Lyd»                                |
| Valget               | 4 stemmekjeder × 5 mastring-nivåer × 5 kanalmoduser                  | tre ord: Tale · Tale og musikk · Ingen          |
| Resultatfil          | `<navn>_mastert.<ext>` ved siden av originalen, **utenom** eksporten | finnes ikke — behandlingen ligger i eksporten   |
| Eksportvalg          | 10 rader, 23 alternativer, 9 klikk (ATLAS §3d)                       | format + hvor, 2 klikk                          |
| Bitrate/bitdybde     | to velgere i modalen                                                 | følger Oppsett; ingen UI                        |
| Mikserens etiketter  | 23 norske strenger, **uten i18n i det hele tatt**                    | `app.editor.mx.*`, no + en                      |
| Etter eksport        | en linje nederst i en arbeidsflate flere skjermer høy + en toast     | kvitteringskort, samme form som etter et opptak |

## Mappingtabellen — de tre ordene, og tallene under dem

`app/editor/sound-profiles.ts`, ren og node-testet. **Eieren kan overprøve
tallene ved å endre to konstanter.**

| Ordet              | `masterPreset` | LUFS / LRA / TP    | kjeden foran loudnorm                                             | `channelRepair` |
| ------------------ | -------------- | ------------------ | ----------------------------------------------------------------- | --------------- |
| **Tale** (std.)    | `speech-clear` | −16 / 8 / −1 dBTP  | `highpass=80`, −1,5 dB @200, +2 dB @3k, −2 dB @7k, komp −18/2,5:1 | ja              |
| **Tale og musikk** | `music-speech` | −16 / 11 / −1 dBTP | `highpass=50`, komp −22/2:1                                       | ja              |
| **Ingen**          | —              | —                  | —                                                                 | nei             |

Tallene er kjernens (`crates/sundayrec-core/src/mastering.rs`), ikke våre:
profilen velger et preset, den definerer ikke et. `sound-profiles.test.ts`
leser preset-id-ene UT AV Rust-fila (`app/editor/test-support/rust-presets.ts`)
og feiler hvis noen bytter navn på én av sidene — ingen binding dekker det, for
ts-rs eksporterer structen, ikke id-ene.

### Hvorfor IKKE `auto_process`-stemmekjeden + et preset

Det var den nærliggende mappingen, og bakenden advarer mot den med ord:

> It deliberately does NOT recommend a mastering preset. Stacking one on top of
> the vocal chain ran the material through two highpasses, two compressors and
> two EQ curves — the classic over-processed «one-click» result (pumping, thin
> low end).
> — `src-tauri/src/editor/mod.rs`, `auto_process`

Og et mastring-preset **er** en stemmekjede: `speech-clear` er høypass + tre
EQ-bånd + kompressor, og så loudnorm. Én kjede dekker begge halvdelene av
løftet «jevner ut nivået og gjør talen tydeligere» — utgivelsesnivået er
loudnorms, tydeligheten er EQ-ens. `vocalChainPreset` sendes derfor **aldri**
fra dette skallet, og en spec går rød hvis den dukker opp.

Det ene `auto_process` fortsatt brukes til er **kanalreparasjonen**. Den er
ikke en farge: en stille venstrekanal eksporteres halvstum, og det er den
vanligste ekte katastrofen i et menighetsopptak. Legacy hadde en femvalgs
velger for den; her ser analysen det, sier det i én setning (legacys egne
`editor.chan*`, som finnes i alle sju språk) og legger reparasjonen i
nyttelasten. «Ingen» får den ikke — «eksporter lyden slik den ble tatt opp» er
et løfte.

Analysen (`editor_auto_process`, én full `astats`-passering) startes når
lyd-steget åpnes, ikke ved filåpning: der konkurrerer topputtrekket og
segmentanalysen allerede om den samme fila. Den er memoisert per fil, og
**`runExport` venter på den** — en frivillig som går rett fra Klipp til
Eksporter skal få den samme reparasjonen som en som stoppet innom.

## Mikseren ERSTATTER profilen

Alle tjue kontrollene er portet: sju trinn, tretten glidere, tre EQ-bånd og
sluttnivå, med de samme feltene, områdene og trinnene som legacys `mixer.ts`.
Startverdiene **importeres derfra** (`defaultProcessing`) i stedet for å
skrives på nytt — det ene stedet `VocalChain::default()` er speilet i
TypeScript skal fortsette å være ett sted. (Rollup rister `renderMixer`,
`VOCAL_PRESETS` og legacys `E` ut igjen; verifisert i `dist`.)

Regelen som ikke er legacys: er mikseren PÅ, sendes `processing` og **ingen**
`masterPreset`. Legacy lot begge stå i den samme nyttelasten, og det er det
doble høypasset i en annen forkledning. Den som har åpnet bordet har tatt over,
og panelet sier det: «Mikseren erstatter «Tale». Ingen mastring legges oppå.»

## Før/etter-lytting er EKTE — og skriver aldri ved siden av originalen

«Etter» er `editor_master_preview` med **det samme presettet eksporten kommer
til å bruke**, over 20 sekunder fra midten av prekenvinduet. Bakenden rendrer
til `std::env::temp_dir()` — aldri ved siden av opptaket. `_mastert`-veien
(`editor_master_apply`) brukes ikke av dette skallet i det hele tatt.

«Før» er originalen fra det samme sekundet, stoppet av en timer etter tjue
(det finnes ingen «spill fra … til …» på et medieelement). Lyttingen har sitt
EGET `<audio>`: transporten i steg 1 eier `E.playerEl` og skriver `playheadSec`
på tegne-kadensen, og å låne det ville flyttet spillehodet i bølgeformen.

⚠️ **Restanse:** utsnittet blir liggende i OS-ens temp-mappe. Kjernen HAR
predikatet som kjenner dem igjen (`mastering::is_preview_temp_name`,
`sundayrec-master-preview-*.mp3`) men **ingen sveip i `src-tauri` kaller det** —
en gjeld fra mastring-panelet, ikke en P4b innfører. ~800 kB per profil per
opptak. Det ryddes ikke herfra: `editor_cleanup_temp_files` ville sett riktig ut
og gjort noe helt annet (den sletter `.__editor_tmp`/`.__editor_bak` ved siden
av OPPTAKET).

## Det som bevisst IKKE finnes

- **«Normaliser lydnivå»-bryteren.** Den lovet −1 dBFS og ble uansett overstyrt:
  bakenden hopper over `volume=`-filteret så snart et preset er aktivt, fordi
  loudnorm setter nivået. En bryter som bare virker når en annen bryter er av er
  ikke en bryter. `gainDb` sendes derfor aldri.
- **`<navn>_mastert.<ext>`.** To filer i mappen, og den ene er ikke den man
  deler.
- **Nivå-raden i eksporten** («Volum styres av mastring» / «Normalisert
  +3,2 dB»). Den fantes fordi to ting kunne love hver sin ting om volumet; nå
  er det én.
- **Bitrate-, bitdybde- og kodek-velgere.** Bitrate følger `settings.bitrate`
  (det `QualityPage` skriver: 256 for «God»). WAV er 16 bit. Video er H.264 i
  mp4 — ett format, fordi MOV og MKV er valg ingen frivillig har en mening om.
- **Innhold-fanen** (tittel/taler/beskrivelse) — eiervalg, kommer med
  publisering. `metadata` sendes ikke, så FFMETADATA-taggene står tomme.
- **AAC** som eksportformat. Bakenden støtter det; tre kort er et valg, fire er
  en meny.
- **Intro/outro-jinglene.** Ikke bygget i noe steg ennå (P4a sa det samme).

## Filnavnet er bakendens, og canvasens «– preken» ble ikke bygget

`editor::export` skriver `<navn>_redigert.<ext>` og lar `collision_free_path`
legge på `_2`, `_3` … Canvasens `2026-08-23 Gudstjeneste – preken.mp3` ville
krevd en Rust-endring, og den ville dessuten vært en PÅSTAND om innholdet:
`_redigert` er sant også for den som trykket «Behold alt».

Linja over knappen er derfor en **forutsigelse** (`predictedOutputName`), og
kvitteringen viser stien bakenden faktisk svarte med. Den er fasiten; vi kan
ikke vite om det lå en fil med det navnet der fra før.

## Størrelsesanslaget regnes av FILA, ikke av innstillingene

`ca. 27 MB` finnes ikke i legacy — det er nytt her. Formelen er diskanslagets
(`bytes = kbps · 125 · sekunder`, `app/state/disk.ts`), men kilobitene leses av
kilden: WAV av opptakets egen `sampleRate`/`channels` fra `editor_load_recording`,
ikke av opptaksinnstillingens. Et 96 kHz-opptak anslått med innstillingens
48 kHz ville bommet med det dobbelte.

**Ingen anslag for video.** En omkoding av bildet har en bitrate vi ikke kan
regne ut på forhånd, og et tall vi ikke kan regne ut skal vi ikke vise.

## `hasVideo` kostet ingen ny kommando

«Ta med video (MP4)» vises bare når kilden har bilde, og svaret lå allerede i
`editor_load_recording` (`EditorMediaInfo.hasVideo`) — den samme ffprobe-en som
gir varigheten. `editor_probe_streams` ville vært en ny prosess for et svar vi
holdt i hånda. ⚠️ **Bildet vises fortsatt ikke** mens man klipper (P4as
forbehold står): tidslinja, kuttene og eksporten er lydens uansett, men den som
redigerer en mp4 ser bare bølgeformen. Med bryteren PÅ er containeren mp4, og
da står de tre lydformatene dempet MED grunnen i stedet for å forsvinne.

## Stegstripa: fri navigasjon, og haken betyr «du har svart»

Alle tre er klikkbare hele tiden — et opptak man bare vil ha ut i mp3 skal ikke
måtte gjennom to skjermer.

| Steg      | ✓ når                                                                                    |
| --------- | ---------------------------------------------------------------------------------------- |
| Klipp     | spørsmålet er besvart: kutt finnes, eller «Behold bare prekenen» / «Behold alt» er brukt |
| Lyd       | brukeren har VÆRT der                                                                    |
| Eksporter | en eksport har lyktes i denne økta                                                       |

Lyd-haken venter på et besøk og ikke på en verdi, fordi «Tale» er standarden:
en hake fra første sekund ville påstått at noen bestemte seg, og det er nettopp
forskjellen på en standardverdi og et valg. `ed.next` («Neste: Lyd» / «Neste:
Eksporter») står nederst på steg 1 og 2; steg 3 har ingen, for kvitteringen har
sine egne tre veier ut.

Bare steg 1 har bølgeform og transport — canvasens 4.2 og 4.3 har ingen av
delene. Å forlate steget river lerretet ned (`WaveformHost` rydder etter seg) og
**stopper avspillingen**, av samme grunn som å forlate SIDEN gjør det. Toppene
blir liggende i `E`, så en retur tegner opp igjen uten å laste noe.

## e2e

`e2e/editor.spec.ts` er 28 tester nå. **Ni** titler er ordrett legacys —
den nye er «the three tabs switch, and switching does not redo the work», som
ble usann da stripa hadde ett steg og sann igjen nå. Den beskytter det samme
den alltid gjorde: `editor_peaks` og `editor_segments` er kallene som ikke skal
gjentas, og testen sammenligner `__E2E_CALLS__` før og etter en runde gjennom
alle tre.

**To av legacys titler har fortsatt ingen kopi:**

| legacy-tittel                                            | hvorfor ikke                                                                                                                                                                                                                    |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| «the chosen tab is remembered across a reopen»           | oppførselen er BORTE, med vilje: hver åpning begynner på steg 1, fordi «er dette prekenen?» er spørsmålet et opptak åpner med. En kopi ville testet noe vi ikke gjør.                                                           |
| «the export modal is honest about destination and level» | halve tittelen er ikke lenger sann — NIVÅ-raden fantes fordi normaliseringen og mastringen kunne love hver sin ting, og normaliseringen er borte. Destinasjons-halvdelen lever i «eksportsteget er ærlig om hvor filen havner». |

Nye specs med nyttelast-assertions på `editor_export`-argumentene: profilene
(`speech-clear` / `music-speech` / ingenting), mikseren som overstyrer, den
stille kanalen som repareres, video-bryteren, avbrytingen, kvitteringen.
Fiksturene ligger i `e2e/editor-fixtures.ts` (`EXPORT_OK`, `EXPORT_HELD`,
`AUTO_PROCESS`, `AUTO_PROCESS_DEAD_LEFT`) og leses av begge skallene, som før.

**Mutasjonsprøven:** endres `SPEECH_MASTER_PRESET` i `sound-profiles.ts`, går
`sound-profiles.test.ts` (tabellen) og e2e-en ««Tale» er standarden …»
(nyttelasten) røde.

## P-restanser

1. **Preview-temp-sveipen** finnes ikke i `src-tauri` (over). Rust-endring.
2. **Videobildet** vises ikke mens man klipper. Eierbeslutning.
3. **Intro/outro-jingler** er ikke bygget i noen av stegene.
4. **`editor.errInvalidFormat`** sier fortsatt «Velg et annet format i
   eksportvinduet» — vinduet finnes ikke. Uoppnåelig herfra (mp3/flac/wav/mp4
   er alle støttet), men teksten er stale i sju språk.
5. **`export-core.ts`s feilkodetabell SPEILER** legacys `EXPORT_ERROR_CODES`.
   ✅ Delvis lukket i fase B: modulen som holdt `describeExportError` er
   slettet, så tabellen i `export-core.ts` er den ENESTE nå. Det er ikke en
   sammenslåing, det er at den andre halvdelen forsvant — men speilet er borte.
6. **`app.editor.mx.*`** er første store prosamengde i skallet (23 etiketter ×
   2 språk). De fem pausete språkene får dem i oversettelsesrunden, som nå er neste.

---

# Etter byttet

Fase B, PR A gjorde `app/` til skallet og slettet `legacy/renderer/`s skall;
PR B flyttet inventaret som ble igjen til `app/lib/`. Dette er den samlede
restanselista — alt som er kjent åpent, ett sted, så ingen av delene
finnes bare i en commit-melding.

## 1. ⚠️ Vekking fra dvale spør aldri om administratorpassord

Den mest konsekvensrike enden. Scheduleren armer OS-vekkinger selv når
`wakeFromSleep` står på — uprivilegert og ikke-interaktivt
(`scheduler/mod.rs`). Den INTERAKTIVE veien, kommandoen `wake_reschedule`, som
eskalerer en feilet uprivilegert `pmset schedule wake` til ÉN
`osascript … with administrator privileges`, har ingen kallsted i skallet.

Konsekvens: på en Mac som trenger root for å skrive en strømhendelse blir
brukeren aldri spurt, vekkingen blir aldri armet, og maskinen sover gjennom
gudstjenesten. Den tar opp helt fint mens den er VÅKEN.

Hullet er P1bs (`advanced/ScheduleCard.tsx` bygde bryteren uten knappen), ikke
byttets — men byttet er det som sender det ut. Å lukke det er en knapp, to
nøkler og en test, og teksten er et eierspørsmål: canvasens sett 5 har ingen
slik rad.

## 2. ⚠️ `healStoredDeviceId` finnes ikke lenger

Den re-pekte en lagret `deviceId` som ikke lenger fantes, ved å matche på lagret
NAVN. Windows omfordeler enhets-id-er etter omstart eller driveroppdatering, og
L/R-kanalvalget er nøklet PÅ id-en — uten helingen faller en Qu-5-rigg stille
tilbake til kanal 1/2, og ingen oppdager det før opptaket er av feil kilde.

Skallet kalte den aldri, og kunne ikke: den leste `state.ts`' modulspeil av
innstillingene, som skallet ikke fyller. Slettet i fase B sammen med `state.ts`
(et ANDRE settings-speil ved siden av `app/state/settings.ts`). Å bygge den opp
igjen over signalet er lite arbeid; å oppdage at den mangler er ikke.

## 3. Flater som ikke ble bygget, og kommandoene bak dem

35 Rust-kommandoer gikk fra nåbar til unåbar ved byttet. Alle er bevisste, alle
er listet med grunn i baseline-committen og i `docs/SMOKE-TEST.md` under «Flater
som ikke finnes lenger». De som er mer enn opprydding:

- **Diagnose-skjermen.** `run_diagnostics` + `diagnose_audio` + capture-proben
  virker fortsatt og er Rust-testet; ingenting åpner dem. Opptakstallene ligger
  i `<app-data>/last-recording.json`, og §5b i røykboken leses derfra nå.
  Tray-menyens «Diagnostikk» navigerer til OPPSETT og stopper der.
- **Kamerabildet.** `recording_preview_frame` er unåbar. Overlegget har en
  brikke som NAVNGIR kameraet; ingenting viser at det kommer bilder, så et dødt
  kamera oppdages først når fila åpnes.
- **«+30 min» / «Avbryt auto-stopp».** Overlegget sier fortsatt «Stopper av seg
  selv om …», men fristen kan ikke skyves. `manualMaxMinutes` er 0 som standard,
  så det rammer bare en rigg som har slått på sikkerhetsnettet — og da rammer
  det midt i gudstjenesten.
- **Notat-redigering** (`recording_update_note`) — eiervalg, P3.
- **Mastringspanelet** (`editor_master_apply` m.fl.) — erstattet av tre ord og
  mikseren.

Rust-kommandoene er IKKE slettet. Det er en egen opprydding med sin egen
vurdering per kommando.

## 4. Atlas-fotografen er borte

`e2e/atlas/**`, `playwright.atlas.config.ts` og `npm run atlas` fotograferte
legacy-skallet med legacy-selektorer. Etter flippen ville kommandoen startet det
NYE skallet og feilet hver eneste scene, så den er slettet. Bildene består:
`docs/design/atlas/` (183 filer) og `docs/design/ATLAS.md` er Fase A-fasiten.
Å fotografere det nye skallet er en ny scenetabell — verdt å gjøre, ikke en
re-peking.

## 5. Fem språk venter

`ACTIVE_LOCALES` er fortsatt `["no", "en"]`. `PAUSED_KEYS` i
`legacy/locales/parity.test.ts` unnskylder 442 nøkler i sv/da/de/fr/pl. Fase B
ryddet 653 DØDE nøkler ut av alle sju katalogene først, nettopp så
oversettelsesrunden ikke går på tekst ingen ser: sv/da/de/fr/pl gikk fra 895 til
244 nøkler, no/en fra 1339 til 686. PR B er ute; oversettelsesrunden er neste.

## 6. ✅ PR B: inventaret er flyttet

`legacy/renderer/**` → `app/lib/**`, 76 filer, `git mv` og ingenting annet i
sin egen commit — så `git log --follow` bærer historikken og en leser kan se
med øynene at ingen linje er skrevet om. `legacy/` er `bindings` + `locales` +
`types` + `shared`, og `@lib`-aliaset peker på den nye stien i vite, vitest og
tsconfig.

De tre restansene denne flyttingen bar med seg, og hva de ble:

- **`.prettierignore`.** Inventaret formateres **fortsatt ikke** — `legacy`-linja
  er byttet mot `app/lib`. Grunnen er den samme og står i fila: en reformat er
  en helfil-diff på 76 filer, og under flyttingen ville den skjult nettopp det
  flyttingen skal kunne bevises på. Det gjøres **fil for fil ved berøring**.
- **ESLints legacy-blokk.** `app/lib/**` fikk sin **egen navngitte blokk** med
  de samme løsnede reglene, og den strenge `app/**`-blokken utelukker
  `app/lib/**` ved navn (ikke ved regel-overstyring — utelukkelsen er poenget og
  skal være synlig). Samme fil-for-fil-opptrapping. Énveisregelen er beholdt og
  skrevet om: `app/lib/` importerer aldri skallet rundt seg, håndhevet per
  mappedybde fordi det ikke lenger finnes et `app/`-ledd å matche på.
- **`window.__isRecording`** — IKKE lukket, se punkt 7. Den er en
  adferdsendring, og PR B var en flytting.

## 7. ⚠️ `window.__isRecording`: to steder som må være enige

`app/lib/audio/vu-feed.ts` LESER flagget som en vakt mot `start_vu` under et
opptak. **Ingen skriver det** — `app/` gjenskaper med vilje ikke globalen — så
vakten er inert. Det er trygt i dag, og bare fordi skallet vokter det samme ved
**MONTERING**: ingen måler i treet, ingen `start_vu`
(`app/pages/record/RecordPage.tsx`).

To steder som må være enige om én sannhet er nøyaktig formen dette skallet
finnes for å avslutte. Å samle dem er lite arbeid — `vu-feed` tar `isRecording`
som et ARGUMENT i stedet for å lese en global — men det er en adferdsendring med
egen test, ikke en flytting. Alle tre lesestedene peker på hverandre i mellomtiden
(`vu-feed.ts`, `api.d.ts`, `RecordPage.tsx`).
