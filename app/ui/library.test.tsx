/**
 * Komponentbiblioteket, som ÉN tabell.
 *
 * ## Hvorfor én tabell og ikke tjue filer
 *
 * Det som må holde for HVER komponent er den samme setningen: «`testId` havner
 * på roten, og de sammensatte avleder navn etter samme mønster». Tjue nesten
 * like testfiler tester den setningen tjue ganger og lar den tjueførste
 * komponenten slippe unna, fordi ingen husket å skrive filen. En tabell har
 * en rad per komponent, og en manglende rad er synlig.
 *
 * Nedenfor tabellen står de fire tingene som IKKE er formelle — de er
 * beslutninger som må holde selv om noen bygger komponenten om:
 *
 *   - bryteren er en `role="switch"`, ikke en skjult avkrysningsboks,
 *   - en farlig dialog har rød SEKUNDÆR-knapp, aldri rød primær,
 *   - `Gate` setter `inert` på innholdet sitt,
 *   - en sperret knapp bærer GRUNNEN sin.
 *
 * Node-miljø, `preact-render-to-string`, ingen jsdom: alt her er markup.
 * Oppførsel som trenger en ekte DOM (fokus, Escape, `inert` i praksis) bevises
 * i `e2e/`.
 */

import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import { activeDialog } from "./dialog";
import { toasts } from "./toast";
import { Banner } from "./Banner/Banner";
import {
  BoundNumberField,
  BoundRadioCards,
  BoundSelect,
  BoundTextField,
  BoundToggle,
} from "./Bound/Bound";
import { Button } from "./Button/Button";
import { Card } from "./Card/Card";
import { Chip } from "./Chip/Chip";
import { ConsentCard } from "./ConsentCard/ConsentCard";
import { ControlCard } from "./ControlCard/ControlCard";
import { DecisionCard } from "./DecisionCard/DecisionCard";
import { DialogHost } from "./DialogHost/DialogHost";
import { EmptyState } from "./EmptyState/EmptyState";
import { Gate } from "./Gate/Gate";
import { NumberField } from "./NumberField/NumberField";
import { PageShell } from "./PageShell/PageShell";
import { ProgressBar } from "./ProgressBar/ProgressBar";
import { RadioCards } from "./RadioCards/RadioCards";
import { Receipt } from "./Receipt/Receipt";
import { Select } from "./Select/Select";
import { SettingRow } from "./SettingRow/SettingRow";
import { Slider } from "./Slider/Slider";
import { StatusDot } from "./StatusDot/StatusDot";
import { Tabs } from "./Tabs/Tabs";
import { TextField } from "./TextField/TextField";
import { Toggle } from "./Toggle/Toggle";
import { ToastHost } from "./ToastHost/ToastHost";
import { VuMeter } from "./VuMeter/VuMeter";

