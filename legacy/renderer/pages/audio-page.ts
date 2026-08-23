import { t, tf, onLocaleApplied } from '../i18n'
import { settings, patchSettings } from '../state'
import { setVal, setRadio, localeTag } from '../helpers'
import { getAudioDevices, isBuiltInDevice } from '../audio/capture'
import { setupChannelGrid, startChannelGrid } from './channel-grid'
import { refreshHomeDiskSpace, loadHomeInfoStrip } from './home'
import { reconcilePreroll } from '../preroll-lifecycle'
import { closeModal, openModal } from '../ui/modal-manager'
import {
  bindRadioGroup,
  bindSetting,
  confirmIfRecordingImminent,
  recordingImminentGuard,
  resyncBoundSettings,
  showSavedChip,
} from '../ui/bind-setting'
import type { ChannelMode } from '../../types'

export function setupAudioPage(): void {
  // AUTO-APPLY is the ONLY save model on this page — and since bindSetting it
  // is no longer SILENT: each control writes on change and flashes an inline
  // «Lagret ✓», the same receipt the channel grid has always shown. (The old
  // Lagre/Avbryt footer contradicted the write that had already happened, and
  // «Avbryt» could not revert it.)

  // Sample-rate mode cards (auto / r44100 / r48000).
  bindRadioGroup('sampleRate', {
    key: 'sampleRateMode',
    apply: () => collectAudioSettings(),
    after: () => afterAudioSave(),
  })

  // Channel-mode cards (stereo / mono / monoL / monoR). The channel grid
  // listens on the same radios to re-render its chips/badges.
  bindRadioGroup('channels', {
    key: 'channels',
    apply: () => collectAudioSettings(),
    after: () => afterAudioSave(),
  })

  // The live channel grid: meters per native channel, tap-to-assign L/R. The
  // grid reports the authoritative channel count back → device sub-line +
  // the mono auto-switch for 1-channel devices.
  setupChannelGrid(onGridChannelCount)

  // Show the actual rate «Automatisk» resolves to (the hardware's native rate).
  void showAutoSampleRate()

  // Advanced audio-engine escape hatches. The ffmpeg hatch (native capture →
  // legacy ffmpeg) applies on every platform; the DirectShow row is the older
  // Windows-only hatch and stays hidden on macOS (no DirectShow there).
  {
    const card = document.getElementById('classic-audio-card')
    if (card) card.style.display = ''
    // Swapping the capture engine mid-service is exactly the change that costs
    // you the recording, so it asks first when one is running or imminent.
    bindSetting('opt-classic-ffmpeg', {
      key: 'classicFfmpegAudio',
      apply: () => collectAudioSettings(),
      confirmIf: recordingImminentGuard(t('audio.guardEngine', 'Bytte opptaksmotor')),
      after: () => afterAudioSave(),
    })
    if (/win/i.test(navigator.userAgent)) {
      const row = document.getElementById('classic-dshow-row')
      if (row) row.style.display = ''
      bindSetting('opt-classic-dshow', {
        key: 'classicDirectshow',
        apply: () => collectAudioSettings(),
        confirmIf: recordingImminentGuard(t('audio.guardEngine', 'Bytte opptaksmotor')),
        after: () => afterAudioSave(),
      })
    }
  }
  // NB: compressor/limiter/EQ/input-volume have NO controls on this page
  // (record-raw philosophy — see saveAudioSettings); their settings-values
  // pass through the blob untouched.

  document.getElementById('btn-audio-diagnose')?.addEventListener('click', runAudioDiagnosis)
  document.getElementById('btn-audio-diagnose-close')?.addEventListener('click', () => {
    closeModal('audio-diagnose-modal')
  })
}

/** Fill the «Automatisk» card with the actual sample rate it will use — the
 *  audio hardware's native rate (what the capture engine records at with no
 *  forced rate). Detected via a throwaway AudioContext, whose `sampleRate` is
 *  the system audio rate (on the Mac built-in mic this matches the capture
 *  rate, e.g. 48 000 Hz). */
