/**
 * Permission + sidecar findings, folded into the preflight card.
 *
 * `media_permissions` and `ffmpeg_health` are two backend commands that nothing
 * in the renderer has ever called. That mattered most for the microphone: a
 * denied mic makes the device open fail with a generic error and makes
 * avfoundation emit "Input/output error" or simply no frames, so the user was
 * told the device was missing when in fact macOS was blocking it. The answer
 * exists — AVFoundation knows — it just never reached a screen.
 *
 * These findings are shaped as `PreflightFinding`s ON PURPOSE: the preflight card
 * is where the app already says "here is what will stop your recording", and a
 * blocked microphone belongs in exactly that list rather than in a second,
 * competing surface. They are prepended, because a permission the OS is refusing
 * outranks a nearly-full disk.
 *
 * Pure — the decision of what to report and how to word it is unit-tested with no
 * IPC and no DOM.
 */

import type { AuthStatus } from "../../../legacy/bindings/AuthStatus";
import type { PreflightFinding } from "../../../legacy/bindings/PreflightFinding";

/** What the two commands answer with (the fields we care about). */
export interface HealthInputs {
  permissions?: { microphone?: AuthStatus; camera?: AuthStatus } | null;
  ffmpeg?: { available?: boolean; path?: string } | null;
  /** Whether the user has video recording enabled — a blocked camera is only a
   *  finding for someone who intends to record video. */
  videoEnabled?: boolean;
  /** Localizer, so the copy stays in the app's language without this module
   *  importing the i18n singleton (which would make it untestable in isolation). */
  t: (key: string, fallback: string) => string;
}

/** Whether the OS is positively blocking the device. Mirrors Rust's
 *  `AuthStatus::is_blocked` — `notDetermined` is NOT blocked (opening the device
 *  is what triggers the prompt), and `unknown` means we could not tell, which
 *  must never be reported as a problem. */
export function isBlocked(status: AuthStatus | undefined): boolean {
  return status === "denied" || status === "restricted";
}

/**
 * Turn the two health probes into preflight findings. Returns an empty array
 * when everything is fine — the card stays away rather than reassuring people
 * about things they never worried about.
 */
export function buildHealthFindings(input: HealthInputs): PreflightFinding[] {
  const { t } = input;
  const out: PreflightFinding[] = [];

  const mic = input.permissions?.microphone;
  if (isBlocked(mic)) {
    out.push({
      severity: "error",
      category: "device",
      message:
        mic === "restricted"
          ? t(
              "health.micRestricted",
              "Mikrofontilgang er sperret av systemadministrator eller foreldrekontroll. Opptak vil ikke virke før sperren oppheves.",
            )
          : t(
              "health.micDenied",
              "Mikrofontilgang er avslått. Åpne Systeminnstillinger → Personvern og sikkerhet → Mikrofon og slå på SundayRec.",
            ),
    });
  }

  const cam = input.permissions?.camera;
  if (input.videoEnabled && isBlocked(cam)) {
    out.push({
      severity: "error",
      category: "device",
      message:
        cam === "restricted"
          ? t(
              "health.cameraRestricted",
              "Kameratilgang er sperret av systemadministrator eller foreldrekontroll. Videoopptak vil ikke virke før sperren oppheves.",
            )
          : t(
              "health.cameraDenied",
              "Kameratilgang er avslått. Åpne Systeminnstillinger → Personvern og sikkerhet → Kamera og slå på SundayRec.",
            ),
    });
  }

  if (input.ffmpeg && input.ffmpeg.available === false) {
    out.push({
      severity: "error",
      category: "device",
      message: t(
        "health.ffmpegMissing",
        "Den innebygde lydmotoren (ffmpeg) ble ikke funnet. Video, eksport og redigering vil ikke virke — installer appen på nytt.",
      ),
    });
  }

  return out;
}