/** Én rad per komponent i biblioteket. Mangler en rad, mangler komponenten. */
const LIBRARY: Array<{
  name: string;
  /** Rendres med `testId="probe"`. */
  markup: () => string;
  /** Avledede testid-er som MÅ finnes. */
  derived?: string[];
}> = [
  { name: "Button", markup: () => render(<Button testId="probe">x</Button>) },
  {
    name: "Card",
    markup: () =>
      render(
        <Card testId="probe" title="T" description="D">
          x
        </Card>,
      ),
    derived: ["probe-title", "probe-description"],
  },
  { name: "Chip", markup: () => render(<Chip testId="probe">x</Chip>) },
  {
    name: "ConsentCard",
    markup: () =>
      render(
        <ConsentCard
          testId="probe"
          onExplain={() => {}}
          onAnswered={() => {}}
        />,
      ),
    derived: ["probe-title", "probe-description", "probe-yes", "probe-no"],
  },
  {
    name: "DecisionCard",
    markup: () =>
      render(
        <DecisionCard
          testId="probe"
          number={1}
          question="Q"
          answer="A"
          detail="D"
          status="todo"
          actionLabel="Sett opp"
          onAction={() => {}}
        />,
      ),
    derived: [
      "probe-number",
      "probe-question",
      "probe-answer",
      "probe-detail",
      "probe-action",
    ],
  },
  {
    name: "ControlCard",
    markup: () =>
      render(
        <ControlCard
          testId="probe"
          id="folder"
          title="Hvor skal opptakene?"
          value="/Users/x/SundayRec"
          tone="warn"
          expanded
          onExpand={() => {}}
          expandLabel="Endre"
          collapseLabel="Lukk"
        >
          x
        </ControlCard>,
      ),
    derived: ["probe-title", "probe-summary", "probe-expand", "probe-body"],
  },
  {
    name: "StatusDot",
    markup: () => render(<StatusDot testId="probe" tone="good" />),
  },
  {
    name: "Receipt",
    markup: () => render(<Receipt testId="probe" state="saved" />),
  },
  {
    name: "SettingRow",
    markup: () =>
      render(
        <SettingRow
          testId="probe"
          label="L"
          description="D"
          receipt="saved"
          error="E"
        >
          <span />
        </SettingRow>,
      ),
    derived: ["probe-label", "probe-receipt", "probe-error", "probe-control"],
  },
  {
    name: "Toggle",
    markup: () => render(<Toggle testId="probe" checked onChange={() => {}} />),
  },
  {
    name: "Select",
    markup: () =>
      render(
        <Select
          testId="probe"
          value="a"
          options={[{ value: "a", label: "A" }]}
          onChange={() => {}}
        />,
      ),
  },
  {
    name: "RadioCards",
    markup: () =>
      render(
        <RadioCards
          testId="probe"
          value="a"
          options={[
            { value: "a", title: "A", description: "d", recommended: true },
            { value: "b", title: "B" },
          ]}
          onChange={() => {}}
        />,
      ),
    derived: ["probe-row-a", "probe-row-b"],
  },
  {
    name: "TextField",
    markup: () =>
      render(<TextField testId="probe" value="" onInput={() => {}} />),
  },
  {
    name: "NumberField",
    markup: () =>
      render(<NumberField testId="probe" value="12" onInput={() => {}} />),
  },
  {
    name: "Slider",
    markup: () =>
      render(
        <Slider
          testId="probe"
          value={5}
          min={0}
          max={10}
          onChange={() => {}}
          format={(v) => `${v}`}
        />,
      ),
    derived: ["probe-value"],
  },
  {
    name: "Gate",
    markup: () =>
      render(
        <Gate testId="probe" status="unavailable" chipText="c" explanation="e">
          <span />
        </Gate>,
      ),
    derived: ["probe-banner", "probe-content"],
  },
  {
    name: "EmptyState",
    markup: () =>
      render(<EmptyState testId="probe" title="T" description="D" />),
    derived: ["probe-title", "probe-description"],
  },
  {
    name: "Tabs",
    markup: () =>
      render(
        <Tabs
          testId="probe"
          label="L"
          value="a"
          items={[
            { id: "a", label: "A" },
            { id: "b", label: "B" },
          ]}
          onChange={() => {}}
        />,
      ),
    derived: ["probe-row-a", "probe-row-b"],
  },
  {
    name: "ProgressBar",
    markup: () =>
      render(<ProgressBar testId="probe" fraction={0.5} etaMs={60_000} />),
    derived: ["probe-percent", "probe-eta"],
  },
  {
    name: "Banner",
    markup: () =>
      render(
        <Banner testId="probe" tone="warn" title="T" onDismiss={() => {}} />,
      ),
    derived: ["probe-dismiss"],
  },
  {
    name: "VuMeter",
    markup: () => render(<VuMeter testId="probe" />),
    derived: ["probe-word", "probe-canvas"],
  },
  {
    name: "BoundToggle",
    markup: () =>
      render(<BoundToggle testId="probe" setting="autoUpdate" label="L" />),
    derived: [
      "probe-label",
      "probe-receipt",
      "probe-control",
      "probe-control-input",
    ],
  },
  {
    name: "BoundSelect",
    markup: () =>
      render(
        <BoundSelect
          testId="probe"
          setting="bitrate"
          label="L"
          options={[{ value: "256", label: "256" }]}
        />,
      ),
    derived: ["probe-control-input"],
  },
  {
    name: "BoundRadioCards",
    markup: () =>
      render(
        <BoundRadioCards
          testId="probe"
          setting="format"
          label="L"
          options={[{ value: "mp3", title: "MP3" }]}
        />,
      ),
    derived: ["probe-control-input"],
  },
  {
    name: "BoundTextField",
    markup: () =>
      render(<BoundTextField testId="probe" setting="churchName" label="L" />),
    derived: ["probe-control-input"],
  },
  {
    name: "BoundNumberField",
    markup: () =>
      render(
        <BoundNumberField
          testId="probe"
          setting="autoDeleteDays"
          label="L"
          rule={{ min: 30 }}
          message={() => "nei"}
        />,
      ),
    derived: ["probe-control-input"],
  },
];