async function showAutoSampleRate(): Promise<void> {
  const el = document.getElementById('sr-auto-actual')
  if (!el) return
  try {
    const Ctx =
      window.AudioContext ||
      (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext
    const ctx = new Ctx()
    const hz = ctx.sampleRate
    void ctx.close()
    el.textContent = hz ? ` · ${hz.toLocaleString(localeTag())} Hz` : ''
  } catch {
    el.textContent = ''
  }
}

/** The channel grid's authoritative count → device-card sub-line + the mono
 *  auto-switch. A 1-channel device (the Mac built-in mic, most USB lavaliers)
 *  can't produce stereo — recording it as stereo gives a dead right channel,
 *  so switch to MonoL. Only ever auto-set mono — never auto-revert a
 *  multichannel device, so a real stereo interface keeps the user's choice. */
function onGridChannelCount(count: number): void {
  if (count === 1 && settings.channels !== 'monoL') {
    setRadio('channels', 'monoL')
    void saveAudioSettings()
    resyncBoundSettings()
  }
  const selCard = document.querySelector('#device-list .device-card.selected') as HTMLElement | null
  const subEl = selCard?.querySelector('.device-sub') as HTMLElement | null
  if (subEl) {
    const base = subEl.dataset.subBase ?? ''
    subEl.textContent = `${base} · ${count} ${t('audio.channelCount', 'kanaler')}`
  }
}

export function applyAudioSettingsToUI(): void {
  setRadio('channels', settings.channels ?? 'stereo')
  // Sample-rate mode cards — default Auto (native capture).
  const srMode = settings.sampleRateMode ?? 'auto'
  document.querySelectorAll<HTMLInputElement>('input[name="sampleRate"]').forEach(r => {
    r.checked = r.value === srMode
  })
  const classicEl = document.getElementById('opt-classic-dshow') as HTMLInputElement | null
  if (classicEl) classicEl.checked = !!settings.classicDirectshow
  const classicFfEl = document.getElementById('opt-classic-ffmpeg') as HTMLInputElement | null
  if (classicFfEl) classicFfEl.checked = !!settings.classicFfmpegAudio
  // (The compressor/limiter/EQ/inputVolume fields — record-raw philosophy since
  // v4.31, never read by the recorder — left the settings model in v0.15.)
  // The DOM was just rewritten from settings — rebase every binding's "last
  // committed value" so the next edit is compared against what is on screen.
  resyncBoundSettings()
}

/** Refresh Home live: the disk estimate (channels/samplerate) + the device and
 *  format info-strip cards, so a change shows without navigating away. */
function afterAudioSave(): void {
  void refreshHomeDiskSpace()
  void loadHomeInfoStrip()
}

/**
 * Read every audio control the page owns into `settings`. Split out of the old
 * `saveAudioSettings` so `bindSetting` owns the persistence (one debounced
 * write, one visible receipt) while the DOM → Settings mapping stays in one
 * place.
 */
function collectAudioSettings(): void {
  const selectedCard = document.querySelector('.device-card.selected') as HTMLElement | null
  const deviceId   = selectedCard?.dataset.deviceId   ?? settings.deviceId   ?? null
  const deviceName = selectedCard?.dataset.deviceLabel ?? settings.deviceName ?? null

  // Channel L/R mapping is NOT read from the DOM here: only the channel grid's
  // explicit tap handler writes `settings.deviceChannels` (channel-grid.ts).
  // This is the structural replacement for the old `pickerActive` guard — a
  // hidden/empty picker once wrote a phantom L=0/R=0 mapping that recorded a
  // near-silent channel on the Qu-5 (2026-07-31).

  const srMode = ((document.querySelector('input[name="sampleRate"]:checked') as HTMLInputElement | null)
    ?.value ?? 'auto') as 'auto' | 'r44100' | 'r48000'
  const classicDirectshow = !!(document.getElementById('opt-classic-dshow') as HTMLInputElement | null)?.checked
  const classicFfmpegAudio = !!(document.getElementById('opt-classic-ffmpeg') as HTMLInputElement | null)?.checked

  const patch = {
    deviceId,
    deviceName,
    channels:       ((document.querySelector('input[name="channels"]:checked') as HTMLInputElement | null)?.value ?? 'stereo') as ChannelMode,
    sampleRateMode: srMode,
    classicDirectshow,
    classicFfmpegAudio,
  }

  patchSettings(patch)
}

/** Collect + persist in one step, for the paths that are not bound controls
 *  (the device cards, the grid's mono auto-switch). */
async function saveAudioSettings(): Promise<void> {
  collectAudioSettings()
  await window.api.saveSettings(settings)
  afterAudioSave()
}

/**
 * Switch the recording device.
 *
 * The device cards are clickable divs, not a form control, so they cannot go
 * through `bindSetting` — but they get the same guard: swapping the input 4
 * minutes before the service starts is the single change most likely to cost
 * you the recording, so it asks first (and only then).
 */
async function selectDevice(
  container: HTMLElement,
  card: HTMLElement,
  deviceId: string,
  deviceName: string | null,
): Promise<void> {
  if (settings.deviceId === deviceId) return
  const proceed = await confirmIfRecordingImminent(t('audio.guardDevice', 'Bytte lydenhet'))
  if (!proceed) return
  container.querySelectorAll('.device-card').forEach(c => c.classList.remove('selected'))
  card.classList.add('selected')
  patchSettings({ deviceId, deviceName })
  // Persist immediately, then point the live channel grid at the device — the
  // grid reports the real channel count back (sub-line + auto-mono).
  await saveAudioSettings()
  showSavedChip(card.querySelector<HTMLElement>('.device-name'))
  // The rolling pre-roll buffer addresses the device by name — re-point it at
  // the new one (or take it down if the new device can't be resolved). Done
  // BEFORE the channel grid reopens the device, so the two never race for it.
  await reconcilePreroll(true)
  void startChannelGrid(deviceId, deviceName)
}

export async function renderDeviceList(containerId: string): Promise<void> {
  const container = document.getElementById(containerId)
  if (!container) return

  const [devices, asioDrivers] = await Promise.all([
    getAudioDevices(),
    window.api.listAsioDrivers().catch(() => [] as string[])
  ])

  container.innerHTML = ''
  if (!devices.length && !asioDrivers.length) {
    container.innerHTML = `<div style="color:var(--text3);font-size:13px;padding:8px 0">${t('audio.noDevices')}</div>`
    return
  }

  // ── ASIO devices (Windows pro audio) ───────────────────────────────────────
  // An ASIO interface shows up as ONE device exposing all its channels (the
  // dshow path splits it into stereo pairs). These come first — the preferred,
  // low-latency, multichannel path. The backend addresses the device by its raw
  // name (`deviceName`); the `asio::`-prefixed `deviceId` is the UI/key handle.
  asioDrivers.forEach(name => {
    const devId    = `asio::${name}`
    const selected = settings.deviceId === devId
    const card     = document.createElement('div')
    card.className           = 'device-card' + (selected ? ' selected' : '')
    card.dataset.deviceId    = devId
    card.dataset.deviceLabel = name
    card.innerHTML = `
      <div class="device-icon">🎛</div>
      <div>
        <div class="device-name">${escHtml(name)}</div>
        <div class="device-sub" data-sub-base="ASIO">ASIO</div>
      </div>
      <span class="device-badge ok">ASIO</span>`
    card.addEventListener('click', () => { void selectDevice(container, card, devId, name) })
    container.appendChild(card)
  })

  // ── Host devices (CoreAudio / WASAPI, from the backend enumeration) ────────
  devices.forEach(d => {
    const builtIn  = isBuiltInDevice(d.label)
    // No stored pick ⇒ the host default is what a recording would use, so it is
    // what the card should show as selected. (There is no `deviceId: 'default'`
    // pseudo-entry any more — the backend list marks the real device instead.)
    const selected = settings.deviceId ? d.deviceId === settings.deviceId : d.isDefault
    const card     = document.createElement('div')
    card.className            = 'device-card' + (selected ? ' selected' : '')
    card.dataset.deviceId     = d.deviceId
    card.dataset.deviceLabel  = d.label
    const subBase = builtIn ? t('audio.internal','Innebygd') : t('audio.deviceExternal', 'USB / Ekstern')
    card.innerHTML = `
      <div class="device-icon">${builtIn ? '🎙' : '🎛'}</div>
      <div>
        <div class="device-name">${escHtml(d.label || t('audio.deviceUnknown', 'Ukjent enhet'))}</div>
        <div class="device-sub" data-sub-base="${escHtml(subBase)}">${escHtml(subBase)}</div>
      </div>
      <span class="device-badge ${builtIn ? 'warn' : 'ok'}">${builtIn ? t('audio.notRecommended') : t('audio.connected','Tilkoblet ✓')}</span>`
    card.addEventListener('click', () => { void selectDevice(container, card, d.deviceId, d.label) })
    container.appendChild(card)
  })

  // After rendering device cards, check ffmpeg device availability
  window.api.listFfmpegAudioDevices?.().then((ffmpegDevices) => {
    const stored = settings.deviceName
    if (!stored || !ffmpegDevices) return
    const found = ffmpegDevices.some(d =>
      d.name.toLowerCase().includes(stored.toLowerCase().slice(0, 8)) ||
      stored.toLowerCase().includes(d.name.toLowerCase().slice(0, 8))
    )
    const warn = document.getElementById('device-ffmpeg-warn')
    if (warn) warn.style.display = found ? 'none' : ''
  }).catch(() => {})

  // Start the live channel grid for the persisted (else the host default) device.
  const fallback = devices.find(d => d.isDefault) ?? devices[0]
  const devId = settings.deviceId ?? (fallback?.deviceId ?? null)
  if (devId) {
    const label = devId.startsWith('asio::')
      ? devId.slice('asio::'.length)
      : (settings.deviceName ?? devices.find(d => d.deviceId === devId)?.label ?? null)
    void startChannelGrid(devId, label)
  }
}

/** Human-readable text from any thrown value (Tauri command errors arrive as
 *  serialized objects — String(err) shows "[object Object]"). */
export function errText(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  if (err && typeof err === 'object') {
    const o = err as Record<string, unknown>
    for (const k of ['message', 'recording', 'validation', 'internal']) {
      if (typeof o[k] === 'string') return o[k] as string
    }
    try { return JSON.stringify(err) } catch { /* fall through */ }
  }
  return String(err)
}

/** One structured row of the audio-diagnosis modal. */
function diagRow(label: string, value: string, ok: boolean | null): string {
  const mark = ok === null ? '·' : ok ? '✓' : '✕'
  const cls = ok === null ? 'diag-row-neutral' : ok ? 'diag-row-ok' : 'diag-row-bad'
  return `<div class="diag-row ${cls}"><span class="diag-row-mark">${mark}</span>` +
    `<span class="diag-row-label">${escHtml(label)}</span>` +
    `<span class="diag-row-value">${escHtml(value)}</span></div>`
}

/** One command's worth of remembered IPC failures, folded down to what the
 *  collapsed «Siste IPC-feil» section shows. */
interface IpcFailureGroup {
  cmd: string
  count: number
  lastError: string
  lastAt: number
}

/**
 * Fold the renderer's flat, newest-first failure ring (E2.4's `ipc-failures-
 * core`) down to one row per distinct command. `failures` is already newest
 * first, so the FIRST occurrence of a command is its most recent failure —
 * exactly what `lastError`/`lastAt` should hold.
 */
function groupIpcFailures(
  failures: { cmd: string; error: string; at: number }[],
): IpcFailureGroup[] {
  const byCmd = new Map<string, IpcFailureGroup>()
  for (const f of failures) {
    const g = byCmd.get(f.cmd)
    if (g) g.count += 1
    else byCmd.set(f.cmd, { cmd: f.cmd, count: 1, lastError: f.error, lastAt: f.at })
  }
  return [...byCmd.values()].sort((a, b) => b.lastAt - a.lastAt)
}

/** «3 min siden» — coarse, unit-abbreviated relative time. The abbreviation
 *  (not a full word) sidesteps per-language plural grammar entirely, which
 *  matters here because this ring can span a nine-hour Sunday. */
function relativeAgo(atMs: number, nowMs: number): string {
  const diffSec = Math.max(0, Math.round((nowMs - atMs) / 1000))
  if (diffSec < 5) return t('audio.diagIpcJustNow', 'akkurat nå')
  if (diffSec < 60) return tf('audio.diagIpcSecAgo', { n: diffSec }, '{n} sek siden')
  const diffMin = Math.round(diffSec / 60)
  if (diffMin < 60) return tf('audio.diagIpcMinAgo', { n: diffMin }, '{n} min siden')
  const diffHour = Math.round(diffMin / 60)
  return tf('audio.diagIpcHourAgo', { n: diffHour }, '{n} t siden')
}

function ipcFailureRow(g: IpcFailureGroup, nowMs: number): string {
  const label = g.count > 1 ? `${g.cmd} ×${g.count}` : g.cmd
  return `<div class="diag-row diag-row-bad"><span class="diag-row-mark">✕</span>` +
    `<span class="diag-row-label">${escHtml(label)}</span>` +
    `<span class="diag-row-value">${escHtml(g.lastError)} · ${escHtml(relativeAgo(g.lastAt, nowMs))}</span></div>`
}

/**
 * The diagnose modal's «Siste IPC-feil» disclosure (E2.7).
 *
 * `call()` in api-shim.ts already remembers every failed backend call in a
 * bounded, renderer-local ring (E2.4) — this is the first surface that reads
 * it back. Kept collapsed and after the audio answer on purpose: it is a
 * support detail for a broken BACKEND, not the headline "is my mic OK?"
 * question the modal leads with. Empty ring → empty string, so the panel
 * never shows a reassuring "no failures" box nobody asked to see.
 */
function renderIpcFailuresSection(): string {
  const summary = window.api.getIpcFailureSummary()
  if (!summary.count) return ''
  const groups = groupIpcFailures(window.api.getRecentIpcFailures())
  const now = Date.now()
  const rows = groups.map(g => ipcFailureRow(g, now)).join('')
  return `<details class="diag-details"><summary>${escHtml(t('audio.diagIpcTitle', 'Siste IPC-feil (teknisk)'))} (${summary.count})</summary>` +
    `<div class="diag-rows">${rows}</div></details>`
}

/**
 * The Lyd tab's "Diagnose" button.
 *
 * It used to call the generic whole-system `run_diagnostics` and dump its
 * markdown into a `<pre>` — the answer to "is my microphone OK?" delivered as a
 * wall of text about disks, updates and last errors. The purpose-built
 * `diagnose_audio` command (one enumeration → the audio-input names the panel
 * actually asks about) had no caller at all.
 *
 * Now the modal leads with the audio answer, rendered as rows: which devices
 * ffmpeg can see, whether the CONFIGURED device is among them, the microphone
 * permission, and the ffmpeg sidecar. The full markdown report is still there —
 * behind a disclosure, with the copy button — because that is what support asks
 * for, not what the user came to read.
 */
async function runAudioDiagnosis(): Promise<void> {
  const btn = document.getElementById('btn-audio-diagnose') as HTMLButtonElement | null
  diagnoseRunning = true
  if (btn) { btn.disabled = true; btn.textContent = t('audio.diagnoseRunning', 'Analyserer…') }

  try {
    const [audio, permissions, ffmpeg, report] = await Promise.all([
      window.api.diagnoseAudio?.() ?? Promise.resolve(null),
      window.api.mediaPermissions?.().catch(() => null) ?? Promise.resolve(null),
      window.api.ffmpegHealth?.().catch(() => null) ?? Promise.resolve(null),
      window.api.runDiagnostics(),
    ])

    const body = document.getElementById('audio-diagnose-body')
    if (!body) return

    const inputs = audio?.dshow ?? []
    const stored = settings.deviceName ?? null
    // The device is addressed by a fuzzy name match in the recorder, so compare
    // the same way rather than demanding an exact string.
    const needle = stored?.toLowerCase().slice(0, 8) ?? ''
    const storedFound = !stored || inputs.some(n =>
      n.toLowerCase().includes(needle) || stored.toLowerCase().includes(n.toLowerCase().slice(0, 8)))

    const mic = permissions?.microphone
    const micOk = mic === undefined || mic === 'unknown' ? null : !(mic === 'denied' || mic === 'restricted')
    const micText = mic === 'authorized' ? t('health.granted', 'Gitt')
      : mic === 'denied' ? t('health.denied', 'Avslått — åpne Systeminnstillinger → Personvern og sikkerhet → Mikrofon')
      : mic === 'restricted' ? t('health.restricted', 'Sperret av systemadministrator')
      : mic === 'notDetermined' ? t('health.notAsked', 'Ikke spurt ennå — første opptak utløser spørsmålet')
      : t('health.cannotTell', 'Kan ikke avgjøres på denne plattformen')

    const rows = [
      diagRow(
        t('audio.diagDevicesFound', 'Lydenheter funnet'),
        String(inputs.length),
        inputs.length > 0,
      ),
      stored
        ? diagRow(
            t('audio.diagStoredDevice', 'Valgt enhet'),
            storedFound ? stored : `${stored} — ${t('audio.diagStoredMissing', 'ikke funnet')}`,
            storedFound,
          )
        : diagRow(t('audio.diagStoredDevice', 'Valgt enhet'), t('audio.diagNoStored', 'Standardenhet'), null),
      diagRow(t('audio.diagMicPermission', 'Mikrofontilgang'), micText, micOk),
      diagRow(
        t('audio.diagFfmpeg', 'Lydmotor (ffmpeg)'),
        ffmpeg?.available === false
          ? t('audio.diagFfmpegMissing', 'Ikke funnet')
          : (ffmpeg?.version ?? t('audio.diagFfmpegOk', 'Tilgjengelig')),
        ffmpeg ? ffmpeg.available : null,
      ),
      audio?.wasapiAvailable
        ? diagRow(t('audio.diagLoopback', 'WASAPI-loopback'), String(audio.wasapi.length), true)
        : '',
    ].join('')

    const deviceList = inputs.length
      ? `<ul class="diag-device-list">${inputs.map(n => `<li>${escHtml(n)}</li>`).join('')}</ul>`
      : `<div class="diag-row diag-row-bad"><span class="diag-row-mark">✕</span><span class="diag-row-label">${escHtml(t('audio.noDevices', 'Ingen lydenheter funnet'))}</span></div>`

    const savedLine = report.savedTo
      ? `<div class="diag-saved">${t('audio.diagnoseSaved', 'Lagret til')}: <code>${escHtml(report.savedTo)}</code></div>`
      : ''

    body.innerHTML = `
      <div class="diag-rows">${rows}</div>
      <details class="diag-details"><summary>${escHtml(t('audio.diagDeviceListTitle', 'Alle lydenheter opptakeren ser'))}</summary>${deviceList}</details>
      <button type="button" class="btn-secondary" id="btn-diagnose-copy" style="margin:8px 0">${t('audio.diagnoseCopy', '📋 Kopier full rapport')}</button>
      ${savedLine}
      ${renderIpcFailuresSection()}
      <details class="diag-details"><summary>${escHtml(t('audio.diagnoseFull', 'Full systemrapport'))}</summary><pre class="diag-report">${escHtml(report.markdown)}</pre></details>`

    document.getElementById('btn-diagnose-copy')?.addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(report.markdown)
        const b = document.getElementById('btn-diagnose-copy')
        if (b) b.textContent = t('audio.diagnoseCopied', '✓ Kopiert')
      } catch { /* clipboard blocked — the report is still visible to copy by hand */ }
    })

    openModal('audio-diagnose-modal')
  } finally {
    diagnoseRunning = false
    if (btn) { btn.disabled = false; btn.textContent = t('audio.diagnose', 'Diagnose') }
  }
}

/** Språkbytte mid-analyse: keep the transient label truthful (the data-i18n
 *  pass resets it to the idle default while the analysis is still running). */
let diagnoseRunning = false
onLocaleApplied(() => {
  if (!diagnoseRunning) return
  const btn = document.getElementById('btn-audio-diagnose') as HTMLButtonElement | null
  if (btn) btn.textContent = t('audio.diagnoseRunning', 'Analyserer…')
})

function escHtml(str: unknown): string {
  return String(str ?? '').replace(/[&<>"']/g, m =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m] ?? m))
}
