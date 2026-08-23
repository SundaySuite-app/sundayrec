import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import prettier from "eslint-config-prettier";

export default tseslint.config(
  {
    ignores: [
      "dist",
      "target",
      "src-tauri/target",
      "src-tauri/gen",
      // Generated ts-rs bindings (synced by scripts/sync-bindings.mjs) — not
      // hand-written code, so not linted.
      "legacy/bindings/**",
      "coverage",
      "node_modules",
      // Agent worktrees live inside the repo and are gitignored, so CI never
      // sees them — but eslint walks the filesystem, not git, and a checkout
      // of the whole tree inside `.claude/` makes `npm run check` fail locally
      // with hundreds of parser errors while CI is green. A gate that is red
      // only on the developer's machine teaches people to ignore it.
      ".claude/**",
    ],
  },

  // The ported legacy Electron renderer (vanilla TS, browser runtime). It is a
  // faithful verbatim copy of a shipped app, so we run the recommended rules but
  // do NOT bikeshed its style: `any` and unused-vars are downgraded so a 1:1 port
  // never fails the lint gate.
  {
    files: ["legacy/**/*.ts"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: globals.browser,
    },
    rules: {
      "@typescript-eslint/no-explicit-any": "off",
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      // Stylistic rules we don't enforce on the verbatim port.
      "no-empty": "off",
      "no-useless-assignment": "off",
      "prefer-const": "off",
      // The dependency between the two shells is ONE WAY. `app/` may import
      // legacy modules (through `@lib/*`, see below); legacy may never import
      // `app/`. The moment it does, the old shell stops being something that
      // can be deleted whole, and the parallel-shell strategy is over.
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: [
                "../app/*",
                "../app/**",
                "../../app/*",
                "../../app/**",
                "../../../app/*",
                "../../../app/**",
              ],
              message:
                "The legacy renderer must not import from app/. The new shell depends on the old one, never the other way round — otherwise legacy/ can no longer be removed in one piece.",
            },
          ],
        },
      ],
    },
  },

  // ── The new Preact shell (app/) ────────────────────────────────────────────
  //
  // The opposite policy to the legacy block above. legacy/ is a verbatim port of
  // a shipped app and is linted loosely on purpose; `app/` is being written now,
  // by us, for volunteers who have never seen the app — so every rule the port
  // had to be excused from is an ERROR here, and the two i18n mistakes that made
  // the old renderer hard to translate are lint failures rather than review
  // comments.
  {
    files: ["app/**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: globals.browser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    rules: {
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "@tauri-apps/api/core",
              message:
                "app/ never talks to the backend directly — go through `window.api` (installed by @lib/api-shim). One door into Tauri is what makes the fixture seam, the failure ring and the reachability gate mean anything; a second door is invisible to all three. scripts/check-command-reachability.mjs fails on this too.",
            },
          ],
          patterns: [
            {
              group: ["@tauri-apps/api/core/*", "@tauri-apps/api/core*"],
              message:
                "app/ never talks to the backend directly — go through `window.api` (installed by @lib/api-shim).",
            },
            {
              group: [
                "../legacy/**",
                "../../legacy/**",
                "../../../legacy/**",
                "@/*",
                "@/**",
              ],
              message:
                "Reach the legacy renderer through the `@lib/*` alias only. One spelling means the shared surface is greppable, and the day legacy/ moves there is one path to change.",
            },
            {
              group: ["@lib/**/*.css", "@lib/*.css"],
              message:
                "The legacy stylesheet belongs to the legacy DOM — importing it here drags 108 kB of selectors written for an element tree app/ does not have. The new shell gets its own styles.",
            },
          ],
        },
      ],
      // The two i18n mistakes that are cheap to make and expensive to find.
      "no-restricted-syntax": [
        "error",
        {
          // `t("k", "Norsk reservetekst")` — a fallback makes a missing key
          // invisible: the UI reads correctly in Norwegian and silently ships
          // untranslated to the other six languages. In app/ a key that is not
          // in the catalogue must LOOK missing. (S1 replaces this with an exact
          // key-existence gate; until then this is the blunt version.)
          selector: "CallExpression[callee.name='t'][arguments.length>=2]",
          message:
            "No fallback argument in app/: t(key). A fallback hides a missing key behind correct-looking Norwegian.",
        },
        {
          selector: "CallExpression[callee.name='tf'][arguments.length>=3]",
          message: "No fallback argument in app/: tf(key, params).",
        },
        {
          selector: "CallExpression[callee.name='tn'][arguments.length>=4]",
          message: "No fallback argument in app/: tn(key, count, params).",
        },
        {
          // `class`, not `className`. Preact accepts both, which is exactly the
          // problem: two spellings in one codebase means every grep for a class
          // name misses half the hits, and a component copied from a React
          // example brings the other spelling with it. One spelling, enforced.
          selector: "JSXAttribute[name.name='className']",
          message:
            "Bruk `class`, ikke `className`. Preact tar imot begge, og to stavemåter i samme kodebase betyr at et søk etter en klasse alltid bommer på halvparten.",
        },
        {
          // Hardcoded prose in JSX. Deliberately coarse — three letters in a
          // row — so it catches sentences and labels without tripping on «—»,
          // numbers, units or single glyphs. The exact gate lands in S1.
          selector: "JSXText[value=/[A-Za-zÆØÅæøå]{3,}/]",
          message:
            "Hardcoded text in JSX. Every string a volunteer reads comes from the catalogue: {t('some.key')}.",
        },
      ],
    },
  },

  // The Playwright browser tier (E5.2). Node runtime for the spec bodies, but
  // `page.evaluate`/`addInitScript` callbacks run IN the page, so both globals
  // are legitimate here. `any` is downgraded for the same reason the legacy
  // renderer downgrades it: reaching into `window` for a test hook is the point.
  {
    files: ["e2e/**/*.ts"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: { ...globals.node, ...globals.browser },
    },
    rules: {
      "@typescript-eslint/no-explicit-any": "off",
      // The fixture reviver rebuilds function fixtures from source inside the
      // page — `new Function` is the mechanism, not an oversight.
      "no-new-func": "off",
    },
  },

  // Config + tooling files (node runtime).
  {
    files: ["*.{js,ts}", "*.config.{js,ts}", "scripts/**/*.{js,mjs}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: globals.node,
    },
  },

  prettier,
);
