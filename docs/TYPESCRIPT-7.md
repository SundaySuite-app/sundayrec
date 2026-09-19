# TypeScript 7 — hvorfor `package.json` har to TypeScript-er

_Innført 2026-09-19 i rammeverk-runden for desktop-appene._

TypeScript 7 er den native (Go) kompilatoren. Den eksponerer ikke lenger det
gamle JavaScript-kompilator-API-et verktøyene bygger på, og `typescript-eslint`
(8.70.0, alle kanaler) krever fortsatt `typescript: ">=4.8.4 <6.1.0"` og kaster
på TS ≥ 7 (typescript-eslint#10940). Én pakke som heter `typescript` kan altså
ikke tjene både `tsc` og eslint.

**Beslutning:** side-om-side-oppsettet fra TS 7.0-kunngjøringen — to npm-aliaser
i stedet for én avhengighet (samme som SundayPaper ADR-004):

```json
"@typescript/native": "npm:typescript@~7.0.2",
"typescript":         "npm:@typescript/typescript6@^6.0.2"
```

`node_modules/.bin/tsc` er TS 7 (`typecheck`), mens `require("typescript")` er
TS 6.0-API-et som `typescript-eslint` parser med. Kompat-pakka kaller binæren
sin `tsc6`, så de to kolliderer aldri.

**Konsekvenser:**

- **`"typescript": "npm:@typescript/typescript6@…"`-linja er ikke en
  nedgradering.** Den er parser-API-et for eslint; kompilatoren er
  `@typescript/native`. Ikke «rett» den tilbake til `"typescript": "^7"` — da
  brekker `lint` igjen.
- Typesjekkingen er TS 7, eslint-parsingen er TS 6. Vi linter med
  `tseslint.configs.recommended` (ingen typebevisste regler), så ingen regel
  leser TS 6-semantikk mens `tsc` leser TS 7.
- TS 7 laster ikke lenger alle `@types/*` automatisk (`types` er nå `[]`), så
  tsconfig sier `"types": ["node"]` — testene, e2e og `playwright.config.ts`
  bruker `import.meta.dirname`, `process` og `__dirname`.
- Ta det opp igjen når typescript-eslint støtter TS ≥ 7.1: da smelter
  aliasene sammen til én `"typescript": "^7"`.
