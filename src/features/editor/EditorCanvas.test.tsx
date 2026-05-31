import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";

import { EditorCanvas } from "./EditorCanvas";
import { emptyCutState, type CutState, type Segment } from "./editorGeometry";
import i18n from "@/i18n";

// The canvas paints into a 1000×100 viewBox; we mock the rendered box so the
// pointer pixel→second maths is exercised headless. With a 1000 px wide box and
// a 100 s file fit-to-view, 1 px == 0.1 s and clientX maps 1:1 into view units.
const BOX = { left: 0, top: 0, width: 1000, height: 100 } as DOMRect;

const PEAKS = Array.from({ length: 50 }, (_, i) => (i % 10) / 10);
const DURATION = 100;

const SEGMENTS: Segment[] = [
  { start: 0, end: 30, kind: "music" },
  { start: 30, end: 90, kind: "sermon" },
];

/**
 * A controlled harness: holds the cut state + playhead like `EditorPanel` does,
 * and exposes the latest values via data attributes so the test can assert the
 * resulting cut-plan / playhead after a gesture. `onChange` lets a test observe
 * every state transition.
 */
function Harness({
  duration = DURATION,
  segments = SEGMENTS,
  initial,
  onChange,
}: {
  duration?: number;
  segments?: Segment[];
  initial?: CutState;
  onChange?: (s: CutState) => void;
}) {
  const [cutState, setCutState] = useState<CutState>(
    () => initial ?? emptyCutState(),
  );
  const [playheadSec, setPlayheadSec] = useState(0);
  return (
    <div>
      <div data-testid="cuts">{JSON.stringify(cutState.cuts)}</div>
      <div data-testid="idx">{cutState.idx}</div>
      <div data-testid="playhead">{playheadSec}</div>
      <EditorCanvas
        peaks={PEAKS}
        duration={duration}
        segments={segments}
        cutState={cutState}
        onCutStateChange={(s) => {
          setCutState(s);
          onChange?.(s);
        }}
        playheadSec={playheadSec}
        onSeek={setPlayheadSec}
      />
    </div>
  );
}

function getCanvas(): SVGSVGElement {
  const el = screen.getByTestId("editor-canvas") as unknown as SVGSVGElement;
  el.getBoundingClientRect = () => BOX;
  // jsdom has no pointer-capture; stub so the handlers don't throw.
  (el as unknown as { setPointerCapture: () => void }).setPointerCapture =
    () => {};
  (
    el as unknown as { releasePointerCapture: () => void }
  ).releasePointerCapture = () => {};
  return el;
}

function cuts() {
  return JSON.parse(screen.getByTestId("cuts").textContent || "[]") as {
    start: number;
    end: number;
  }[];
}
function playhead(): number {
  return Number(screen.getByTestId("playhead").textContent);
}

/** A pointer drag from `x1` to `x2` at vertical `y` (default below the ruler so
 *  it's a cut-creation drag, not a playhead grab). `shift` disables snap. */
function drag(
  el: SVGSVGElement,
  x1: number,
  x2: number,
  opts: { y?: number; shift?: boolean } = {},
) {
  const y = opts.y ?? 60;
  const shiftKey = !!opts.shift;
  fireEvent.pointerDown(el, {
    clientX: x1,
    clientY: y,
    pointerId: 1,
    button: 0,
  });
  fireEvent.pointerMove(el, {
    clientX: x2,
    clientY: y,
    pointerId: 1,
    shiftKey,
  });
  fireEvent.pointerUp(el, { clientX: x2, clientY: y, pointerId: 1, shiftKey });
}

