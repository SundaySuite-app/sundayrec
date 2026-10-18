/**
 * Kameraøkta i bakenden, delt av alle visninger. Reglene står i
 * `camera-feed-core.ts`; dette er bare koblingen til `window.api` og timerne.
 */

import { createCameraFeed } from "./camera-feed-core";
import { classifyStartError } from "./camera-preview-core";

export const cameraFeed = createCameraFeed(
  {
    start: (token, onFrame) => window.api.cameraPreviewStart(token, onFrame),
    stop: () => window.api.cameraPreviewStop(),
  },
  classifyStartError,
  {
    set: (fn, ms) => setTimeout(fn, ms),
    clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
  },
);
