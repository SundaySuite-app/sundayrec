/**
 * `sundayrec://` deep-link arrivals.
 *
 * The Rust side has parsed, validated and (for captions) already APPLIED these
 * since the port — `tray/mod.rs::dispatch_deep_link` emits `deeplink://import`
 * and `deeplink://captions` — and nothing in the renderer has ever listened. A
 * hand-off from SundayEdit brought the window forward and then, from the user's
 * point of view, did nothing at all.
 *
 * ## E1.1 — a deep link now ASKS before it writes
 *
 * A `sundayrec://` URL is not a user action: any web page that can trigger a
 * custom-scheme navigation reaches the backend. Rust therefore validates the
 * request (the recording must resolve inside the save folder, the caption file
 * must be a real `.srt`/`.vtt`), PARKS it under a single-use id, and emits
 * `deeplink://captions-confirm`. This module shows the question; the write only
 * happens when the user says yes and we call `deeplink_confirm_captions` with
 * the parked id. Declining — or simply never answering — writes nothing, and no
 * amount of renderer misbehaviour can widen what Rust already validated.
 *
 * Payload shapes (src-tauri/src/tray/mod.rs + commands/deeplink.rs):
 *   deeplink://import           → `{ path, returnTo }` | `{ error }`
 *   deeplink://captions-confirm → `{ requestId, recording, recordingName,
 *                                    subtitle, subtitleName }`
 *   deeplink://captions         → `{ ok: false, recording, error }`  (rejections)
 *
 * Like the status store and the tray dispatcher, this subscribes DIRECTLY: these
 * event names never had an Electron channel, so there is nothing for EVENT_MAP
 * to map.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

import { baseName, rejectionCopy } from './deeplinks-core'
import { t } from './i18n'
import { confirmDialog } from './ui/dialog'
import { toast } from './ui/toast'

export { baseName } from './deeplinks-core'

/** `deeplink://import` — SundayEdit (or a Finder hand-off) is handing us a file.
 *  `error` is set instead of `path` when Rust refused the hand-off. */
export interface ImportPayload {
  path?: string
  returnTo?: string | null
  error?: string
}

/** `deeplink://captions` — the RESULT of a captions hand-back. Since E1.1 the
 *  backend emits this only for rejections; a successful import is the return
 *  value of `deeplink_confirm_captions`. */
export interface CaptionsPayload {
  ok?: boolean
  recording?: string
  path?: string
  transcriptPath?: string
  error?: string
}

/** `deeplink://captions-confirm` — a VALIDATED, parked hand-back waiting for the
 *  operator's answer. Nothing has been written yet. */
export interface CaptionsConfirmPayload {
  requestId?: string
  recording?: string
  recordingName?: string
  subtitle?: string
  subtitleName?: string
}

/**
 * Turn a backend rejection code into the sentence the operator reads. The
 * key→fallback choice is the pure, unit-tested `rejectionCopy`; this only
 * localizes it and fills `{err}` for the fallthrough.
 */
export function rejectionMessage(code: string | undefined): string {
  const copy = rejectionCopy(code)
  return t(copy.key, copy.fallback).replace(
    '{err}',
    code ?? t('general.unknownError', 'ukjent feil'),
  )
}

/** Toast a landed transcript + refresh the transcript surfaces. */
function announceCaptions(p: CaptionsPayload, handlers: DeeplinkHandlers): void {
  const name = baseName(p.recording) || t('deeplink.theRecording', 'opptaket')
  toast(
    'success',
    t('deeplink.captionsOk', 'Teksting mottatt for «{name}».').replace('{name}', name),
    {
      action: {
        label: t('deeplink.openEditor', 'Åpne i redigering'),
        onClick: () => { if (p.recording) handlers.openInEditor(p.recording) },
      },
    },
  )
  handlers.refreshTranscripts?.()
}

/**
 * Ask before the write, then hand the answer back to Rust.
 *
 * The dialog is the user's decision, but it is NOT the security boundary: Rust
 * already refused everything it would not write, and re-validates the parked
 * request when the answer comes back. Declining still calls the command (with
 * `accept: false`) so the parked entry is released immediately instead of
 * lingering until its TTL.
 */
async function askAndApplyCaptions(
  p: CaptionsConfirmPayload,
  handlers: DeeplinkHandlers,
): Promise<void> {
  const requestId = p.requestId
  if (!requestId) return
  const recordingName = p.recordingName || baseName(p.recording) || t('deeplink.theRecording', 'opptaket')
  const subtitleName = p.subtitleName || baseName(p.subtitle) || ''

  const accept = await confirmDialog({
    title: t('deeplink.confirmCaptionsTitle', 'Legge til teksting for «{name}»?')
      .replace('{name}', recordingName),
    message: t(
      'deeplink.confirmCaptionsBody',
      'En annen app har sendt tekstfilen «{file}». Sier du ja, lagres teksten ved siden av opptaket.',
    ).replace('{file}', subtitleName),
    confirmLabel: t('deeplink.confirmCaptionsAccept', 'Ja, legg til teksting'),
    cancelLabel: t('deeplink.confirmCaptionsReject', 'Nei, ikke gjør noe'),
  }).catch(() => false)

  const result = await invoke<CaptionsPayload>('deeplink_confirm_captions', {
    requestId,
    accept,
  }).catch(() => null)

  if (!accept) return
  if (result?.ok) {
    announceCaptions({ ...result, recording: p.recording }, handlers)
    return
  }
  toast('error', rejectionMessage(result?.error))
}

export interface DeeplinkHandlers {
  /** Open a file in the editor. */
  openInEditor: (path: string) => void
  /** Re-read transcript sidecars after captions landed. */
  refreshTranscripts?: () => void
}

const unlisteners: UnlistenFn[] = []
let started = false

/** Wire the two listeners. Idempotent. */
export function initDeeplinks(handlers: DeeplinkHandlers): void {
  if (started) return
  started = true

  const track = (p: Promise<UnlistenFn>): void => {
    p.then(u => unlisteners.push(u)).catch(err =>
      console.warn('[deeplink] listen failed:', err),
    )
  }

  track(
    listen<ImportPayload>('deeplink://import', e => {
      const path = e.payload?.path
      if (!path) {
        // Rust refused the hand-off (not a media file, not absolute, protected
        // location). Say so — a silently ignored deep link is what the pre-E1
        // renderer did, and it taught nobody anything.
        if (e.payload?.error) {
          toast('error', t(
            'deeplink.importRejected',
            'Filen kunne ikke åpnes: SundayRec åpner bare lyd- og videofiler fra en kjent mappe.',
          ))
        }
        return
      }
      handlers.openInEditor(path)
      toast('info', t('deeplink.imported', 'Åpnet «{name}» i redigering.')
        .replace('{name}', baseName(path) || path))
    }),
  )

  // A VALIDATED hand-back, parked in Rust, waiting for a yes. This is the only
  // thing that can turn into a sidecar write.
  track(
    listen<CaptionsConfirmPayload>('deeplink://captions-confirm', e => {
      void askAndApplyCaptions(e.payload ?? {}, handlers)
    }),
  )

  // Rejections (and any legacy success payload) land here.
  track(
    listen<CaptionsPayload>('deeplink://captions', e => {
      const p = e.payload ?? {}
      if (p.ok) {
        announceCaptions(p, handlers)
        return
      }
      toast('error', rejectionMessage(p.error))
    }),
  )

  window.addEventListener('beforeunload', () => {
    for (const u of unlisteners.splice(0)) {
      try {
        u()
      } catch {
        /* teardown is best-effort */
      }
    }
  })
}
