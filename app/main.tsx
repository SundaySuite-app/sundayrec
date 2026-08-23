// The new shell's entry point — deliberately the smallest thing that proves the
// three claims S0 exists to test, and nothing more.
//
//   1. `@lib/*` reaches the legacy renderer, so the new tree can reuse the IPC
//      layer and the seven locale catalogues instead of forking them. Both the
//      shim import below and `App`'s `t()` come from `legacy/renderer/`.
//   2. The shim boots as a side effect: importing it installs `window.api`
//      exactly as `legacy/renderer/index.html` does with its module script.
//      This file exposes NOTHING else on `window`.
//   3. Preact compiles through tsconfig's `jsx: react-jsx` +
//      `jsxImportSource: preact` alone — no `@preact/preset-vite`. That preset
//      is a Babel plugin, and Vite 8 here is rolldown + oxc, not esbuild; the
//      preset is unverified on that stack, so the JSX transform is the
//      compiler's, not a plugin's. `npm run build:app` passing IS that proof.

import "@lib/api-shim";

import { render } from "preact";

import { App } from "./App";

const host = document.getElementById("app");
if (!host) {
  // A white screen with a console error is the failure mode this spike is
  // meant to catch loudly rather than silently.
  throw new Error('app/index.html is missing its <div id="app"> mount point');
}
render(<App />, host);
