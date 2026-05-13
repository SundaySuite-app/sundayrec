import Store from 'electron-store'
import type { Settings, RecordingEntry } from '../types'

const defaults: Settings = {
  language: null,

  deviceId: null,
  deviceName: null,
  deviceChannels: {},

  channels: 'stereo',
  sampleRate: 48000,
  inputVolume: 85,
  eqBass: 0,
  eqMid: 0,
  eqTreble: 0,
  compEnabled: false,
  compThreshold: -24,
  compRatio: 4,
  compAttack: 10,
  compRelease: 200,
  limiterEnabled: true,
  limiterCeiling: -1,

  format: 'mp3',
  bitrate: '192',
  filenamePattern: 'date',
  saveFolder: null,
  autoDeleteDays: 0,

  slots: [],
  specialRecordings: [],
  stopOnSilence: false,
  splitHourly: false,

  launchAtLogin: false,
  showOnStartup: false,
  minimizeToTray: true,
  wakeFromSleep: true,
  protectRecording: true,

  notifyStart: true,
  notifyStop: true,
  emailOnError: false,
  emailAddress: '',
  emailSmtp: '',
  emailSmtpPort: 587,
  emailSmtpUser: '',
  emailSmtpPass: '',

  autoUpdate: true,

  churchName: 'Alta Frikirke',
  responsiblePerson: '',

  activeRecovery: null,
  recordingHistory: []
}

const store = new Store<Settings>({
  name: 'sundayrec-settings',
  defaults
})

export function get<K extends keyof Settings>(key: K): Settings[K] {
  return store.get(key)
}

export function set<K extends keyof Settings>(key: K, value: Settings[K]): void {
  store.set(key, value)
}

export function getAll(): Settings {
  return store.store
}

export function setAll(obj: Partial<Settings>): void {
  // Never overwrite history with a stale renderer copy
  const { recordingHistory, activeRecovery, ...safe } = obj
  store.store = { ...store.store, ...safe }
}

export function getHistory(): RecordingEntry[] {
  return store.get('recordingHistory') ?? []
}

export function addHistory(entry: RecordingEntry): void {
  const history = getHistory()
  history.unshift({ ...entry, timestamp: Date.now() })
  store.set('recordingHistory', history.slice(0, 200))
}

export function deleteHistoryEntry(timestamp: number): void {
  store.set('recordingHistory', getHistory().filter(e => e.timestamp !== timestamp))
}

export function clearHistory(): void {
  store.set('recordingHistory', [])
}

export function exportProfile(): Omit<Settings, 'recordingHistory' | 'activeRecovery'> {
  const { recordingHistory, activeRecovery, hasLaunched, ...profile } = store.store
  return profile
}

export function importProfile(json: string): boolean {
  try {
    const profile = JSON.parse(json) as Partial<Settings>
    const { recordingHistory, activeRecovery, ...safe } = profile
    Object.entries(safe).forEach(([k, v]) => store.set(k as keyof Settings, v as never))
    return true
  } catch {
    return false
  }
}

export function reset(): void {
  store.clear()
}
