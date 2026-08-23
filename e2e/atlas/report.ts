import { spawnSync } from "node:child_process";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { SCENES } from "./scenes";

// The atlas's two written reports — assembled ONCE, after every scene.
//
// Why this is not a `test.afterAll`: Playwright restarts the worker process
// after a failed test, and `afterAll` runs on every worker teardown. With an
// in-memory array that meant INDEX.md was rewritten mid-run from whatever the
// LAST worker happened to hold — a partial index that looked complete. So each
// scene appends one JSON line to a scratch file, and `globalTeardown` (which
// runs exactly once, in the main process, after everything) assembles from that.

const HERE = dirname(fileURLToPath(import.meta.url));
export const ATLAS_DIR = join(HERE, "../../docs/design/atlas");
/** Scratch, not a deliverable: `test-results/` is gitignored. */
const RECORD_PATH = join(HERE, "../../test-results/atlas-report.jsonl");

export interface ConsoleFinding {
  kind: "console.error" | "pageerror";
  text: string;
}

export interface SceneRecord {
  id: string;
  locale: string;
  files: string[];
  findings: ConsoleFinding[];
}

/** Start a fresh run. */
export function resetRecords(): void {
  mkdirSync(dirname(RECORD_PATH), { recursive: true });
  rmSync(RECORD_PATH, { force: true });
}

/** One captured scene. Appended, so a worker restart cannot lose the earlier ones. */
export function appendRecord(rec: SceneRecord): void {
  mkdirSync(dirname(RECORD_PATH), { recursive: true });
  appendFileSync(RECORD_PATH, `${JSON.stringify(rec)}\n`, "utf8");
}

function readRecords(): SceneRecord[] {
  if (!existsSync(RECORD_PATH)) return [];
  return readFileSync(RECORD_PATH, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line) as SceneRecord);
}

/**
 * "No Tauri backend" is the harness telling the truth, not a defect: outside
 * Tauri every unfixtured command rejects, by design (api-shim catches it and
 * renders the empty state). Anything matching this is filed as harness noise;
 * everything else is real console debt this run happened to find.
 */
const HARNESS_NOISE = [
  /no Tauri backend in the browser tier/i,
  /Failed to load resource.*asset:\/\//i,
  /net::ERR_UNKNOWN_URL_SCHEME/i,
  /asset:\/\/localhost/i,
];

function isHarnessNoise(text: string): boolean {
  return HARNESS_NOISE.some((re) => re.test(text));
}

// ── Optional PNG squeeze ─────────────────────────────────────────────────────

/**
 * Shrink the atlas with whatever PNG optimiser the machine happens to have.
 *
 * ~140 kB per 1180×760 shot × ~150 shots is 20 MB of repository — more than
 * this deserves. A lossless `oxipng`/`optipng` pass takes maybe 10 % off; the
 * real win is a 256-colour palette (the UI is flat, dark, few gradients), which
 * roughly halves it with no visible difference at 1×.
 *
 * ENTIRELY optional: none of these tools is a dependency, and a machine with
 * none of them simply gets a bigger atlas. `npm run atlas` must not fail because
 * a PNG optimiser is missing.
 */
function compressPngs(): {
  tool: string;
  before: number;
  after: number;
} | null {
  const has = (bin: string): boolean =>
    spawnSync("command", ["-v", bin], { shell: true, stdio: "ignore" })
      .status === 0;

  const files = listPngs(ATLAS_DIR);
  if (files.length === 0) return null;
  const before = files.reduce((n, f) => n + statSync(f).size, 0);

  let tool: string | null = null;
  let run: ((batch: string[]) => void) | null = null;
  if (has("pngquant")) {
    tool = "pngquant --quality 65-90";
    run = (batch) =>
      void spawnSync(
        "pngquant",
        [
          "--force",
          "--skip-if-larger",
          "--ext",
          ".png",
          "--quality",
          "65-90",
          ...batch,
        ],
        { stdio: "ignore" },
      );
  } else if (has("magick")) {
    tool = "magick mogrify -colors 256";
    run = (batch) =>
      void spawnSync(
        "magick",
        [
          "mogrify",
          "-colors",
          "256",
          "-define",
          "png:compression-level=9",
          ...batch,
        ],
        { stdio: "ignore" },
      );
  } else if (has("oxipng")) {
    tool = "oxipng -o4 (tapsfri)";
    run = (batch) =>
      void spawnSync("oxipng", ["-o4", "-q", ...batch], { stdio: "ignore" });
  } else if (has("optipng")) {
    tool = "optipng -o3 (tapsfri)";
    run = (batch) =>
      void spawnSync("optipng", ["-o3", "-quiet", ...batch], {
        stdio: "ignore",
      });
  }
  if (!tool || !run) return null;

  // Batched so argv stays well under ARG_MAX on any host.
  for (let i = 0; i < files.length; i += 40) run(files.slice(i, i + 40));

  const after = files.reduce((n, f) => {
    try {
      return n + statSync(f).size;
    } catch {
      return n;
    }
  }, 0);
  return { tool, before, after };
}

