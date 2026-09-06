/**
 * `import styles from "./Button.module.css"` — hva TypeScript skal tro det er.
 *
 * `Record<string, string>` og ikke en generert type per fil: en typegenerator
 * er en byggetrinn til som må kjøre før `tsc`, og som er utdatert i nøyaktig
 * det sekundet noen legger til en klasse. Prisen er at en skrivefeil i et
 * klassenavn gir `undefined` i stedet for en typefeil — og den prisen betales
 * av `scripts/check-app-css-tokens.mjs`' nabo: hver komponent har en
 * røyk-test som rendrer den, og `class="undefined"` er ikke noe man overser
 * to ganger.
 *
 * `localsConvention: "camelCaseOnly"` (vite.config.ts) gjør at `.setting-row`
 * i CSS-en leses som `styles.settingRow` her — ett navnesystem i CSS
 * (kebab-case) og ett i TS (camelCase), uten at noen må velge.
 */
declare module "*.module.css" {
  const classes: Record<string, string>;
  export default classes;
}
