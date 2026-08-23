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