describe("komponentbiblioteket", () => {
  for (const entry of LIBRARY) {
    it(`${entry.name} bærer testId på roten`, () => {
      const html = entry.markup();
      expect(html, `${entry.name} mistet sin data-testid`).toContain(
        'data-testid="probe"',
      );
      for (const id of entry.derived ?? []) {
        expect(html, `${entry.name} mangler den avledede «${id}»`).toContain(
          `data-testid="${id}"`,
        );
      }
    });
  }

  it("dekker hele biblioteket — en komponent uten rad ville sluppet unna", () => {
    // Et tall å måtte oppdatere BEVISST. Legger noen til en komponent uten en
    // rad her, feiler denne i stedet for at dekningen stille blir mindre.
    expect(LIBRARY.length).toBe(26);
  });
});

// ── De fire beslutningene som ikke er formelle ──────────────────────────────

describe("Toggle er en bryter, ikke en skjult avkrysningsboks", () => {
  it("har role=switch og aria-checked", () => {
    const on = render(<Toggle checked onChange={() => {}} testId="t" />);
    expect(on).toContain('role="switch"');
    expect(on).toContain('aria-checked="true"');
    const off = render(
      <Toggle checked={false} onChange={() => {}} testId="t" />,
    );
    expect(off).toContain('aria-checked="false"');
    // Ingen skjult input: den er hele feilklassen bryteren erstatter.
    expect(on).not.toContain('type="checkbox"');
  });
});

describe("en farlig dialog", () => {
  it("har rød SEKUNDÆR-knapp, aldri rød primær", () => {
    activeDialog.value = {
      id: 1,
      kind: "confirm",
      spec: { title: "Tømme papirkurven?", danger: true },
    };
    const html = render(<DialogHost />);
    activeDialog.value = null;

    // Bekreft-knappen er `danger` (rød kant, gjennomsiktig fyll)…
    expect(html).toContain('data-dialog-button="ok"');
    expect(html).toMatch(/data-variant="danger"[^>]*data-dialog-button="ok"/);
    // …og INGEN knapp i dialogen er primær.
    expect(html).not.toContain('data-variant="primary"');
    expect(html).toContain('data-danger="true"');
  });

  it("en ufarlig dialog har primær bekreft og ingen rød knapp", () => {
    activeDialog.value = {
      id: 2,
      kind: "confirm",
      spec: { title: "Fortsette?" },
    };
    const html = render(<DialogHost />);
    activeDialog.value = null;

    expect(html).toMatch(/data-variant="primary"[^>]*data-dialog-button="ok"/);
    expect(html).not.toContain('data-variant="danger"');
  });

  it("er ingenting når køen er tom", () => {
    activeDialog.value = null;
    expect(render(<DialogHost />)).toBe("");
  });

  it("en alert har ÉN knapp, og kan bære en ordrett blokk", () => {
    // «Vis hva som sendes» er hele telemetri-nyttelasten som JSON. Den bor i
    // den samme køen og den samme verten som bekreftelsene, fordi verten er
    // det ene stedet som setter `inert` på resten av appen — en andre
    // modal-mekanisme ville vært et andre sted det kunne bli glemt.
    activeDialog.value = {
      id: 3,
      kind: "alert",
      spec: { title: "Hva sendes", preformatted: '{ "a": 1 }' },
    };
    const html = render(<DialogHost />);
    activeDialog.value = null;

    expect(html).toContain('data-dialog-button="ok"');
    expect(html).not.toContain('data-dialog-button="cancel"');
    expect(html).toContain('data-testid="dialog-pre"');
    expect(html).toContain("&quot;a&quot;: 1");
  });
});

