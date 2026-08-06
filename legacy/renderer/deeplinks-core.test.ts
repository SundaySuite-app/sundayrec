import { describe, expect, it } from 'vitest'

import no from '../locales/no.json'
import { baseName, hasDedicatedCopy, rejectionCopy } from './deeplinks-core'

/** Every reject code the Rust side can emit (commands/deeplink.rs). Kept as a
 *  literal list so adding a backend code without a sentence here is caught. */
const BACKEND_CODES = [
  'missing_recording',
  'path_not_absolute',
  'path_traversal',
  'subtitle_wrong_type',
  'subtitle_unreadable',
  'recording_unreadable',
  'recording_outside_save_folder',
  'unknown_request',
  'request_expired',
  'declined',
]

const lookup = (key: string): unknown =>
  key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], no)

describe('deep-link rejection copy', () => {
  it('has a dedicated sentence for every code the backend can emit', () => {
    const generic = BACKEND_CODES.filter(c => !hasDedicatedCopy(c))
    expect(generic, 'codes falling through to the generic {err} line').toEqual([])
  })

  it('resolves every key it names in no.json', () => {
    // A key that exists only as a fallback renders Norwegian for every user;
    // the locale-parity test then carries it to the other six.
    for (const code of [...BACKEND_CODES, 'something-new-from-the-backend']) {
      const { key } = rejectionCopy(code)
      expect(lookup(key), `${key} missing from no.json`).toBeTypeOf('string')
    }
  })

  it('sends an unrecognised code to the generic line so it is never swallowed', () => {
    const copy = rejectionCopy('some_future_code')
    expect(copy.key).toBe('deeplink.captionsFailed')
    expect(copy.fallback).toContain('{err}')
  })

  it('groups the path-shaped refusals onto one actionable sentence', () => {
    const keys = new Set(
      ['path_not_absolute', 'path_traversal', 'subtitle_unreadable', 'recording_unreadable', 'unknown_request'].map(
        c => rejectionCopy(c).key,
      ),
    )
    expect([...keys]).toEqual(['deeplink.rejectedPath'])
  })

  it('keeps the pre-E1 missing_recording message', () => {
    // The renderer already branched on this exact code before E1; the operator
    // guidance it carries ("import from the editor instead") is still right.
    expect(rejectionCopy('missing_recording').key).toBe('deeplink.captionsNoRecording')
  })
})

describe('baseName', () => {
  it('names the file, not the path', () => {
    expect(baseName('/Users/ola/Opptak/Bønn møte.mp4')).toBe('Bønn møte.mp4')
    expect(baseName('C:\\rec\\service.mp3')).toBe('service.mp3')
  })

  it('is empty for nothing', () => {
    expect(baseName(undefined)).toBe('')
    expect(baseName(null)).toBe('')
    expect(baseName('')).toBe('')
  })

  it('tolerates a trailing separator', () => {
    expect(baseName('/Users/ola/Opptak/')).toBe('Opptak')
  })
})
