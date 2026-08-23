import { defineConfig } from "vite";
import path from "path";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// ONE SHELL. `app/` — «Frivilligen først»'s Preact shell — is the shipped
// frontend since fase B.
//
// Until then this file carried two roots selected by `--mode app`, because the
// old Electron renderer (`legacy/renderer/`) was still the one every release
// was cut from and the new shell was built BESIDE it. Fase B ended the parallel
// period: `legacy/renderer/index.html`, `main.ts` and every DOM module under it
// are gone, and the only thing left in that directory is the shared inventory
// the new shell reaches through `@lib/*` — the IPC shim, the locale loader and
// the pure `*-core` modules. There is no second root left to select, so there
// is no mode branch: `npm run dev` and `npm run build` are the app shell, and
// `dist/` is what `tauri.conf.json`'s `frontendDist: "../dist"` bundles.
//
// The port is 1420 for the same reason it always was: `tauri.conf.json`'s
// `devUrl` says so, `playwright.config.ts` says so, and the dev-CSP's
// `connect-src ws://localhost:1420` says so. Moving the app shell onto it (from
// the parallel period's 1430) is what let the overlay config
// `src-tauri/tauri.app-shell.conf.json` be deleted rather than promoted.
//
// JSX is the COMPILER's, not a plugin's: `jsx: "react-jsx"` +
// `jsxImportSource: "preact"` in tsconfig.json. `@preact/preset-vite` is
// deliberately NOT used — it is a Babel plugin, and Vite 8 here is rolldown +
// oxc rather than esbuild, so the preset is unverified on this stack. Hence
// `plugins: []`.
//
// https://vite.dev/config/
export default defineConfig({
  root: "app",
  plugins: [],

  // CSS Modules. `camelCaseOnly` means a class written `.setting-row` in CSS is
  // read as `styles.settingRow` in TSX and ONLY that — the un-converted key is
  // not also exposed, so a component can never quietly depend on the kebab
  // spelling and drift from its sibling.
  css: {
    modules: { localsConvention: "camelCaseOnly" },
  },

  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./legacy"),
      // The shell's way into the shared inventory: the IPC shim, the locale
      // catalogues and the pure `*-core` modules. PR B moves those files under
      // `app/lib/`; until then the alias is what keeps every import site
      // spelled once, so that move is a one-line change here.
      "@lib": path.resolve(__dirname, "./legacy/renderer"),
    },
  },

  build: {
    // Tauri's `frontendDist` is "../dist" (relative to src-tauri), i.e.
    // repo-root /dist. This is the artifact a release is cut from.
    outDir: path.resolve(__dirname, "dist"),
    emptyOutDir: true,
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev`
  // or `tauri build`.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
