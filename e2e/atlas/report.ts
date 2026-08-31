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
//
// The trap is worth stating plainly because it is invisible: the partial index
// is not empty and not malformed. It is a shorter table that reads as the whole
// truth.

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
 * everything else is real console debt and goes in the table above it.
 *
 * The list is deliberately SHORT and specific. A wide pattern here would be the
 * quietest way to lose a real finding — the whole point of the guard is that
 * something unclassifiable stays visible.
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
 * ~140 kB per 1180×760 shot × a couple of hundred shots is 20 MB of repository
 * — more than this deserves. A lossless `oxipng`/`optipng` pass takes maybe
 * 10 % off; the real win is a 256-colour palette (the UI is flat, dark, few
 * gradients), which roughly halves it with no visible difference at 1×. That is
 * the same grip fase A used, so the two atlases are comparable byte for byte.
 *
 * ENTIRELY optional: none of these tools is a dependency, and a machine with
 * none of them simply gets a bigger atlas. `npm run atlas` must not fail because
 * a PNG optimiser is missing.
 *
 * ⚠️ Every branch below must be DETERMINISTIC — the atlas's own gate is that two
 * runs produce byte-identical files, and an optimiser that stamps a timestamp
 * into the PNG would break that without changing a single pixel. Hence
 * `png:exclude-chunk=date,time` and the stripped `date:*` properties on the
 * ImageMagick path.
 */
