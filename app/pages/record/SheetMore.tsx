/**
 * «Mer» i lydarket: mappe, kvalitet og fem fliser (Før, Stille, Del, Rydd,
 * Maks) — alt det som før lå som seksjonene Lagringsmappe, Kvalitet og
 * Opptaksvalg på Innstillinger.
 *
 * ## Flisen: trykk = av/på, verdien under = bla gjennom forhåndsvalg
 *
 * En flis er TO knapper (en knapp kan ikke ligge i en knapp): overdelen
 * slår funksjonen av/på, og den lille verdien under blar til neste
 * forhåndsvalg — de samme valgene de gamle nedtrekkslistene tilbød. Er flisen
 * av, står «Av» der i stedet for en verdi.
 *
 * Alt går gjennom `usePatch`/`useSetting`: auto-anvend, kvittering, og
 * tilbakerulling om lagringen feiler.
 */

import { useState } from "preact/hooks";

import { errorCode } from "@lib/error-code-core";

import { t, tDyn, tf } from "../../i18n";
import { usePatch } from "../../settings/use-patch";
import { useReceipt } from "../../settings/use-receipt";
import { useSetting } from "../../settings/use-setting";
import {
  currentRoomMinutes,
  diskFreeBytes,
  refreshDiskSpace,
} from "../../state/disk";
import { patchSettings, settings, type Settings } from "../../state/settings";
import { confirmDialog } from "../../ui/dialog";
import { Icon, type IconName } from "../../ui/Icon/Icon";
import { Receipt } from "../../ui/Receipt/Receipt";
import { toast } from "../../ui/toast";
import { qualityIdFor, type QualityId } from "../settings/decisions-core";
import { folderRefusalMessage } from "../settings/folder-refusal";
import styles from "./SheetMore.module.css";

// ── Forhåndsvalgene — de gamle nedtrekkslistenes egne ───────────────────────

/** Sekundene forhåndsbufferen tilbyr (0 = av). */
export const PREROLL_CHOICES = [15, 30] as const;
/** Minuttene «stopp ved lang stillhet» venter. */
export const SILENCE_CHOICES = [2, 5, 10, 15] as const;
/** Hvor ofte en lang opptaksfil deles. */
export const SPLIT_CHOICES = [30, 45, 60, 90, 120] as const;
/** Slett gamle opptak etter dager. 7 er med, så vakten under 30 d har noe å vokte. */
export const CLEAN_CHOICES = [7, 30, 90, 180, 365] as const;
/** Maks lengde i minutter (0 = ingen grense). */
export const MAX_CHOICES = [60, 120, 180, 240, 360] as const;

/** Standardene en flis slår på med — legacys egne. */
const PREROLL_DEFAULT = 15;
const SPLIT_DEFAULT = 60;
const CLEAN_DEFAULT = 90;
const MAX_DEFAULT = 120;
/** Under dette spør vi først — samme grense som legacy. */
const CLEAN_GUARD_DAYS = 30;

/** Neste forhåndsvalg etter `current` (den neste større, ellers den første). */
export function nextChoice(
  choices: readonly number[],
  current: number,
): number {
  return choices.find((n) => n > current) ?? choices[0]!;
}

/** «90 min» / «1 t» / «2 t» — kort nok til å stå under en flis. */
function minutesText(n: number): string {
  return n % 60 === 0
    ? tf("app.sheet.unit.h", { n: n / 60 })
    : tf("app.sheet.unit.min", { n });
}

export function SheetMore({ initiallyOpen }: { initiallyOpen: boolean }) {
  const [open, setOpen] = useState(initiallyOpen);
  return (
    <>
      <button
        type="button"
        data-testid="sheet-more-toggle"
        aria-expanded={open}
        aria-controls="sheet-more"
        class={styles.more}
        onClick={() => setOpen(!open)}
      >
        <span class={`${styles.chev} ${open ? styles.chevOpen : ""}`}>
          <Icon name="chevron-up" size={18} strokeWidth={2} />
        </span>
        <span>{t("app.sheet.more")}</span>
        <span aria-hidden="true" class={styles.rule} />
      </button>
      {open ? (
        <div id="sheet-more" data-testid="sheet-more" class={styles.body}>
          <FolderRow />
          <QualityCards />
          <Tiles />
        </div>
      ) : null}
    </>
  );
}

// ── Mappen ──────────────────────────────────────────────────────────────────

/** «412 GB ledig · plass til 300 t», eller bare det halve vi vet. */
function spaceText(): string {
  const free = diskFreeBytes.value;
  if (free === null) return t("app.setup.folder.unknownSpace");
  const gb = Math.round(free / 1e9);
  const minutes = currentRoomMinutes();
  if (minutes === null) return tf("app.setup.folder.free", { gb });
  return tf("app.setup.folder.space", { gb, hours: Math.floor(minutes / 60) });
}