beforeEach(() => {
  i18n.changeLanguage("no");
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe("EditorCanvas", () => {
  it("renders the waveform, minimap, and toolbar", () => {
    render(<Harness />);
    expect(screen.getByTestId("editor-canvas")).toBeInTheDocument();
    expect(screen.getByLabelText("Oversikt")).toBeInTheDocument();
    expect(screen.getByLabelText("Zoom inn")).toBeInTheDocument();
    // No cuts → whole file remains.
    expect(screen.getByTestId("editor-remaining").textContent).toContain(
      "100.0s",
    );
  });

  it("drag on blank canvas marks a cut (snapped to a segment boundary)", () => {
    render(<Harness segments={[]} />);
    const el = getCanvas();
    // Drag 200 px → 400 px = 20 s → 40 s (no segments → no snap).
    drag(el, 200, 400);
    expect(cuts()).toEqual([{ start: 20, end: 40 }]);
  });

  it("snaps a cut edge to a nearby detected segment boundary", () => {
    render(<Harness />);
    const el = getCanvas();
    // Snap threshold at this zoom = max(0.15, 100/1000*8) = 0.8 s.
    // Drag from ~28 px (2.8 s) to ~305 px (30.5 s). The sermon starts at 30 and
    // music ends at 30 → the end edge (30.5, within 0.8) snaps to 30; the start
    // (2.8) is far from any boundary so it stays.
    drag(el, 28, 305);
    const c = cuts();
    expect(c).toHaveLength(1);
    expect(c[0]!.end).toBe(30);
    expect(c[0]!.start).toBeCloseTo(2.8, 5);
  });

  it("shift-drag disables snapping", () => {
    render(<Harness />);
    const el = getCanvas();
    drag(el, 28, 305, { shift: true });
    const c = cuts();
    expect(c[0]!.start).toBeCloseTo(2.8, 5);
    expect(c[0]!.end).toBeCloseTo(30.5, 5);
  });

  it("a tap (no movement) seeks the playhead instead of cutting", () => {
    render(<Harness segments={[]} />);
    const el = getCanvas();
    fireEvent.pointerDown(el, {
      clientX: 500,
      clientY: 60,
      pointerId: 1,
      button: 0,
    });
    fireEvent.pointerUp(el, { clientX: 500, clientY: 60, pointerId: 1 });
    expect(cuts()).toEqual([]);
    expect(playhead()).toBeCloseTo(50, 5);
  });

  it("dragging a cut handle resizes the cut and commits on release", () => {
    const start: CutState = {
      cuts: [{ start: 20, end: 40 }],
      history: [[{ start: 20, end: 40 }]],
      idx: 0,
    };
    render(<Harness initial={start} segments={[]} />);
    const el = getCanvas();
    // Grab the end handle at 40 s (x=400) and drag it to 60 s (x=600).
    fireEvent.pointerDown(el, {
      clientX: 400,
      clientY: 60,
      pointerId: 1,
      button: 0,
    });
    fireEvent.pointerMove(el, {
      clientX: 600,
      clientY: 60,
      pointerId: 1,
      shiftKey: true,
    });
    expect(cuts()).toEqual([{ start: 20, end: 60 }]);
    fireEvent.pointerUp(el, {
      clientX: 600,
      clientY: 60,
      pointerId: 1,
      shiftKey: true,
    });
    // Commit recorded a new history step.
    expect(Number(screen.getByTestId("idx").textContent)).toBe(1);
  });

  it("clicking the ruler band seeks (and snaps out of a cut)", () => {
    const start: CutState = {
      cuts: [{ start: 30, end: 60 }],
      history: [[{ start: 30, end: 60 }]],
      idx: 0,
    };
    render(<Harness initial={start} segments={[]} />);
    const el = getCanvas();
    // Click inside the ruler (y<28) at 45 s (x=450) — inside the cut → snap to
    // the cut end (60).
    fireEvent.pointerDown(el, {
      clientX: 450,
      clientY: 10,
      pointerId: 1,
      button: 0,
    });
    fireEvent.pointerUp(el, { clientX: 450, clientY: 10, pointerId: 1 });
    expect(playhead()).toBe(60);
  });

  it("right-click on a cut deletes it", () => {
    const start: CutState = {
      cuts: [{ start: 20, end: 40 }],
      history: [[{ start: 20, end: 40 }]],
      idx: 0,
    };
    render(<Harness initial={start} segments={[]} />);
    const el = getCanvas();
    fireEvent.contextMenu(el, { clientX: 300, clientY: 60 });
    expect(cuts()).toEqual([]);
  });

  it("zoom buttons change the visible window (cut maths follows the zoom)", () => {
    render(<Harness segments={[]} />);
    const el = getCanvas();
    // Zoom in (button) → viewport halves around the centre to [25, 75].
    // Now a drag at the SAME pixels marks a DIFFERENT cut, proving the geometry
    // tracks the viewport. 200 px → 400 px in a [25,75] window = 35 s → 45 s.
    fireEvent.click(screen.getByLabelText("Zoom inn"));
    drag(el, 200, 400);
    expect(cuts()).toEqual([{ start: 35, end: 45 }]);
  });

  it("fit-all resets the viewport after a zoom", () => {
    render(<Harness segments={[]} />);
    const el = getCanvas();
    fireEvent.click(screen.getByLabelText("Zoom inn"));
    fireEvent.click(screen.getByText("Vis alt"));
    // Back to the whole file → 200..400 px maps to 20..40 s again.
    drag(el, 200, 400);
    expect(cuts()).toEqual([{ start: 20, end: 40 }]);
  });

  it("ctrl+wheel zooms around the cursor", () => {
    render(<Harness segments={[]} />);
    const el = getCanvas();
    // Zoom in centred on x=200 (20 s). A subsequent drag near there should land
    // inside the new tighter window. We just assert the window tightened by
    // checking a drag spanning the full pixel width now covers < 100 s.
    fireEvent.wheel(el, {
      deltaY: -100,
      ctrlKey: true,
      clientX: 200,
      clientY: 60,
    });
    drag(el, 0, 1000);
    const c = cuts();
    expect(c).toHaveLength(1);
    expect(c[0]!.end - c[0]!.start).toBeLessThan(100);
  });

  it("ignores interaction when there are no peaks", () => {
    render(
      <div>
        <div data-testid="cuts2">empty</div>
        <EditorCanvas
          peaks={[]}
          duration={DURATION}
          cutState={emptyCutState()}
          onCutStateChange={() => {
            throw new Error("should not edit with no peaks");
          }}
          playheadSec={0}
          onSeek={() => {
            throw new Error("should not seek with no peaks");
          }}
        />
      </div>,
    );
    const el = getCanvas();
    // No throw → handlers no-op'd.
    drag(el, 200, 400);
    expect(screen.getByTestId("cuts2").textContent).toBe("empty");
  });
});