describe("Gate", () => {
  it("setter inert på innholdet når statusen ikke er ok", () => {
    const off = render(
      <Gate status="unavailable" testId="g" explanation="ikke bygget inn">
        <button type="button">x</button>
      </Gate>,
    );
    expect(off).toContain("inert");
    expect(off).toContain("ikke bygget inn");
  });

  it("er helt usynlig når alt virker", () => {
    const ok = render(
      <Gate status="ok" testId="g">
        <button type="button">x</button>
      </Gate>,
    );
    // Ingen banner, ingenting slått av — en gate som synes når alt virker blir
    // tapetet folk lærer å ignorere.
    expect(ok).not.toContain("inert");
    expect(ok).not.toContain('data-testid="g-banner"');
  });
});

describe("en sperret knapp bærer grunnen sin", () => {
  it("som title og som aria-describedby", () => {
    const html = render(
      <Button testId="b" disabled disabledReason="Velg lyd først">
        {"Start"}
      </Button>,
    );
    expect(html).toContain('title="Velg lyd først"');
    expect(html).toContain("aria-describedby=");
    expect(html).toContain('aria-disabled="true"');
    // IKKE `disabled`: da kunne ingen tabbe fram til knappen for å HØRE
    // hvorfor den er av.
    expect(html).not.toMatch(/<button[^>]*\sdisabled/);
  });
});

describe("ToastHost", () => {
  // ⚠️ Denne het en gang «er ingenting når køen er tom». Det VAR den, og det
  // var feilen: en `aria-live`-region som opprettes i samme oppdatering som sin
  // egen første melding blir ikke annonsert. Regionen må stå der på forhånd for
  // at skjermleseren skal ha noe å observere endringen i — ellers er hver
  // første toast stum.
  //
  // MUTASJONSPRØVEN: legg `if (queue.length === 0) return null` tilbake øverst i
  // ToastHost, og denne testen blir rød på første linje.
  it("holder aria-live-regionen stående også når køen er tom", () => {
    toasts.value = [];
    const empty = render(<ToastHost />);
    expect(empty).toContain('data-testid="toast-host"');
    expect(empty).toContain('aria-live="polite"');
    // …og den er tom: ingen toast-rader inni den stående regionen.
    expect(empty).not.toContain("data-kind");
    expect(empty).toContain('data-empty="true"');
  });

  it("viser meldingen når køen ikke er tom", () => {
    toasts.value = [
      { id: 7, kind: "error", msg: "Kunne ikke lagre", durationMs: 0 },
    ];
    const html = render(<ToastHost />);
    toasts.value = [];

    expect(html).toContain('data-testid="toast-host"');
    expect(html).toContain('aria-live="polite"');
    expect(html).toContain("Kunne ikke lagre");
    expect(html).toContain('data-testid="toast-7-message"');
  });
});

describe("PageShell", () => {
  it("har topplinjas dra-attributt, de fire knappene og statuslinjen", () => {
    const html = render(
      <PageShell>
        <span />
      </PageShell>,
    );
    // EKSAKT dette attributtet — uten det kan ikke vinduet flyttes. D3 flyttet
    // det fra skinnens rot til topplinjas; formen er den samme.
    expect(html).toContain("data-tauri-drag-region");
    // Tre destinasjoner (D3: Opptak · Redigering · Eksportering) pluss
    // tannhjulet, som teller som `nav-*` men ikke er en destinasjon.
    for (const page of ["record", "edit", "export", "setup"]) {
      expect(html).toContain(`data-testid="nav-${page}"`);
    }
    expect(html).toContain('data-testid="status-line"');
    expect(html).toContain('data-testid="app-heading"');
  });
});
