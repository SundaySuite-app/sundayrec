/**
 * `sundayrec://` deep-link arrivals.
 *
 * The Rust side has parsed, validated and (for captions) already APPLIED these
 * since the port — `tray/mod.rs::dispatch_deep_link` emits `deeplink://import`
 * and `deeplink://captions` — and nothing in the renderer has ever listened. A
 * hand-off from SundayEdit brought the window forward and then, from the user's
 * point of view, did nothing at all.
 *
 * Payload shapes (src-tauri/src/tray/mod.rs):
 *   deeplink://import   → `{ path, returnTo }`
 *   deeplink://captions → `{ ok: true,  recording, transcriptPath }`
 *                       | `{ ok: false, recording, error }`
 *                       | `{ ok: false, path, error: 'missing_recording' }`
 *
 * Like the status store and the tray dispatcher, this subscribes DIRECTLY: these
 * event names never had an Electron channel, so there is nothing for EVENT_MAP
 * to map.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event'

import { t } from './i18n'
import { toast } from './ui/toast'

/** `deeplink://import` — SundayEdit (or a Finder hand-off) is handing us a file. */
export interface ImportPayload {
  path?: string
  returnTo?: string | null
}

/** `deeplink://captions` — SundayEdit finished captioning and handed the SRT
 *  back. Rust has already written the sidecar when `recording` was known. */
export interface CaptionsPayload {
  ok?: boolean
  recording?: string
  path?: string
  transcriptPath?: string
  error?: string
}

/** The last path component, for a message that names the file rather than a
 *  40-character absolute path. Pure. */
export function baseName(p: string | undefined | null): string {
  if (!p) return ''
  const parts = p.split(/[/\\]/).filter(Boolean)
  return parts[parts.length - 1] ?? ''
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
      if (!path) return
      handlers.openInEditor(path)
      toast('info', t('deeplink.imported', 'Åpnet «{name}» i redigering.')
        .replace('{name}', baseName(path) || path))
    }),
  )

  track(
    listen<CaptionsPayload>('deeplink://captions', e => {
      const p = e.payload ?? {}
      if (p.ok) {
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
        return
      }
      // The one failure the user can act on: SundayEdit didn't say WHICH
      // recording the captions belong to, so Rust could not pick a sidecar.
      const msg = p.error === 'missing_recording'
        ? t('deeplink.captionsNoRecording',
            'Teksting mottatt, men uten å si hvilket opptak den hører til. Åpne opptaket i redigering og importer teksten der.')
        : t('deeplink.captionsFailed', 'Teksting kunne ikke lagres: {err}')
            .replace('{err}', p.error ?? t('general.unknownError', 'ukjent feil'))
      toast('error', msg)
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