/**
 * Mappen velges i OS-ets egen dialog, som RUST åpner (`settingsPickSaveFolder`)
 * — dialogen ER autorisasjonen, og siden den åpnes av prosessen kan ingen sti
 * sendes fra nettvisningen uten at en dialog ble vist. Svaret er de lagrede
 * innstillingene. Avviste mapper (en app, roten av disken …) forklares i
 * toasten.
 */
function FolderRow() {
  const folder = (settings.value.saveFolder ?? "").trim();
  const { receipt, show, reset } = useReceipt();
  const [picking, setPicking] = useState(false);

  async function pick(): Promise<void> {
    if (picking) return;
    setPicking(true);
    try {
      const answer = await window.api.settingsPickSaveFolder();
      if (!answer.ok) {
        toast(
          "error",
          folderRefusalMessage(errorCode(answer.error)) ??
            t("general.saveFailed"),
        );
        show("failed");
        return;
      }
      // Avbrutt dialog: ingen endring, ingen kvittering.
      if (!answer.settings) {
        reset();
        return;
      }
      patchSettings({ saveFolder: answer.settings.saveFolder ?? null });
      show("saved");
      // Ny disk, nytt tall.
      await refreshDiskSpace();
    } finally {
      setPicking(false);
    }
  }

  return (
    <div data-testid="setup-folder" class={styles.folderWrap}>
      <button
        type="button"
        data-testid="folder-pick"
        aria-busy={picking ? "true" : undefined}
        title={t("app.sheet.folder")}
        aria-label={t("app.sheet.folder")}
        class={styles.folder}
        onClick={() => void pick()}
      >
        <span aria-hidden="true" class={styles.folderIcon}>
          <Icon name="folder" size={24} />
        </span>
        <span class={styles.folderText}>
          <span data-testid="folder-path" class={styles.path}>
            {folder || t("app.setup.folder.pick")}
          </span>
          <span data-testid="folder-space" class={styles.space}>
            {folder ? spaceText() : t("app.setup.folder.noneDesc")}
          </span>
        </span>
      </button>
      <Receipt state={receipt} testId="folder-receipt" />
    </div>
  );
}

// ── Kvaliteten ──────────────────────────────────────────────────────────────

/** De tre kortene, og hva hvert av dem SKRIVER. Stolpene er «hvor mye». */
const QUALITY: ReadonlyArray<{
  id: QualityId;
  format: "mp3" | "flac" | "wav";
  bitrate: string | null;
  bars: 1 | 2 | 3;
}> = [
  { id: "mp3", format: "mp3", bitrate: "256", bars: 1 },
  { id: "flac", format: "flac", bitrate: null, bars: 2 },
  { id: "wav", format: "wav", bitrate: null, bars: 3 },
];

/**
 * God / Best / Arkiv. Hvert kort skriver BEGGE innstillingene (`format` og —
 * for MP3 — `bitrate`) i ÉN lagring: å la bitraten stå urørt ville gitt en
 * «God»-profil som egentlig er 128 kbps. En lagret kombinasjon som ikke er ett
 * av de tre (MP3 · 320) får en linje over kortene og INGEN valgt — skjermen
 * skal ikke si én ting mens fila blir en annen.
 */
function QualityCards() {
  const s = settings.value;
  const current = qualityIdFor(s);
  const format = useSetting("format", { kind: "radio" });

  return (
    <div data-testid="setup-quality" class={styles.quality}>
      {!current ? (
        <p data-testid="quality-choices-row-custom" class={styles.custom}>
          {tf("app.setup.qualityCustom", {
            format: String(s.format ?? "mp3").toUpperCase(),
            bitrate: String(s.bitrate ?? ""),
          })}
        </p>
      ) : null}
      <div
        data-testid="quality-choices"
        role="group"
        class={styles.qualityGrid}
      >
        {QUALITY.map((card) => (
          <button
            key={card.id}
            type="button"
            data-testid={`quality-choices-row-${card.id}`}
            aria-pressed={current === card.id}
            disabled={format.busy}
            title={tDyn("app.setup.qDesc", card.id)}
            class={`${styles.q} ${current === card.id ? styles.qOn : ""}`}
            onClick={() => {
              // Bitraten FØRST, i samme signal-oppdatering som resten.
              if (card.bitrate) patchSettings({ bitrate: card.bitrate });
              format.set(card.format);
            }}
          >
            <span aria-hidden="true" class={styles.bars}>
              {[1, 2, 3].map((n) => (
                <span
                  key={n}
                  class={`${styles.bar} ${n <= card.bars ? styles.barOn : ""}`}
                  style={{ height: `${5 + n * 5}px` }}
                />
              ))}
            </span>
            <span>{tDyn("app.setup.quality", card.id)}</span>
          </button>
        ))}
      </div>
      <Receipt state={format.receipt} testId="quality-receipt" />
    </div>
  );
}

// ── Flisene ─────────────────────────────────────────────────────────────────

interface TileProps {
  icon: IconName;
  word: string;
  /** Beskrivelsen, som verktøytips. */
  title: string;
  on: boolean;
  /** Verdien under ordet (bare når flisen er på). */
  value: string;
  busy: boolean;
  onToggle: () => void;
  onCycle: () => void;
  testId: string;
}

