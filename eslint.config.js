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

  // What is left under `legacy/` after PR B carried the inventory to
  // `app/lib/`: the seven locale catalogues and their parity test, `types/`
  // and `shared/`. Still the old app's house style, still linted loosely for
  // the same reason — it is a verbatim port, not code written to this repo's
  // rules.
  //
  // The block is NOT merged into the `app/lib/**` one below even though the
  // rules are the same set: they are the same rules for two different reasons
  // (a port that moved, and data/types that stayed), and the day one of the two
  // is finally tightened the other must not silently come along.
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
                "legacy/ must not import from app/. The shell depends on what is left here, never the other way round — otherwise legacy/ can no longer be removed in one piece.",
            },
          ],
        },
      ],
    },
  },

  // ── The Preact shell (app/, including the ported inventory) ────────────────
  //
  // The opposite policy to the `legacy/**` block above. The port is a verbatim
  // copy of a shipped app and used to be linted loosely on purpose; the shell
  // is written now, by us, for volunteers who have never seen the app — so
  // every rule the port was once excused from is an ERROR here, and the two
  // i18n mistakes that made the old renderer hard to translate are lint
  // failures rather than review comments.
  //
  // `app/lib/**` (the inventory PR B moved inside `app/`) used to be EXCLUDED
  // here by name, with its own loosened block further down — the V1 app/lib
  // sweep deleted both: an actual `eslint app/lib` run under these exact rules
  // found only 3 real `any`s (fixed in the same PR) and no prefer-const/
  // no-empty/no-unused-vars hits at all. Two narrow, named exceptions below
  // (search "Tauri doors" and "fallback surface") carve out what genuinely
  // can't tighten yet without doing other rounds' work.
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
                "Reach `legacy/` through the `@legacy/*` alias only (the ts-rs bindings, the locale catalogues, `types`/`shared`). One spelling means the dependency is greppable, and the day something moves there is one path to change.",
            },
            {
              // The form PR B had to repair in 24 files: reaching OUT of one
              // alias with `..` to land in a directory that only happened to be
              // its neighbour. It resolves, it typechecks, and it silently
              // depends on where `@lib` points rather than on what is being
              // imported — so the day `@lib` moved, all 24 broke at once.
              group: ["@lib/../*", "@lib/../**"],
              message:
                "Don't walk out of `@lib` with `..` — say what you mean: `@legacy/bindings/X`, `@legacy/types`, `@legacy/locales/no.json`. An alias reached relative to another alias breaks the day either one moves.",
            },
            {
              // `app/lib` is the ported inventory. Reaching it by relative path
              // is the same surface under a second spelling — and a grep for
              // "what does the shell take from the port" then misses half.
              group: [
                "./lib/*",
                "./lib/**",
                "../lib/*",
                "../lib/**",
                "../../lib/*",
                "../../lib/**",
                "../../../lib/*",
                "../../../lib/**",
              ],
              message:
                "Reach the ported inventory through the `@lib/*` alias only — never by relative path into `app/lib/`.",
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

  // ── The one-way rule: `app/lib/` never imports the shell ───────────────────
  //
  // The inventory is what the shell BUILDS ON. The moment a `*-core` module
  // reaches back into `app/pages/…` or `app/state/…`, the dependency is a
  // cycle: the pure modules stop being testable without the shell, and the
  // per-file tightening described above stops being possible one file at a
  // time. Before PR B this was a single glob (`../app/**`, from outside);
  // inside `app/` there is no `app/` segment left to match on, so it is
  // expressed as "no relative import may escape `app/lib/`".
  //
  // Which prefix escapes depends on how deep the file sits, and the SAME
  // prefix is legitimate one level down (`../audio/smoothing` from
  // `app/lib/ui/` is inventory-internal; from `app/lib/` it would be
  // `app/audio/`). So the rule is written per depth — 1, 2 and 3 are every
  // depth the inventory has (`app/lib/pages/editor/` is the deepest).
  //
  // `[A-Za-z]` is what keeps `../../legacy/bindings/X` legal at depth 1: a
  // segment starting with a letter is a sibling INSIDE `app/`, while `..` is
  // the way out of `app/` altogether, which is where the bindings live.
  ...[1, 2, 3].map((depth) => {
    const up = "../".repeat(depth);
    return {
      files: [`app/lib/${"*/".repeat(depth - 1)}*.{ts,tsx}`],
      rules: {
        "no-restricted-imports": [
          "error",
          {
            patterns: [
              {
                group: [`${up}[A-Za-z]*`, `${up}[A-Za-z]*/**`],
                message:
                  "The ported inventory (app/lib/) must not import from the shell around it. The shell depends on the inventory, never the other way round — otherwise the pure modules stop being pure and the file-by-file tightening stops being possible. Reach `legacy/` with one more `../` (it is outside `app/`), and take a value the shell owns as an ARGUMENT instead.",
              },
            ],
          },
        ],
      },
    };
  }),

  // ── Tauri doors ──────────────────────────────────────────────────────────
  //
  // The strict block above bans `@tauri-apps/api/core` (and its `core*`
  // siblings) everywhere under `app/`, on the theory that `window.api`
  // (installed by api-shim.ts) is the ONE door into Tauri. These three files
  // ARE that door — api-shim.ts installs it, tray-actions.ts is the one other
  // accepted direct listener (documented at its own `@tauri-apps/api/event`
  // import), and api-shim-listen.test.ts exercises the `listen()` path the
  // shim depends on. Loosening the rule here is not a leak in the seam; it is
  // the seam.
  //
  // Only `@tauri-apps/*` is loosened: `no-restricted-imports` is REDEFINED
  // rather than turned off, so the one-way-dependency ban two blocks up
  // (`app/lib/` must not import the shell — these files sit at depth 1) keeps
  // applying. `any`, `no-unused-vars` and everything else in the strict block
  // are untouched for these files too.
  {
    files: [
      "app/lib/api-shim.ts",
      "app/lib/tray-actions.ts",
      "app/lib/api-shim-listen.test.ts",
    ],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["../[A-Za-z]*", "../[A-Za-z]*/**"],
              message:
                "The ported inventory (app/lib/) must not import from the shell around it. The shell depends on the inventory, never the other way round — otherwise the pure modules stop being pure and the file-by-file tightening stops being possible. Reach `legacy/` with one more `../` (it is outside `app/`), and take a value the shell owns as an ARGUMENT instead.",
            },
          ],
        },
      ],
    },
  },

  // ── The i18n fallback surface ────────────────────────────────────────────
  //
  // `no-restricted-syntax` bans a fallback argument on `t`/`tf`/`tn` — a
  // fallback hides a missing catalogue key behind correct-looking Norwegian.
  // The port's OWN `t`/`tf`/`tn` (declared in i18n.ts) take that fallback as
  // a real, load-bearing parameter: it is the Rust-sourced Norwegian prose
  // these keys fall back to when a translation is missing, not a mistake
  // waiting to be deleted. An actual `eslint app/lib` run under the strict
  // block found 20 such calls across these 6 files — stripping the fallback
  // argument is a translation-content decision (what replaces it, and
  // whether the ~35–40 new keys it would require are ready), which belongs to
  // the language round, not this formatting/lint sweep. See the V1 plan.
  //
  // Only the three fallback selectors are dropped; `className`/hardcoded-JSX
  // are re-declared rather than silently lost (they never match in a `.ts`
  // file with no JSX, but the rule value shouldn't quietly say less than it
  // means).
  {
    files: [
      "app/lib/i18n.ts",
      "app/lib/i18n.test.ts",
      "app/lib/status/health-findings.ts",
      "app/lib/status/next-recording-core.ts",
      "app/lib/status/next-recording-core.test.ts",
      "app/lib/ui/progress-core.ts",
    ],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector: "JSXAttribute[name.name='className']",
          message:
            "Bruk `class`, ikke `className`. Preact tar imot begge, og to stavemåter i samme kodebase betyr at et søk etter en klasse alltid bommer på halvparten.",
        },
        {
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
