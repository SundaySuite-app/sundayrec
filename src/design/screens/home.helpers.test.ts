import { describe, expect, it } from "vitest";

import {
  channelModeLayout,
  inputMeta,
  metersForChannelMode,
} from "./home.helpers";
import type { AudioDeviceList } from "@/lib/bindings/AudioDeviceList";

/** A hardware-MONO default device (like a MacBook built-in mic). */
const MONO_DEVICE: AudioDeviceList = {
  inputs: [
    {
      name: "MacBook Pro-mikrofon",
      is_default: true,
      channels: 1,
      sample_rates: [48000],
    },
  ],
} as unknown as AudioDeviceList;

describe("inputMeta reflects the chosen channel MODE, not device channels", () => {
  it("stereo mode → 'stereo' even on a hardware-mono device", () => {
    expect(inputMeta(MONO_DEVICE, "stereo")).toBe("stereo · 48 kHz");
  });

  it("monoMix mode → 'mono'", () => {
    expect(inputMeta(MONO_DEVICE, "monoMix")).toBe("mono · 48 kHz");
  });

  it("monoL / monoR modes → 'mono'", () => {
    expect(inputMeta(MONO_DEVICE, "monoL")).toBe("mono · 48 kHz");
    expect(inputMeta(MONO_DEVICE, "monoR")).toBe("mono · 48 kHz");
  });

  it("returns null with no devices", () => {
    expect(inputMeta(null, "stereo")).toBeNull();
  });
});

describe("channelModeLayout", () => {
  it("maps stereo → 'stereo' and every mono variant → 'mono'", () => {
    expect(channelModeLayout("stereo")).toBe("stereo");
    expect(channelModeLayout("monoMix")).toBe("mono");
    expect(channelModeLayout("monoL")).toBe("mono");
    expect(channelModeLayout("monoR")).toBe("mono");
  });
});

describe("metersForChannelMode (channel-count confirmation)", () => {
  it("stereo → 2 bars", () => {
    expect(metersForChannelMode("stereo")).toBe(2);
  });

  it("every mono variant → 1 bar", () => {
    expect(metersForChannelMode("monoMix")).toBe(1);
    expect(metersForChannelMode("monoL")).toBe(1);
    expect(metersForChannelMode("monoR")).toBe(1);
  });
});
