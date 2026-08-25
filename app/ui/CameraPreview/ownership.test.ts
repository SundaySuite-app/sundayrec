import { afterEach, describe, expect, it, vi } from "vitest";

import {
  cameraPreviewCount,
  registerCameraPreview,
  releaseCameraPreview,
  resumeCameraPreview,
  type CameraPreviewOwner,
} from "./ownership";

// Registeret er modul-globalt, som eierskapet det speiler (det er ÉN maskin og
// ett kamera). Hver test rydder etter seg, ellers ville neste test målt naboens
// preview.
const cleanup: Array<() => void> = [];
function register(owner: CameraPreviewOwner): () => void {
  const off = registerCameraPreview(owner);
  cleanup.push(off);
  return off;
}
afterEach(() => {
  while (cleanup.length) cleanup.pop()!();
  expect(cameraPreviewCount()).toBe(0);
});

function owner() {
  return { stop: vi.fn<() => void>(), resume: vi.fn<() => void>() };
}

describe("kamera-eierskapet", () => {
  it("slipper hver registrert preview", () => {
    const a = owner();
    const b = owner();
    register(a);
    register(b);
    releaseCameraPreview();
    expect(a.stop).toHaveBeenCalledTimes(1);
    expect(b.stop).toHaveBeenCalledTimes(1);
    expect(a.resume).not.toHaveBeenCalled();
  });

  it("angrer slippet når det ikke ble noe opptak likevel", () => {
    const a = owner();
    register(a);
    releaseCameraPreview();
    resumeCameraPreview();
    expect(a.resume).toHaveBeenCalledTimes(1);
  });

  it("en utmeldt preview blir verken stoppet eller gjenopptatt", () => {
    const a = owner();
    register(a)();
    expect(cameraPreviewCount()).toBe(0);
    releaseCameraPreview();
    resumeCameraPreview();
    expect(a.stop).not.toHaveBeenCalled();
    expect(a.resume).not.toHaveBeenCalled();
  });

  it("en eier som melder seg ut MENS vi slipper, tar ikke ned løkka", () => {
    // Den ekte formen: komponentens `stop` fører til en avmontering, som kjører
    // `useEffect`-opprydningen, som melder seg ut av settet vi står i.
    const order: string[] = [];
    let off: (() => void) | null = null;
    off = register({
      stop: () => {
        order.push("a");
        off?.();
      },
      resume: () => {},
    });
    register({ stop: () => order.push("b"), resume: () => {} });
    releaseCameraPreview();
    expect(order).toEqual(["a", "b"]);
  });

  it("uten noen registrert er begge bare ikke-kall", () => {
    expect(() => releaseCameraPreview()).not.toThrow();
    expect(() => resumeCameraPreview()).not.toThrow();
  });
});
