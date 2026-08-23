import { describe, expect, it, vi } from 'vitest'

import { createTrayDispatcher, TRAY_ACTION_IDS } from './tray-actions'

describe('createTrayDispatcher', () => {
  it('routes every id the Rust tray can send', () => {
    const calls: string[] = []
    const dispatch = createTrayDispatcher({
      startRecording: () => calls.push('start'),
      stopRecording: () => calls.push('stop'),
      openRecordingsFolder: () => calls.push('folder'),
      runPreflight: () => calls.push('preflight'),
      runDiagnostics: () => calls.push('diagnostics'),
    })

    for (const id of TRAY_ACTION_IDS) expect(dispatch(id)).toBe(true)
    expect(calls).toEqual(['start', 'stop', 'folder', 'preflight', 'diagnostics'])
  })

  it('covers exactly the ids the tray emits to the renderer', () => {
    // The Rust side (`src-tauri/src/tray/mod.rs::action_id`) owns the strings;
    // `open-window` / `show-on-error` / `quit` / `none` are handled entirely in
    // Rust and must NOT be claimed here, or a menu click would fire twice.
    expect([...TRAY_ACTION_IDS].sort()).toEqual([
      'open-recordings-folder',
      'run-diagnostics',
      'run-preflight',
      'start-recording',
      'stop-recording',
    ])
  })

  it('ignores ids handled in Rust, unknown ids and non-string payloads', () => {
    const dispatch = createTrayDispatcher({ startRecording: () => undefined })
    for (const p of ['quit', 'open-window', 'show-on-error', 'none', 'from-a-newer-build']) {
      expect(dispatch(p)).toBe(false)
    }
    expect(dispatch(undefined)).toBe(false)
    expect(dispatch(null)).toBe(false)
    expect(dispatch(42)).toBe(false)
    expect(dispatch({ action: 'start-recording' })).toBe(false)
  })

  it('reports false for an id with no handler wired', () => {
    const dispatch = createTrayDispatcher({})
    expect(dispatch('start-recording')).toBe(false)
  })

  it('swallows a throwing handler — an event callback has no catcher above it', () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const dispatch = createTrayDispatcher({
      stopRecording: () => {
        throw new Error('boom')
      },
    })
    expect(() => dispatch('stop-recording')).not.toThrow()
    expect(dispatch('stop-recording')).toBe(true)
    expect(err).toHaveBeenCalled()
    err.mockRestore()
  })
})