function listPngs(dir: string): string[] {
  const out: string[] = [];
  const walk = (d: string): void => {
    for (const entry of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, entry.name);
      if (entry.isDirectory()) walk(p);
      else if (entry.name.endsWith(".png")) out.push(p);
    }
  };
  try {
    walk(dir);
  } catch {
    /* nothing was shot */
  }
  return out;
}

// ── The reports ──────────────────────────────────────────────────────────────

const mb = (n: number): string => `${(n / 1_048_576).toFixed(1)} MB`;

/** Called once by globalTeardown. */
export function writeReports(): void {
  const records = readRecords();
  const squeezed = compressPngs();
  writeIndex(records, squeezed);
  writeConsoleFindings(records);
  rmSync(RECORD_PATH, { force: true });
}

function writeIndex(
  records: SceneRecord[],
  squeezed: { tool: string; before: number; after: number } | null,
): void {
  const byScene = new Map<string, SceneRecord[]>();
  for (const r of records) {
    const list = byScene.get(r.id) ?? [];
    list.push(r);
    byScene.set(r.id, list);
  }

  const rows = SCENES.map((s) => {
    const shots = byScene.get(s.id) ?? [];
    const plain = (locale: string): string => {
      const hit = shots
        .flatMap((r) => (r.locale === locale ? r.files : []))
        .find((f) => !/--(full|960x640)\.png$/.test(f));
      return hit ? `\`${hit}\`` : "—";
    };
    const extras = [
      ...new Set(
        shots
          .flatMap((r) => r.files)
          .filter((f) => /--(full|960x640)\.png$/.test(f)),
      ),
    ]
      .map((f) => `\`${f}\``)
      .join(" · ");
    return `| \`${s.id}\` | ${s.page} | ${s.state} | \`${s.recipe}\` | ${plain("no")} | ${plain("en")} | ${extras || "—"} |`;
  });

  const files = listPngs(ATLAS_DIR);
  const bytes = files.reduce((n, f) => n + statSync(f).size, 0);
  const sizeLine =
    `**Total størrelse:** ${mb(bytes)} i ${files.length} PNG-er.` +
    (squeezed
      ? ` Komprimert med \`${squeezed.tool}\`: ${mb(squeezed.before)} → ${mb(squeezed.after)}.`
      : " Ingen PNG-komprimator i PATH (pngquant / magick / oxipng / optipng) — bildene er slik Playwright skrev dem.");

  const missing = SCENES.filter((s) => !byScene.has(s.id)).map((s) => s.id);

  const md = `# Atlas — SundayRec slik appen ER (Fase A)

Hvert bilde er ett skjermbilde av appen som den står på \`main\` etter Fase R
(PR #139 + #141). Ingenting her er et forslag; dette er dokumentasjon av
nåtilstanden, og inndata til Fase D (redesign). IA-uttrekket ligger i
[../ATLAS.md](../ATLAS.md).

**Vindu:** 1180×760 (Tauri-vinduets standardstørrelse). Utvalgte hovedsider er i
tillegg fotografert på 960×640 — appens minimumsvindu — med suffikset
\`--960x640\`. Sider som ruller forbi vindushøyden har et \`--full\`-skudd som
viser hele siden (vindushøyden settes til innholdets høyde; Playwrights egen
\`fullPage\` gir ingenting her, fordi appen ruller inne i \`#main\`, ikke i
dokumentet).

**Språk:** \`no/\` og \`en/\`. De fem andre språkene i språkvelgeren er satt på
pause og er ikke fotografert.

${sizeLine}

## Kjøre på nytt

\`\`\`bash
npm run atlas                 # hele atlaset (starter Vite selv, port 1420)
npm run atlas -- -g "editor"  # bare scener som matcher
\`\`\`

Atlaset er bevisst utenfor \`npm run check\` og utenfor CI: det er et
fotoapparat, ikke en port. \`playwright.config.ts\` ignorerer \`e2e/atlas/\`.

## Scener

| Scene-id | Side | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- | --- |
${rows.join("\n")}
${missing.length ? `\n**Ikke fotografert i denne kjøringen:** ${missing.map((m) => `\`${m}\``).join(", ")}.\n` : ""}
## Hvordan scenene lages

Scenetabellen ligger i \`e2e/atlas/scenes.ts\`. Hver scene er
\`{ fixtures, settings, goto }\` gjennom \`e2e/harness.ts\` (api-shim-sømmen),
pluss eventuelle klikk. \`e2e/atlas/harness.ts\` legger til to ting den vanlige
nettleser-tieren ikke har:

1. **Backend-hendelser.** Halvparten av tilstandene males av en Tauri-event, ikke
   av et kall: opptaksmåleren (\`recording://levels\`), tapt-opptak-kortet
   (\`scheduler://missed\`), forhåndssjekken (\`scheduler://preflight\`),
   gjenkoblingsbanneret, den globale feilstripa. Broen husker hvilken
   callback-id som abonnerte på hvilket eventnavn, og
   \`window.__ATLAS_EMIT__(event, payload)\` fyrer dem av. Fyrer den mot et
   eventnavn ingen lytter på, feiler scenen — den fotograferer ikke feil skjerm.
2. **En fast klokke.** \`page.clock.setFixedTime\` låser \`Date.now()\` til
   søndag 23. august 2026 kl. 10:55, slik at «om 3 dager», «for 2 timer siden»
   og opptakstelleren er de samme i to kjøringer.
`;
  mkdirSync(ATLAS_DIR, { recursive: true });
  writeFileSync(join(ATLAS_DIR, "INDEX.md"), md, "utf8");
}