function Tile({
  icon,
  word,
  title,
  on,
  value,
  busy,
  onToggle,
  onCycle,
  testId,
}: TileProps) {
  return (
    <div class={`${styles.tile} ${on ? styles.tileOn : ""}`}>
      <button
        type="button"
        data-testid={testId}
        aria-pressed={on}
        title={title}
        disabled={busy}
        class={styles.tileMain}
        onClick={onToggle}
      >
        <Icon name={icon} size={22} />
        <span class={styles.word}>{word}</span>
      </button>
      {on ? (
        <button
          type="button"
          data-testid={`${testId}-value`}
          aria-label={`${word}: ${value} — ${t("app.sheet.change")}`}
          title={t("app.sheet.change")}
          disabled={busy}
          class={styles.value}
          onClick={onCycle}
        >
          {value}
        </button>
      ) : (
        <span class={styles.valueOff}>{t("app.sheet.off")}</span>
      )}
    </div>
  );
}

function Tiles() {
  const s = settings.value;
  const save = usePatch();
  const write = (patch: Partial<Settings>): void => void save.write(patch);

  const pre = s.preRollSeconds ?? 0;
  const silenceOn = s.stopOnSilence === true;
  const silenceMin = s.silenceTimeoutMinutes ?? SILENCE_CHOICES[0];
  const split = s.splitMinutes ?? 0;
  const clean = s.autoDeleteDays ?? 0;
  const max = s.manualMaxMinutes ?? 0;

  /** Rydd: under 30 dager spør vi først — en gudstjeneste kan være borte før
   *  noen har rukket å laste den ned. */
  async function writeClean(days: number): Promise<void> {
    if (days > 0 && days < CLEAN_GUARD_DAYS) {
      const ok = await confirmDialog({
        title: t("app.setup.advanced.autoDeleteConfirm"),
        message: t("app.setup.advanced.autoDeleteConfirmBody"),
        confirmLabel: t("app.setup.advanced.autoDeleteConfirmYes"),
        cancelLabel: t("app.setup.cancel"),
      });
      if (!ok) return;
    }
    await save.write({ autoDeleteDays: days });
  }

  return (
    <div class={styles.tiles}>
      <Tile
        icon="rewind"
        word={t("app.sheet.tile.pre")}
        title={t("app.setup.advanced.preroll")}
        on={pre > 0}
        value={tf("app.sheet.unit.s", { n: pre })}
        busy={save.busy}
        testId="tile-pre"
        onToggle={() =>
          write({ preRollSeconds: pre > 0 ? 0 : PREROLL_DEFAULT })
        }
        onCycle={() =>
          write({ preRollSeconds: nextChoice(PREROLL_CHOICES, pre) })
        }
      />
      <Tile
        icon="mute"
        word={t("app.sheet.tile.silence")}
        title={t("app.setup.advanced.silence")}
        on={silenceOn}
        value={tf("app.sheet.unit.min", { n: silenceMin })}
        busy={save.busy}
        testId="tile-silence"
        onToggle={() => write({ stopOnSilence: !silenceOn })}
        onCycle={() =>
          write({
            silenceTimeoutMinutes: nextChoice(SILENCE_CHOICES, silenceMin),
          })
        }
      />
      <Tile
        icon="scissors"
        word={t("app.sheet.tile.split")}
        title={t("app.setup.advanced.split")}
        on={split > 0}
        value={minutesText(split)}
        busy={save.busy}
        testId="tile-split"
        onToggle={() => write({ splitMinutes: split > 0 ? 0 : SPLIT_DEFAULT })}
        onCycle={() =>
          write({ splitMinutes: nextChoice(SPLIT_CHOICES, split) })
        }
      />
      <Tile
        icon="trash"
        word={t("app.sheet.tile.clean")}
        title={t("app.setup.advanced.autoDelete")}
        on={clean > 0}
        value={tf("app.sheet.unit.d", { n: clean })}
        busy={save.busy}
        testId="tile-clean"
        onToggle={() => void writeClean(clean > 0 ? 0 : CLEAN_DEFAULT)}
        onCycle={() => void writeClean(nextChoice(CLEAN_CHOICES, clean))}
      />
      <Tile
        icon="timer"
        word={t("app.sheet.tile.max")}
        title={t("app.setup.advanced.maxLen")}
        on={max > 0}
        value={minutesText(max)}
        busy={save.busy}
        testId="tile-max"
        onToggle={() => write({ manualMaxMinutes: max > 0 ? 0 : MAX_DEFAULT })}
        onCycle={() =>
          write({ manualMaxMinutes: nextChoice(MAX_CHOICES, max) })
        }
      />
      {/* Forhåndsbufferen holder mikrofonen åpen — det sies høyt mens den er på. */}
      {pre > 0 ? (
        <p data-testid="tile-pre-note" class={styles.note}>
          {t("app.setup.advanced.prerollDesc")}
        </p>
      ) : null}
      <Receipt state={save.receipt} testId="tiles-receipt" />
    </div>
  );
}
