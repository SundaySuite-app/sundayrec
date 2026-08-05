import { t } from '../i18n'
import { settings, patchSettings } from '../state'
import { setVal, setRadio } from '../helpers'
import { getAudioDevices } from '../audio/capture'
import { setupChannelGrid, startChannelGrid } from './channel-grid'
import { refreshHomeDiskSpace, loadHomeInfoStrip } from './home'
import type { ChannelMode } from '../../types'

function updateVolGradient(): void {
  const el = document.getElementById('input-volume') as HTMLInputElement | null
  if (!el) return
  const pct = +el.value
  el.style.setProperty('--vol-pct', pct + '%')
}

export function setupAudioPage(): void {
  // AUTO-SAVE is the ONLY save model on this page: every control persists on
  // change (the old Lagre/Avbryt footer contradicted it — the footer implied
  // unsaved work while the write had already happened, and Avbryt could not
  // revert it). The channel grid shows its own inline «Lagret ✓».
  const autoSave = () => { void saveAudioSettings() }

  // Sample-rate mode cards (auto / r44100 / r48000) → save.
  document.querySelectorAll<HTMLInputElement>('input[name="sampleRate"]').forEach(r => {
    r.addEventListener('change', autoSave)
  })

  // Channel-mode cards (stereo / mono / monoL / monoR) → save. The channel
  // grid listens on the same radios to re-render its chips/badges.
  document.querySelectorAll<HTMLInputElement>('input[name="channels"]').forEach(r => {
    r.addEventListener('change', autoSave)
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
    document.getElementById('opt-classic-ffmpeg')?.addEventListener('change', autoSave)
    if (/win/i.test(navigator.userAgent)) {
      const row = document.getElementById('classic-dshow-row')
      if (row) row.style.display = ''
      document.getElementById('opt-classic-dshow')?.addEventListener('change', autoSave)
    }
  }
  // NB: compressor/limiter/EQ/input-volume controls are hidden inert inputs
  // (record-raw philosophy — see saveAudioSettings); no listeners needed.

  document.getElementById('btn-audio-diagnose')?.addEventListener('click', runAudioDiagnosis)
  document.getElementById('btn-audio-diagnose-close')?.addEventListener('click', () => {
    const modal = document.getElementById('audio-diagnose-modal')
    if (modal) modal.style.display = 'none'
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
    el.textContent = hz ? ` · ${hz.toLocaleString('nb-NO')} Hz` : ''
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
  }
  const selCard = document.querySelector('#device-list .device-card.selected') as HTMLElement | null
  const subEl = selCard?.querySelector('.device-sub') as HTMLElement | null
  if (subEl) {
    const base = subEl.dataset.subBase ?? ''
    subEl.textContent = `${base} · ${count} ${t('audio.channelCount', 'kanaler')}`
  }
}

export function applyAudioSettingsToUI(): void {
  setVal('input-volume', settings.inputVolume ?? 100)
  setRadio('channels', settings.channels ?? 'stereo')
  // Sample-rate mode cards — default Auto (native capture).
  const srMode = settings.sampleRateMode ?? 'auto'
  document.querySelectorAll<HTMLInputElement>('input[name="sampleRate"]').forEach(r => {
    r.checked = r.value === srMode
  })
  updateVolGradient()
  const classicEl = document.getElementById('opt-classic-dshow') as HTMLInputElement | null
  if (classicEl) classicEl.checked = !!settings.classicDirectshow
  const classicFfEl = document.getElementById('opt-classic-ffmpeg') as HTMLInputElement | null
  if (classicFfEl) classicFfEl.checked = !!settings.classicFfmpegAudio
  const compEl = document.getElementById('opt-compressor') as HTMLInputElement | null
  if (compEl) {
    compEl.checked = !!settings.compEnabled
    const cs = document.getElementById('comp-settings')
    if (cs) cs.style.display = settings.compEnabled ? 'block' : 'none'
  }
  setVal('comp-threshold', settings.compThreshold ?? -24)
  setVal('comp-ratio',     settings.compRatio     ?? 4)
  setVal('comp-attack',    settings.compAttack    ?? 10)
  setVal('comp-release',   settings.compRelease   ?? 200)
}

async function saveAudioSettings(): Promise<void> {
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

  // NB: the compressor/limiter/EQ/input-volume fields are NOT saved here. They are
  // hidden, inert inputs (record-raw philosophy since v4.31 — dynamics/EQ live in
  // the editor, not the capture pipeline), so they keep their DEFAULT_SETTINGS
  // values. The audio-page only persists what it actually controls.
  const patch = {
    deviceId,
    deviceName,
    channels:       ((document.querySelector('input[name="channels"]:checked') as HTMLInputElement | null)?.value ?? 'stereo') as ChannelMode,
    sampleRateMode: srMode,
    classicDirectshow,
    classicFfmpegAudio,
    // Keep the numeric sampleRate in sync for client-side use (VU monitor + disk
    // estimate). Only auto / 44.1 kHz / 48 kHz exist in this picker today (no
    // 96 kHz option), so the two-way ternary below is exhaustive: auto and
    // r48000 both map to 48 kHz as a reasonable client-side estimate — the
    // recorder itself uses sampleRateMode (auto = native, no forced rate), not
    // this field. Revisit if a higher-rate mode is ever added to the UI.
    sampleRate:     srMode === 'r44100' ? 44100 : 48000,
  }

  patchSettings(patch)
  await window.api.saveSettings(settings)
  // Refresh Home live: disk estimate (channels/samplerate) + the device/format
  // info-strip cards so the change shows without navigating away and back.
  void refreshHomeDiskSpace()
  void loadHomeInfoStrip()
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
    card.addEventListener('click', () => {
      container.querySelectorAll('.device-card').forEach(c => c.classList.remove('selected'))
      card.classList.add('selected')
      patchSettings({ deviceId: devId, deviceName: name })
      // Persist immediately, then point the live channel grid at the device —
      // the grid reports the real channel count back (sub-line + auto-mono).
      void saveAudioSettings()
      void startChannelGrid(devId, name)
    })
    container.appendChild(card)
  })

  // ── Standard Web Audio devices ─────────────────────────────────────────────
  devices.forEach(d => {
    const builtIn  = /built-in|innebygd|default/i.test(d.label)
    const selected = d.deviceId === (settings.deviceId ?? 'default')
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
    card.addEventListener('click', () => {
      container.querySelectorAll('.device-card').forEach(c => c.classList.remove('selected'))
      card.classList.add('selected')
      patchSettings({ deviceId: d.deviceId, deviceName: d.label })
      void saveAudioSettings()
      void startChannelGrid(d.deviceId, d.label)
    })
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

  // Start the live channel grid for the persisted (or first) device.
  const devId = settings.deviceId ?? (devices[0]?.deviceId ?? null)
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

// Comprehensive diagnose: calls the unified backend `run_diagnostics`, which
// gathers system/devices/ffmpeg/disk/permissions/audio-engine/last-error and
// returns coded findings (SR-*) + a full markdown report. The modal shows the
// colour-coded findings on top and the raw report below, with a copy button so
// the user can paste it to support — the "fishing" the diagnose tool is for.
async function runAudioDiagnosis(): Promise<void> {
  const btn = document.getElementById('btn-audio-diagnose') as HTMLButtonElement | null
  if (btn) { btn.disabled = true; btn.textContent = t('audio.diagnoseRunning', 'Analyserer…') }

  try {
    const report = await window.api.runDiagnostics()

    const modal = document.getElementById('audio-diagnose-modal')
    const body  = document.getElementById('audio-diagnose-body')
    if (!modal || !body) return

    const badge = (sev: string): string =>
      sev === 'critical' ? '🔴' : sev === 'warning' ? '⚠️' : sev === 'info' ? 'ℹ️' : '✅'

    const findingsHtml = (report.findings ?? [])
      .map(f => `
        <div class="diag-finding diag-${escHtml(f.severity)}">
          <div class="diag-finding-head">${badge(f.severity)} <code>${escHtml(f.code)}</code> — <strong>${escHtml(f.title)}</strong></div>
          ${f.detail ? `<div class="diag-finding-detail">${escHtml(f.detail)}</div>` : ''}
          ${f.hint ? `<div class="diag-finding-hint">👉 ${escHtml(f.hint)}</div>` : ''}
        </div>`)
      .join('')

    const savedLine = report.savedTo
      ? `<div class="diag-saved">${t('audio.diagnoseSaved', 'Lagret til')}: <code>${escHtml(report.savedTo)}</code></div>`
      : ''

    body.innerHTML = `
      <div class="diag-findings">${findingsHtml}</div>
      <button type="button" class="btn-secondary" id="btn-diagnose-copy" style="margin:8px 0">${t('audio.diagnoseCopy', '📋 Kopier full rapport')}</button>
      ${savedLine}
      <details style="margin-top:8px"><summary>${t('audio.diagnoseFull', 'Full rapport')}</summary><pre class="diag-report">${escHtml(report.markdown)}</pre></details>`

    document.getElementById('btn-diagnose-copy')?.addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(report.markdown)
        const b = document.getElementById('btn-diagnose-copy')
        if (b) b.textContent = t('audio.diagnoseCopied', '✓ Kopiert')
      } catch { /* clipboard blocked — the report is still visible to copy by hand */ }
    })

    modal.style.display = 'flex'
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = t('audio.diagnose', 'Diagnose') }
  }
}

function escHtml(str: unknown): string {
  return String(str ?? '').replace(/[&<>"']/g, m =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m] ?? m))
}
