import { t } from '../../i18n'
import { E, $ } from './state'
import { clampMain } from './geometry'
import { attachProgress, type ProgressHandle } from '../../ui/progress'

// ── Mastering panel ─────────────────────────────────────────────────────────

interface MasterPresetView {
  id: string; label: string; description: string
  targetLufs: number; targetLra: number; truePeakDb: number; filters: string
}

let masterPresets: MasterPresetView[] = []
let masterJobId   = ''
let masterPreviewPath = ''
let masterOriginalPreviewPath = ''
let masterProgressUnsubscribe: (() => void) | null = null

/**
 * The status row's bar + label, from ui/progress.ts. Attached on demand and
 * torn down when the row is hidden — the row is shared by the preview and the
 * apply, and one widget per run keeps the ETA estimator from carrying a
 * previous job's rate into the next one.
 *
 * `fraction === null` means "running, no denominator" (the sliding stripe).
 */
let masterUi: ProgressHandle | null = null

function masterStatus(fraction: number | null, label: string): void {
  const row = $('master-status-row')
  if (row) row.style.display = ''
  const host = $('master-progress-host')
  if (!masterUi && host) masterUi = attachProgress(host, { compact: true })
  masterUi?.update(fraction, label)
}

/** End the run: a full bar (or a stopped one) with the closing message. */
function masterFinish(ok: boolean, label: string): void {
  if (ok) masterUi?.done(label)
  else masterUi?.fail(label)
}

/** Drop the widget so the next run starts with a fresh rate estimate. */
function masterStatusReset(): void {
  masterUi?.destroy()
  masterUi = null
}

export async function setupMasteringPanel(): Promise<void> {
  const select       = $('master-preset-select') as HTMLSelectElement | null
  const btnPreview   = $('btn-master-preview') as HTMLButtonElement | null
  const btnListenO   = $('btn-master-listen-orig') as HTMLButtonElement | null
  const btnApply     = $('btn-master-apply') as HTMLButtonElement | null
  const btnCancel    = $('btn-master-cancel') as HTMLButtonElement | null
  const btnOpenFold  = $('btn-master-open-folder') as HTMLButtonElement | null
  const btnListenDn  = $('btn-master-listen-done') as HTMLButtonElement | null

  if (!select || !btnPreview || !btnApply) return

  // Fetch presets once. Network roundtrip is local IPC — fast.
  try { masterPresets = await window.api.masterPresets() } catch { masterPresets = [] }

  // Populate selector. Pre-select the recommended preset (speech-clear).
  select.innerHTML = ''
  for (const p of masterPresets) {
    const opt = document.createElement('option')
    opt.value = p.id
    opt.textContent = p.label
    select.appendChild(opt)
  }
  const recommended = masterPresets.find(p => p.id === 'speech-clear') ?? masterPresets[0]
  if (recommended) select.value = recommended.id
  updateMasterDesc()

  select.addEventListener('change', updateMasterDesc)

  btnPreview.addEventListener('click', () => runMasterPreview())
  btnListenO?.addEventListener('click', () => toggleListenOriginal())
  btnApply.addEventListener('click', () => runMasterApply())
  btnCancel?.addEventListener('click', () => runMasterCancel())
  btnOpenFold?.addEventListener('click', () => {
    const out = btnOpenFold.dataset.path
    if (out) window.api.revealFile(out).catch(() => {})
  })
  btnListenDn?.addEventListener('click', () => {
    const out = btnListenDn.dataset.path
    if (!out) return
    const audio = $('master-preview-audio') as HTMLAudioElement | null
    if (!audio) return
    audio.src = window.api.toAssetUrl(out)
    audio.style.display = ''
    audio.play().catch(() => {})
  })

  // Progress channel listener (set up once; outlives panel rebuilds)
  if (masterProgressUnsubscribe) { try { masterProgressUnsubscribe() } catch {} ; masterProgressUnsubscribe = null }
  const unsub = window.api.on('master-progress', (data: unknown) => {
    const { currentSec, totalSec } = data as { currentSec: number; totalSec: number }
    // `totalSec === 0` is the backend saying "I could not probe this file's
    // duration" — an unknown denominator, not 0 %. Pinning the bar at 0 for the
    // whole apply is exactly what made a working mastering run look hung; show
    // the sliding stripe instead, and give the estimator nothing rather than a
    // fraction it would turn into a confident wrong number.
    if (!(totalSec > 0)) {
      masterStatus(null, t('master.applying', 'Mastrer…'))
      return
    }
    // Capped just under 1: the file is not finished until ffmpeg exits and the
    // container is closed, so 100 % belongs to the apply's own success path.
    masterStatus(Math.min(0.99, currentSec / totalSec), t('master.applying', 'Mastrer…'))
  })
  if (typeof unsub === 'function') masterProgressUnsubscribe = unsub
}

