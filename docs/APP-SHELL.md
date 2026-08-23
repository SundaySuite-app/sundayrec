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
