/**
 * Deep-link presentation decisions — pure, DOM-free, i18n-free.
 *
 * The half of `deeplinks.ts` that can be reasoned about without a webview: which
 * sentence a backend rejection code turns into, and how a path becomes a name a
 * human recognises. Kept separate so it carries tests under the node-environment
 * vitest gate (the same split as `dialog-core` / `bind-setting-core`).
 *
 * The codes are `src-tauri/src/commands/deeplink.rs`
 * (`CaptionsReject::code()` + `TakeError::code()`) plus the two the confirm
 * command adds (`declined`). A code with no entry here is NOT swallowed — it
 * falls through to the generic "could not be saved: {err}" line, so a new
 * backend reason surfaces as itself rather than as silence.
 */

/** A localizable string: the locale key plus its Norwegian fallback. */
export interface Copy {
  key: string
  fallback: string
}

/**
 * The message for a rejection code.
 *
 * Grouped rather than translated one-for-one, because the groups are the three
 * different things an operator would DO: fix the file type, move/expect the
 * recording in the recordings folder, or give up on the deep link and import
 * from the editor. `{err}` in the fallthrough is substituted by the caller.
 */
export function rejectionCopy(code: string | undefined): Copy {
  switch (code) {
    case 'missing_recording':
      return {
        key: 'deeplink.captionsNoRecording',
        fallback:
          'Teksting mottatt, men uten å si hvilket opptak den hører til. Åpne opptaket i redigering og importer teksten der.',
      }
    case 'subtitle_wrong_type':
      return {
        key: 'deeplink.rejectedType',
        fallback: 'Teksting ble avvist: filen er ikke en undertekstfil (.srt eller .vtt).',
      }
    case 'recording_outside_save_folder':
      return {
        key: 'deeplink.rejectedOutside',
        fallback:
          'Teksting ble avvist: opptaket ligger utenfor opptaksmappen din. SundayRec skriver bare ved siden av egne opptak.',
      }
    case 'request_expired':
      return {
        key: 'deeplink.rejectedExpired',
        fallback: 'Forespørselen er utløpt. Send tekstingen på nytt fra SundayEdit.',
      }
    case 'declined':
      return { key: 'deeplink.rejectedDeclined', fallback: 'Tekstingen ble ikke lagt til.' }
    case 'path_not_absolute':
    case 'path_traversal':
    case 'subtitle_unreadable':
    case 'recording_unreadable':
    case 'unknown_request':
      return {
        key: 'deeplink.rejectedPath',
        fallback:
          'Teksting ble avvist: filbanen kunne ikke godkjennes. Importer teksten fra redigering i stedet.',
      }
    default:
      return {
        key: 'deeplink.captionsFailed',
        fallback: 'Teksting kunne ikke lagres: {err}',
      }
  }
}

/** Whether `code` has a dedicated sentence (as opposed to the `{err}` fallthrough). */
export function hasDedicatedCopy(code: string | undefined): boolean {
  return rejectionCopy(code).key !== 'deeplink.captionsFailed'
}

/** The last path component, for a message that names the file rather than a
 *  60-character absolute path. Handles both separators. Pure. */
export function baseName(p: string | undefined | null): string {
  if (!p) return ''
  const parts = p.split(/[/\\]/).filter(Boolean)
  return parts[parts.length - 1] ?? ''
}