export function updateMasterDesc(): void {
  const select = $('master-preset-select') as HTMLSelectElement | null
  const descEl = $('master-preset-desc')
  if (!select || !descEl) return
  const p = masterPresets.find(x => x.id === select.value)
  descEl.textContent = p ? p.description : ''
}

export function getSelectedPreset(): MasterPresetView | null {
  const select = $('master-preset-select') as HTMLSelectElement | null
  if (!select) return null
  return masterPresets.find(p => p.id === select.value) ?? null
}

export async function runMasterPreview(): Promise<void> {
  if (!E.filePath) return
  const preset = getSelectedPreset()
  if (!preset) return
  const btn   = $('btn-master-preview') as HTMLButtonElement | null
  const audio = $('master-preview-audio') as HTMLAudioElement | null
  const btnListenO = $('btn-master-listen-orig') as HTMLButtonElement | null

  if (btn) { btn.disabled = true; btn.textContent = t('master.applying', 'Lager forhåndsvisning…') }
  // A 15-second window renders in a second or two and reports no progress of
  // its own. The old code drew a bar at a made-up 20 % for the duration; the
  // stripe says the same thing without the invented number.
  masterStatusReset()
  masterStatus(null, t('master.applying', 'Lager forhåndsvisning…'))

  const start = Math.max(0, Math.min(E.duration > 15 ? E.duration - 15 : 0, clampMain(E.playStartSec)))
  try {
    const res = await window.api.masterPreview(E.filePath, preset.id, start, 15)
    if (!res.ok || !res.previewPath) {
      masterFinish(false, `${t('master.error', '✕ Feil')}: ${res.error ?? 'unknown'}`)
      return
    }
    masterPreviewPath = res.previewPath
    if (audio) {
      audio.src = window.api.toAssetUrl(res.previewPath)
      audio.style.display = ''
      audio.play().catch(() => {})
    }
    if (btnListenO) btnListenO.style.display = ''
    masterFinish(true, t('master.done', '✓ Forhåndsvisning klar'))
  } catch (err) {
    masterFinish(false, `${t('master.error', '✕ Feil')}: ${(err as Error).message}`)
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = t('master.preview', 'Lytt på forhåndsvisning') }
  }
}

export function toggleListenOriginal(): void {
  const audio = $('master-preview-audio') as HTMLAudioElement | null
  const btn   = $('btn-master-listen-orig') as HTMLButtonElement | null
  if (!audio || !btn) return
  if (!masterOriginalPreviewPath || audio.dataset.mode !== 'orig') {
    // Play original snippet via asset:// (WKWebView blocks file://).
    audio.src = window.api.toAssetUrl(E.filePath)
    audio.currentTime = clampMain(E.playStartSec)
    audio.dataset.mode = 'orig'
    btn.textContent = t('master.previewListenMastered', 'Lytt mastret')
    audio.style.display = ''
    audio.play().catch(() => {})
  } else if (masterPreviewPath) {
    audio.src = window.api.toAssetUrl(masterPreviewPath)
    audio.dataset.mode = 'mast'
    btn.textContent = t('master.previewListenOrig', 'Lytt original')
    audio.play().catch(() => {})
  }
}

export function deriveMasteredPath(input: string): string {
  // <dir>/<stem>_mastert.<ext>  — keep the source extension/codec format
  const lastSep   = Math.max(input.lastIndexOf('/'), input.lastIndexOf('\\'))
  const dir       = lastSep >= 0 ? input.slice(0, lastSep + 1) : ''
  const file      = lastSep >= 0 ? input.slice(lastSep + 1)    : input
  const lastDot   = file.lastIndexOf('.')
  const stem      = lastDot > 0 ? file.slice(0, lastDot) : file
  const ext       = lastDot > 0 ? file.slice(lastDot + 1).toLowerCase() : 'mp3'
  return dir + stem + '_mastert.' + ext
}