function compressPngs(): {
  tool: string;
  before: number;
  after: number;
} | null {
  // An escape hatch for exactly one question: «is the atlas unstable, or is the
  // OPTIMISER unstable?» `SUNDAYREC_ATLAS_RAW=1 npm run atlas` writes the
  // screenshots as Playwright made them, and the answer falls out of two runs.
  if (process.env.SUNDAYREC_ATLAS_RAW) return null;

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
          "-strip",
          "-define",
          "png:exclude-chunk=date,time",
          "-define",
          "png:compression-level=9",
          ...batch,
        ],
        { stdio: "ignore" },
      );
  } else if (has("oxipng")) {
    tool = "oxipng -o4 (tapsfri)";
    run = (batch) =>
      void spawnSync("oxipng", ["-o4", "-q", "--strip", "safe", ...batch], {
        stdio: "ignore",
      });
  } else if (has("optipng")) {
    tool = "optipng -o3 (tapsfri)";
    run = (batch) =>
      void spawnSync("optipng", ["-o3", "-quiet", "-strip", "all", ...batch], {
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
    for (const entry of readdirSync(d, { withFileTypes: true }).sort((a, b) =>
      a.name.localeCompare(b.name),
    )) {
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

/**
 * Delete photographs no scene wrote — but ONLY after a complete run.
 *
 * Overwriting in place is the quiet failure mode: rename a scene and its old
 * PNG stays behind, already tracked, so `git status` says nothing and the
 * folder keeps a picture that no row in INDEX.md points at. Pruning fixes that,
 * and the condition is exact rather than guessed: every scene in the table has
 * a record, so this WAS the whole atlas and anything else in the folder is
 * stale. After `npm run atlas -- -g "editor"` some scenes are missing, and
 * nothing is deleted.
 *
 * (Reading the `-g` flag out of the command line was the obvious alternative
 * and it is wrong: Playwright's CLI grep never reaches `FullConfig.grep`, which
 * stays `/.*∕/` no matter what was typed. The record set is the truth.)
 */
function prune(records: SceneRecord[]): number {
  if (SCENES.some((s) => !records.some((r) => r.id === s.id))) return 0;
  const kept = new Set(
    records.flatMap((r) => r.files.map((f) => join(ATLAS_DIR, f))),
  );
  let removed = 0;
  for (const file of listPngs(ATLAS_DIR)) {
    if (kept.has(file)) continue;
    rmSync(file, { force: true });
    removed += 1;
  }
  return removed;
}

/** Called once by globalTeardown. */
export function writeReports(): void {
  const records = readRecords();
  prune(records);
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

  const groups = [...new Set(SCENES.map((s) => s.page))];
  const sections = groups.map((page) => {
    const rows = SCENES.filter((s) => s.page === page).map((s) => {
      const shots = byScene.get(s.id) ?? [];
      const plain = (locale: string): string => {
        const hit = shots
          .flatMap((r) => (r.locale === locale ? r.files : []))
          .find((f) => !/--(full|1000x760)\.png$/.test(f));
        return hit ? `[\`${hit}\`](${hit})` : "—";
      };
      const extras = [
        ...new Set(
          shots
            .flatMap((r) => r.files)
            .filter((f) => /--(full|1000x760)\.png$/.test(f)),
        ),
      ]
        // The LOCALE stays in the label. Two links reading
        // «opptak--klar--full.png» side by side is a column that has stopped
        // saying which one is which.
        .map((f) => `[\`${f}\`](${f})`)
        .join(" · ");
      return `| \`${s.id}\` | ${s.state} | ${s.recipe} | ${plain("no")} | ${plain("en")} | ${extras || "—"} |`;
    });
    return `### ${page}\n\n| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |\n| --- | --- | --- | --- | --- | --- |\n${rows.join("\n")}`;
  });

  const files = listPngs(ATLAS_DIR);
  const bytes = files.reduce((n, f) => n + statSync(f).size, 0);
  const sizeLine =
    `**Total størrelse:** ${mb(bytes)} i ${files.length} PNG-er.` +
    (squeezed
      ? ` Komprimert med \`${squeezed.tool}\`: ${mb(squeezed.before)} → ${mb(squeezed.after)}.`
      : " Ingen PNG-komprimator i PATH (pngquant / magick / oxipng / optipng) — bildene er slik Playwright skrev dem.");

  const missing = SCENES.filter((s) => !byScene.has(s.id)).map((s) => s.id);

  const md = `# Atlas — SundayRec slik appen ER

Hvert bilde er ett skjermbilde av appen slik den står på \`main\` i dag: D3-skallet
(topplinje + bunnlinje, tre destinasjoner og et tannhjul), D2-kontrollrommet på
OPPTAK, og V1-runden (diagnoserad, hevede treffflater, rettede tekster).
Ingenting her er et forslag — dette er den visuelle regresjonsbasen.

Arkivet fra v0.15 ligger i [\`../atlas-v015/\`](../atlas-v015/INDEX.md), sammen
med IA-uttrekket i [\`../ATLAS.md\`](../ATLAS.md). De to viser appen slik den
VAR, og skal ikke forveksles med denne mappa.

**Vindu:** 1180×760 (Tauri-vinduets standardstørrelse). Hovedscenen på hver
destinasjon er i tillegg fotografert på 1000×760 — bredden der kontrollrommet
faller til ÉN kolonne — med suffikset \`--1000x760\` (norsk bare: om en layout
overlever én kolonne er ikke et språkspørsmål). Sider som ruller forbi vindushøyden har
et \`--full\`-skudd som viser hele siden (vindushøyden settes til innholdets
høyde; Playwrights egen \`fullPage\` gir ingenting her, fordi appen ruller inne i
\`#main\`, ikke i dokumentet).

**Språk:** \`no/\` og \`en/\`. De fem andre språkene i språkvelgeren er satt på
pause og er ikke fotografert.

${sizeLine}

## Kjøre på nytt

\`\`\`bash
npm run atlas                    # hele atlaset (starter Vite selv, port 1421)
npm run atlas -- -g "editor"     # bare scener som matcher
SUNDAYREC_ATLAS_PORT=1431 npm run atlas   # når 1421 er opptatt
\`\`\`

Atlaset er bevisst utenfor \`npm run check\` og utenfor CI: det er et
fotoapparat, ikke en port. \`playwright.config.ts\` ignorerer \`e2e/atlas/\` med
en regex (\`npx playwright test --list | grep -c atlas\` skal gi 0).

**Egen port med vilje.** Nettleser-tieren bruker \`SUNDAYREC_E2E_PORT\` (1420);
atlaset bruker \`SUNDAYREC_ATLAS_PORT\` (1421), begge med \`--strictPort\`. Uten
det ville en fotografering startet mens \`npm run e2e\` kjører festet seg til
DEN serveren gjennom \`reuseExistingServer\` — og i en worktree fotografert et
annet utsjekk enn det man står i.

**To kjøringer skal gi identiske filer.** Klokka er låst, VU-pakkene er
konstante og animasjonene er ferdige før lukkeren går. Diff-er
\`shasum\`-summene etter to kjøringer: en fil som endrer seg uten at koden gjorde
det, er en scene som ikke står stille.

## Scener

${sections.join("\n\n")}
${missing.length ? `\n**Ikke fotografert i denne kjøringen:** ${missing.map((m) => `\`${m}\``).join(", ")}.\n` : ""}
## Hvordan scenene lages

Scenetabellen ligger i \`e2e/atlas/scenes.ts\`. Hver scene er
\`{ fixtures, settings, goto }\` gjennom \`e2e/harness.ts\` (api-shim-sømmen),
pluss eventuelle klikk mot \`data-testid\`. \`e2e/atlas/harness.ts\` legger til
det den vanlige nettleser-tieren ikke har:

1. **Backend-hendelser.** En stor del av tilstandene males av en Tauri-event,
   ikke av et kall: opptaksmåleren (\`recording://levels\`), tapt-opptak-kortet
   (\`scheduler://missed\`), forhåndssjekken (\`scheduler://preflight\`),
   nedtellingen før auto-stopp, oppdateringsbanneret. Brua er
   \`e2e/events.ts\` — den samme de vanlige spec-ene bruker — og den må
   installeres FØR \`boot()\`. \`emit\`/\`emitEvent\` returnerer hvor mange
   lyttere som tok imot; **0 lyttere feiler scenen**, slik at atlaset ikke
   fotograferer feil skjerm i stillhet.
2. **En fast klokke.** \`page.clock.setFixedTime\` låser \`Date.now()\` til
   søndag 23. august 2026 kl. 10:55, slik at «om 3 dager», «for 2 timer siden»
   og opptakstelleren er de samme i to kjøringer.
3. **\`settle()\`.** \`toBeVisible()\` betyr IKKE «malt»: Playwrights synlighet
   er boks + \`display\`/\`visibility\`, og sier ingenting om OPACITY. Så
   fotografen venter til hver endelige animasjon og overgang er ferdig
   (uendelige — spinnere — er unntatt, ellers ville en travel skjerm aldri falt
   til ro), og deretter én \`requestAnimationFrame\` for rendererens egne
   malere (VU-barene, waveform-canvaset).
4. **VU-pakker som har konvergert.** \`settleVu\` sender identiske pakker til
   utjevningen har stabilisert seg og stopper — måleren holder siste malte
   posisjon, fordi det er pakkene som driver malingen.
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

  const md = `# Konsollfunn under fotograferingen

Hver scene i atlaset kjøres med en lytter på \`console.error\` og
\`pageerror\`; alt som falt ut står her.

## Slik er funnene klassifisert

Atlaset kjører **uten backend**. \`api-shim\` fanger hvert avviste \`invoke\` og
returnerer kallerens fallback, så «ingen backend» er den sanne tilstanden, ikke
en feil. Meldinger som matcher \`no Tauri backend in the browser tier\` (og
\`asset://\`-URL-er som aldri kan lastes i en nettleser) er derfor ført opp som
**harness-støy**. Alt annet er ekte konsollgjeld appen bærer i dag.

Grensetilfeller er ført opp som **ekte funn**, ikke som støy: en melding som
ikke kan klassifiseres med sikkerhet skal være synlig. Lista over støymønstre er
med vilje kort og spesifikk — et bredt mønster her er den stilleste måten å
miste et ekte funn på.

Merk også at appens egne \`console.warn\` for hvert fallback IKKE fanges her —
vakten ser bare \`console.error\` og ufangede unntak. Warn-strømmen er stor og
forventet uten backend.

## Ekte funn

Les hver rad sammen med scenens oppskrift i [INDEX.md](INDEX.md): noen scener
**fyrer en feil med vilje** (feilbannere, feilede lastinger, feilede lagringer).
En \`console.error\` derfra er riktig oppførsel, ikke gjeld.

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
