/**
 * Hvilke kanaler målerne viser for kilden som er valgt — det samme som
 * opptaket tar opp: et valgt par på en flerkanals enhet, ellers kanal 1 og 2
 * i den lagrede kanalmodusen (mono speiler).
 */

import type { VuPick } from "@lib/audio/vu-feed";

import type { Settings } from "../../state/settings";
import type { SourceState } from "./record-core";

export function pickForSource(
  s: Pick<Settings, "channels">,
  source: Pick<SourceState, "pair">,
): VuPick {
  if (source.pair) {
    return { mode: "stereo", chL: source.pair.l - 1, chR: source.pair.r - 1 };
  }
  return { mode: s.channels ?? "stereo", chL: 0, chR: 1 };
}
