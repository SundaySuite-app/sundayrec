/**
 * One-time cleartext-secret purge — pure, DOM-free (E1.6).
 *
 * `saveSettingsLocal` in `api-shim.ts` now strips `emailSmtpPass` before every
 * write, but that fix is write-forward only: an install that saved a password
 * before the strip landed still carries it in its EXISTING `sundayrec.settings`
 * localStorage blob, forever, until the user happens to touch a setting that
 * triggers a re-save. This is the pure half of the fix — given the raw JSON
 * string from localStorage, decide whether it still has a cleartext secret in
 * it and return the cleaned JSON. The caller (`api-shim.ts`) owns the actual
 * localStorage read/write and the once-per-install guard.
 *
 * Keys stripped: `emailSmtpPass` (the field this was written for — see
 * `types/index.ts`, "runtime only — always '' in store") and its sibling
 * `emailSmtpPassEnc` (documented in the same type as an Electron-era
 * `safeStorage` ciphertext; nothing in the current code writes it, but an old
 * blob carried over from the Electron build could still have it). `emailSmtpPassSet`
 * is a boolean flag, not a secret, and is left alone.
 */

const LEGACY_SECRET_KEYS = ['emailSmtpPass', 'emailSmtpPassEnc'] as const

/**
 * Strip any legacy cleartext-secret keys from a settings blob.
 *
 * `changed` is false (and `out` echoes `blobJson` unchanged) whenever there is
 * nothing to do — including when `blobJson` is missing/empty/corrupt JSON, so
 * the caller never needs its own try/catch around this. `out` is always
 * re-serialized JSON when `changed` is true (key order is not preserved).
 */
export function stripLegacySecrets(blobJson: string | null | undefined): {
  changed: boolean
  out: string
} {
  if (!blobJson) return { changed: false, out: blobJson ?? '' }

  let parsed: unknown
  try {
    parsed = JSON.parse(blobJson)
  } catch {
    return { changed: false, out: blobJson }
  }

  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return { changed: false, out: blobJson }
  }

  const obj = parsed as Record<string, unknown>
  let changed = false
  for (const key of LEGACY_SECRET_KEYS) {
    if (key in obj) {
      delete obj[key]
      changed = true
    }
  }

  if (!changed) return { changed: false, out: blobJson }
  return { changed: true, out: JSON.stringify(obj) }
}
