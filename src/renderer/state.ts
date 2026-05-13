import type { Settings } from '../types'

export let settings: Settings = {} as Settings

export function updateSettings(next: Settings): void {
  settings = next
}

export function patchSettings(patch: Partial<Settings>): void {
  settings = { ...settings, ...patch }
}