function writeConsoleFindings(records: SceneRecord[]): void {
  const real: string[] = [];
  const noise = new Map<string, number>();

  for (const r of records) {
    const seen = new Set<string>();
    for (const f of r.findings) {
      if (isHarnessNoise(f.text)) {
        const key = f.text.slice(0, 140);
        noise.set(key, (noise.get(key) ?? 0) + 1);
        continue;
      }
      const key = `${f.kind}|${f.text}`;
      if (seen.has(key)) continue;
      seen.add(key);
      real.push(
        `| \`${r.id}\` | ${r.locale} | ${f.kind} | ${f.text.replace(/\|/g, "\\|").replace(/\n/g, " ").slice(0, 400)} |`,
      );
    }
  }

  const noiseRows = [...noise.entries()]
    .sort((a, b) => b[1] - a[1])
    .map(
      ([text, n]) =>
        `| ${n} | ${text.replace(/\|/g, "\\|").replace(/\n/g, " ")} |`,
    );
  const noiseTotal = [...noise.values()].reduce((a, b) => a + b, 0);

  const md = `# Konsollfunn under fotograferingen (Fase A)

En streng ny vakt over en gammel app. Hver scene i atlaset kjøres med en lytter
på \`console.error\` og \`pageerror\`; alt som falt ut står her.

**Ingenting er fikset.** Dette er en observasjon, ikke en oppgave — Fase A rører
ikke appen.

## Slik er funnene klassifisert

Atlaset kjører **uten backend**. \`api-shim\` fanger hvert avviste \`invoke\` og
returnerer kallerens fallback, så «ingen backend» er den sanne tilstanden, ikke
en feil. Meldinger som matcher \`no Tauri backend in the browser tier\` (og
\`asset://\`-URL-er som aldri kan lastes i en nettleser) er derfor ført opp som
**harness-støy**. Alt annet er ekte konsollgjeld appen bærer i dag.

Grensetilfeller er ført opp som **ekte funn**, ikke som støy: en melding som
ikke kan klassifiseres med sikkerhet skal være synlig.

Merk også at appens egne \`console.warn\` for hvert fallback IKKE fanges her —
vakten ser bare \`console.error\` og ufangede unntak. Warn-strømmen er stor og
forventet uten backend.

## Ekte funn

Les hver rad sammen med scenens oppskrift i [INDEX.md](INDEX.md): noen scener
**fyrer en feil med vilje** (\`home--backend-feil\`, \`home--kvalitetsalarm\`,
\`editor--feil\`, \`toast--lagring-feilet\`). En \`console.error\` derfra er
riktig oppførsel, ikke gjeld.

${
  real.length
    ? `| Scene | Språk | Type | Melding |\n| --- | --- | --- | --- |\n${real.join("\n")}`
    : "_Ingen. Ingen scene ga `console.error` eller `pageerror` utover harness-støyen._"
}

## Harness-støy (forventet, ikke gjeld)

${noiseRows.length ? `| Antall | Melding |\n| --- | --- |\n${noiseRows.join("\n")}` : "_Ingen._"}

---

Scener fotografert: ${records.length}. Ekte funn: ${real.length}. Støy-treff: ${noiseTotal}.
`;
  mkdirSync(ATLAS_DIR, { recursive: true });
  writeFileSync(join(ATLAS_DIR, "CONSOLE-FINDINGS.md"), md, "utf8");
}