export async function runMasterApply(): Promise<void> {
  if (!E.filePath) return
  const preset = getSelectedPreset()
  if (!preset) return
  const btnApply = $('btn-master-apply')  as HTMLButtonElement | null
  const btnPrv   = $('btn-master-preview') as HTMLButtonElement | null
  const btnCancel = $('btn-master-cancel') as HTMLButtonElement | null
  const resRow = $('master-result-row')

  if (btnApply)  { btnApply.disabled  = true }
  if (btnPrv)    { btnPrv.disabled    = true }
  if (btnCancel) { btnCancel.style.display = '' }
  if (resRow)    { resRow.style.display = 'none' }
  // Pass 1 (the loudness measure) reports no percentage — it used to be drawn
  // as a 5 % bar, then a 15 % one, both invented. The stripe is the truth until
  // pass 2 starts emitting real positions. A fresh widget per apply so the
  // remaining-time estimate never inherits the previous file's rate.
  masterStatusReset()
  masterStatus(null, t('master.applying', 'Mastrer…') + ' (måler lydstyrke…)')

  masterJobId = 'm-' + Date.now() + '-' + Math.random().toString(36).slice(2, 8)
  const outPath = deriveMasteredPath(E.filePath)

  try {
    // Pass 1: measure
    const measureRes = await window.api.masterMeasure(E.filePath, preset.id)
    if (!measureRes.ok || !measureRes.measurement) {
      masterFinish(false, `${t('master.error', '✕ Feil')}: ${measureRes.error ?? 'measure_failed'}`)
      return
    }
    const beforeLufs = measureRes.measurement.inputI
    masterStatus(
      null,
      `${t('master.applying', 'Mastrer…')} (${t('master.lufsBefore', 'Original')}: ${beforeLufs.toFixed(1)} LUFS → ${preset.targetLufs} LUFS)`,
    )

    // Pass 2: apply
    const applyRes = await window.api.masterApply({
      inputPath:   E.filePath,
      outputPath:  outPath,
      presetId:    preset.id,
      measurement: measureRes.measurement,
      jobId:       masterJobId,
    })

    if (applyRes.ok && applyRes.outputPath) {
      masterFinish(
        true,
        t('master.done', '✓ Mastret') +
          ` — ${t('master.lufsBefore', 'Original')}: ${beforeLufs.toFixed(1)} LUFS → ` +
          `${t('master.lufsAfter', 'Etter')}: ${preset.targetLufs} LUFS`,
      )
      const resText = $('master-result-text')
      const fname = applyRes.outputPath.split(/[/\\]/).pop() ?? ''
      if (resText) resText.textContent = (t('master.done', '✓ Mastret')) + (fname ? ' — ' + fname : '')
      if (resRow)  resRow.style.display = ''
      const btnOpenFold = $('btn-master-open-folder') as HTMLButtonElement | null
      const btnListenDn = $('btn-master-listen-done') as HTMLButtonElement | null
      if (btnOpenFold) { btnOpenFold.style.display = ''; btnOpenFold.dataset.path = applyRes.outputPath }
      if (btnListenDn) { btnListenDn.style.display = ''; btnListenDn.dataset.path = applyRes.outputPath }
    } else {
      masterFinish(false, `${t('master.error', '✕ Feil')}: ${applyRes.error ?? 'apply_failed'}`)
    }
  } catch (err) {
    masterFinish(false, `${t('master.error', '✕ Feil')}: ${(err as Error).message}`)
  } finally {
    if (btnApply)  btnApply.disabled  = false
    if (btnPrv)    btnPrv.disabled    = false
    if (btnCancel) btnCancel.style.display = 'none'
    masterJobId = ''
  }
}

export async function runMasterCancel(): Promise<void> {
  if (!masterJobId) return
  try { await window.api.masterCancel(masterJobId) } catch {}
  masterFinish(false, t('master.cancel', 'Avbrutt'))
}
