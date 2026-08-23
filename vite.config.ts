import { defineConfig } from "vite";
import path from "path";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// TWO SHELLS, ONE CONFIG — selected by `--mode`, never at runtime.
//
// Default mode (`vite`, `vite build`) is the SHIPPED shell: the ported old
// Electron vanilla-TS renderer under `legacy/renderer/`. Vite's root is that
// directory, so the old `index.html` (which loads `./api-shim.ts` + `./main.ts`
// and `styles.css`) is the entry and every relative import (`../types`,
// `../../types`, `../../shared`, `../locales`) resolves against the mirrored
// `legacy/{types,shared,locales}` tree unchanged. No React/Tailwind — the old
// renderer ships its own styles.css.
//
// `--mode app` is «Frivilligen først»'s NEW Preact shell in `app/`, built beside
// the old one rather than inside it. Different root, different port, different
// outDir, so the two never share a bundle, a dev server or a dist — the legacy
// release flow cannot be affected by anything that happens in `app/`. There is
// no runtime branch anywhere: which shell you get is decided by the command you
// typed.
//
// JSX is the COMPILER's, not a plugin's: `jsx: "react-jsx"` +
// `jsxImportSource: "preact"` in tsconfig.json. `@preact/preset-vite` is
// deliberately NOT used — it is a Babel plugin, and Vite 8 here is rolldown +
// oxc rather than esbuild, so the preset is unverified on this stack. Hence
// `plugins: []` in both modes.
//
// https://vite.dev/config/
export default defineConfig(async ({ mode }) => {
  const isApp = mode === "app";

  return {
    root: isApp ? "app" : "legacy/renderer",
    plugins: [],

    // CSS Modules for the new shell. `camelCaseOnly` means a class written
    // `.setting-row` in CSS is read as `styles.settingRow` in TSX and ONLY
    // that — the un-converted key is not also exposed, so a component can
    // never quietly depend on the kebab spelling and drift from its sibling.
    // Declared in both modes because there is one config: `legacy/renderer`
    // ships a single global `styles.css` and has no `*.module.css`, so this
    // changes nothing for the shipped shell.
    css: {
      modules: { localsConvention: "camelCaseOnly" },
    },

    resolve: {
      alias: {
        "@": path.resolve(__dirname, "./legacy"),
        // The new shell's one way into the old tree: the IPC shim, the locale
        // catalogues and the pure `*-core` modules. Defined in BOTH modes so a
        // module can be shared without caring which shell imported it.
        "@lib": path.resolve(__dirname, "./legacy/renderer"),
      },
    },

    build: {
      // Legacy: Tauri's frontendDist is "../dist" (relative to src-tauri), i.e.
      // repo-root /dist. The app shell gets its OWN directory so a `build:app`
      // can never overwrite the artifact a release is cut from.
      outDir: path.resolve(__dirname, isApp ? "dist-app" : "dist"),
      emptyOutDir: true,
    },

    // Vite options tailored for Tauri development and only applied in `tauri dev`
    // or `tauri build`.
    clearScreen: false,
    server: {
      port: isApp ? 1430 : 1420,
      strictPort: true,
      host: host || false,
      hmr: host
        ? {
            protocol: "ws",
            host,
            port: isApp ? 1431 : 1421,
          }
        : undefined,
      watch: {
        ignored: ["**/src-tauri/**"],
      },
    },
  };
});
