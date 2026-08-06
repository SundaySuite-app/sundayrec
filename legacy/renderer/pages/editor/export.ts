import { t } from '../../i18n'
import { settings } from '../../state'
import { E, $, clearDirty } from './state'
import { closeModal, openModal } from '../../ui/modal-manager'
import { clearEditorDraft } from './cuts'
import { saveMetadata } from './metadata'
import { renderMixer, loadPresetIntoMixer, mixerProcessing } from './mixer'
import {
  buildExportRequest,
  exportLevelSummary,
  EXPORT_PHASE_MEASURING,
} from './export-params'
import { toast } from '../../ui/toast'
import { attachProgress } from '../../ui/progress'

// ── Export + publish flow ───────────────────────────────────────────────────

export function openExportModal(): void {
  if (!E.filePath) return

  // For a video file the user can either keep the video (re-encode) or extract
  // the audio track only to a normal audio format. The "Eksporttype" toggle is
  // shown only for video; audio files always use the audio format picker.
  const typeSection = $('export-type-section')
  if (typeSection) typeSection.style.display = E.isVideoFile ? '' : 'none'
  applyExportSides()

  renderLevelSummary()
  const ioRow     = $('export-io-row')
  const ioSummary = $('export-io-summary')
  if (ioRow && ioSummary) {
    const parts = []
    if (!E.isVideoFile) {
      if (E.includeIntroOutro && settings.editorIntroPath) {
        parts.push('Intro: ' + (settings.editorIntroPath.split(/[/\\]/).pop() ?? ''))
      }
      if (E.includeIntroOutro && settings.editorOutroPath) {
        parts.push('Outro: ' + (settings.editorOutroPath.split(/[/\\]/).pop() ?? ''))
      }
    } else if (!E.videoExportAudioOnly && (E.videoIntroPath || E.videoOutroPath)) {
      // A VIDEO export drops the jingles in the seam — the modal used to list
      // them anyway, promising something the finished mp4 never contained.
      // (Audio-only extract from a video drops them too, and says nothing:
      // there is no video-jingle expectation to correct there.)
      parts.push(t('editor.jinglesVideoUnsupported', 'Jingler støttes ikke for video ennå'))
    }
    ioSummary.textContent = parts.length ? parts.join(' · ') : ''
    ioRow.style.display   = parts.length ? '' : 'none'
  }
  // Audio-enhancement section (channel repair + vocal chain + one-click auto)
  setupEnhanceSection()

  // Render publishing section
  void renderPublishOptions()

  openModal('editor-export-modal')
}

/**
 * Paint the export modal's LEVEL row from the current state.
 *
 * With a mastering preset active the backend skips the peak-normalize gain
 * entirely (loudnorm owns the delivery level), so claiming "Normalisert
 * (+x dB)" there would be a promise the export doesn't keep. The decision
 * itself is the pure `exportLevelSummary`; this only localises it. Re-runnable:
 * one-click auto-enhance changes the preset while the modal is open.
 */
function renderLevelSummary(): void {
  const procRow = $('export-proc-row')
  const summary = $('export-proc-summary')
  if (!procRow || !summary) return
  const level = exportLevelSummary(E.masterPreset, E.audioGainDb)
  if (level.kind === 'masterOwnsLevel') {
    summary.textContent = t('editor.volumeByMastering', 'Volum styres av mastring')
    procRow.style.display = ''
  } else if (level.kind === 'normalized') {
    const sign = level.gainDb >= 0 ? '+' : ''
    summary.textContent = `${t('editor.normalizeApplied', 'Normalisert')} (${sign}${level.gainDb.toFixed(1)} dB → -1 dBFS)`
    procRow.style.display = ''
  } else {
    procRow.style.display = 'none'
  }
}

/** Sync the export modal's audio-vs-video sides to `E.isVideoFile` +
 *  `E.videoExportAudioOnly`. Safe to call repeatedly (on open + on toggle). */
function applyExportSides(): void {
  // Type pills reflect current state.
  document.querySelectorAll<HTMLButtonElement>('.export-type-btn').forEach((b) => {
    b.classList.toggle('active', (b.dataset.type === 'audio') === E.videoExportAudioOnly)
  })

  const showAudioSide = !E.isVideoFile || E.videoExportAudioOnly
  const fmtSection  = $('export-fmt-section')
  const videoNotice = $('export-video-notice')
  if (fmtSection)  fmtSection.style.display  = showAudioSide ? '' : 'none'
  if (videoNotice) videoNotice.style.display = showAudioSide ? 'none' : ''

  if (showAudioSide) {
    const activeFmt = document.querySelector<HTMLElement>('#export-fmt-section .export-fmt-btn.active')?.dataset.fmt ?? 'mp3'
    updateExportFormatUI(activeFmt)
  } else {
    ;['export-mp3-opts', 'export-wav-opts', 'export-aac-opts'].forEach((id) => {
      const el = $(id); if (el) el.style.display = 'none'
    })
  }
}

let enhanceWired = false

/** Show + wire the "Lydforbedring" section in the export modal. Syncs the
 *  selects from E, wires one-click auto, per-control changes, and the channel
 *  diagnose button. Listeners are attached once; values re-sync on every open. */
function setupEnhanceSection(): void {
  const section = $('export-enhance-section')
  if (!section) return
  section.style.display = ''

  const vocalSel  = $('enhance-vocal-chain')    as HTMLSelectElement | null
  const chanSel   = $('enhance-channel-repair') as HTMLSelectElement | null
  const masterSel = $('enhance-master-preset')  as HTMLSelectElement | null
  const summary   = $('enhance-summary')
  const diagLine  = $('enhance-channel-diag')

  // Sync current state into the controls.
  if (vocalSel) vocalSel.value = E.vocalChainPreset
  if (chanSel)  chanSel.value  = E.channelRepairMode === 'gainDb' ? '' : E.channelRepairMode
  if (masterSel) masterSel.value = E.masterPreset
  const mixerToggleSync = $('opt-use-mixer') as HTMLInputElement | null
  const mixerControlsSync = $('mixer-controls')
  if (mixerToggleSync) mixerToggleSync.checked = E.useMixer
  if (mixerControlsSync) {
    mixerControlsSync.style.display = E.useMixer ? '' : 'none'
    if (E.useMixer) renderMixer(mixerControlsSync)
  }

  // Video format/codec pickers (active-class toggles like the audio format row).
  const vfmtBtns = document.querySelectorAll<HTMLButtonElement>('.export-vfmt-btn')
  vfmtBtns.forEach((b) => {
    b.classList.toggle('active', b.dataset.vfmt === E.videoFormat)
  })
  const vcodecBtns = document.querySelectorAll<HTMLButtonElement>('.export-vcodec-btn')
  vcodecBtns.forEach((b) => {
    b.classList.toggle('active', b.dataset.vcodec === E.videoCodec)
  })

  if (enhanceWired) return
  enhanceWired = true

  // Export-type pills (video files only): keep video vs. extract audio only.
  const typeBtns = document.querySelectorAll<HTMLButtonElement>('.export-type-btn')
  typeBtns.forEach((b) => {
    b.addEventListener('click', () => {
      E.videoExportAudioOnly = b.dataset.type === 'audio'
      applyExportSides()
    })
  })

  vfmtBtns.forEach((b) => {
    b.addEventListener('click', () => {
      E.videoFormat = b.dataset.vfmt || 'mp4'
      vfmtBtns.forEach((x) => x.classList.toggle('active', x === b))
    })
  })
  vcodecBtns.forEach((b) => {
    b.addEventListener('click', () => {
      E.videoCodec = b.dataset.vcodec || 'h264'
      vcodecBtns.forEach((x) => x.classList.toggle('active', x === b))
    })
  })

  // Advanced mixer: toggle visibility + render; preset selection loads into it.
  const mixerToggle = $('opt-use-mixer') as HTMLInputElement | null
  const mixerControls = $('mixer-controls')
  const syncMixerVisibility = () => {
    if (mixerControls) mixerControls.style.display = E.useMixer ? '' : 'none'
  }
  mixerToggle?.addEventListener('change', () => {
    E.useMixer = mixerToggle.checked
    syncMixerVisibility()
    if (E.useMixer && mixerControls) renderMixer(mixerControls)
  })

  vocalSel?.addEventListener('change', () => {
    E.vocalChainPreset = vocalSel.value
    // Keep the mixer in sync when a preset is picked, so opening it shows the
    // preset's settings (the mixer is the editable form of the same chain).
    if (vocalSel.value && mixerControls) loadPresetIntoMixer(vocalSel.value, mixerControls)
  })
  chanSel?.addEventListener('change', () => {
    E.channelRepairMode = chanSel.value
    // Picking an explicit mode clears any auto-balance gains.
    E.channelRepairLeftDb = 0
    E.channelRepairRightDb = 0
  })
  // Mastering is an EXPLICIT choice, never an automatic one (auto-enhance
  // recommends the vocal chain only — stacking both double-processes). Picking
  // one hands the output level to loudnorm, which the level row must reflect.
  masterSel?.addEventListener('change', () => {
    E.masterPreset = masterSel.value
    renderLevelSummary()
  })

  // One-click: analyse + apply the recommended best-result bundle.
  $('btn-auto-enhance')?.addEventListener('click', async () => {
    if (!E.filePath) return
    const btn = $('btn-auto-enhance') as HTMLButtonElement | null
    if (btn) { btn.disabled = true; btn.textContent = t('editor.autoEnhancing', '✨ Analyserer…') }
    const res = (await window.api.editorAutoProcess(E.filePath)) as {
      diagnosis: { recommended: { mode: string; leftDb: number; rightDb: number }; code: string }
      vocalChainPreset: string
      masterPreset: string
      summary: string
    } | null
    if (btn) { btn.disabled = false; btn.textContent = t('editor.autoEnhance', '✨ Automatisk lydforbedring (ett klikk)') }
    if (!res) {
      if (summary) { summary.textContent = t('editor.autoEnhanceFail', 'Kunne ikke analysere lyden (krever editor-bygg).'); (summary as HTMLElement).style.display = '' }
      return
    }
    // Apply the recommendation to export state + controls. `masterPreset` is
    // EMPTY from auto-process on purpose (the vocal chain alone — stacking a
    // mastering chain on it double-processes); '' flows through
    // `buildExportRequest`'s orUndefined as "no mastering".
    E.vocalChainPreset = res.vocalChainPreset
    E.masterPreset     = res.masterPreset
    const rec = res.diagnosis.recommended
    E.channelRepairMode    = rec.mode === 'none' ? '' : rec.mode
    E.channelRepairLeftDb  = rec.leftDb
    E.channelRepairRightDb = rec.rightDb
    if (vocalSel) vocalSel.value = E.vocalChainPreset
    if (chanSel)  chanSel.value  = E.channelRepairMode === 'gainDb' ? '' : E.channelRepairMode
    if (masterSel) masterSel.value = E.masterPreset
    // The level row depends on the preset we just changed.
    renderLevelSummary()
    if (summary) { summary.textContent = res.summary; (summary as HTMLElement).style.display = '' }
  })

  // Diagnose channels: show the analysis + apply the recommended repair.
  $('btn-diagnose-channels')?.addEventListener('click', async () => {
    if (!E.filePath || !diagLine) return
    const btn = $('btn-diagnose-channels') as HTMLButtonElement | null
    if (btn) { btn.disabled = true; btn.textContent = t('editor.diagnosing', 'Analyserer…') }
    const d = (await window.api.editorDiagnoseChannels(E.filePath)) as {
      code: string; imbalanceDb: number; peakLeftDb: number; peakRightDb: number | null
      recommended: { mode: string; leftDb: number; rightDb: number }
    } | null
    if (btn) { btn.disabled = false; btn.textContent = t('editor.diagnoseChannels', 'Diagnostiser') }
    if (!d) { diagLine.textContent = t('editor.diagnoseFail', 'Kunne ikke analysere kanaler.'); (diagLine as HTMLElement).style.display = ''; return }
    const codeText: Record<string, string> = {
      balanced:  t('editor.chanBalanced', 'Kanalene er balanserte ✓'),
      imbalance: t('editor.chanImbalance', 'Ulik styrke mellom kanalene'),
      dead_left: t('editor.chanDeadLeft', 'Venstre kanal er stille (sjekk kabel)'),
      dead_right:t('editor.chanDeadRight', 'Høyre kanal er stille (sjekk kabel)'),
      both_dead: t('editor.chanBothDead', 'Begge kanaler er svært svake'),
      mono:      t('editor.chanMono', 'Mono-opptak'),
    }
    const lvl = d.peakRightDb === null
      ? `${d.peakLeftDb.toFixed(1)} dB`
      : `V ${d.peakLeftDb.toFixed(1)} / H ${d.peakRightDb.toFixed(1)} dB`
    diagLine.textContent = `${codeText[d.code] ?? d.code} · ${lvl}`
    ;(diagLine as HTMLElement).style.display = ''
    // Apply the recommended repair.
    E.channelRepairMode    = d.recommended.mode === 'none' ? '' : d.recommended.mode
    E.channelRepairLeftDb  = d.recommended.leftDb
    E.channelRepairRightDb = d.recommended.rightDb
    if (chanSel) chanSel.value = E.channelRepairMode === 'gainDb' ? '' : E.channelRepairMode
  })
}

// Publishing options state (mirrored from DOM into module on toggle)
export interface PublishState {
  gdrive:   boolean
  dropbox:  boolean
  onedrive: boolean
  podcast:  boolean
  youtube:  boolean
}
const publishSelections: PublishState = { gdrive: false, dropbox: false, onedrive: false, podcast: false, youtube: false }
let configuredCache: { gdrive: boolean; dropbox: boolean; onedrive: boolean; youtubeConnected: boolean } =
  { gdrive: false, dropbox: false, onedrive: false, youtubeConnected: false }

/**
 * Build the publishing checkbox list in the export modal. Each service is
 * shown ONLY if `cloudIsConfigured(...)` returns true (user has connected it).
 * Podcast appears when settings.podcast.enabled is true. If nothing is
 * configured, we show a single "Konfigurer publisering →" link to the
 * publish settings page.
 *
 * For video files we also append disabled placeholder rows for YouTube +
 * Vimeo so the user can see the roadmap.
 */
export async function renderPublishOptions(): Promise<void> {
  const wrap     = $('export-publish-options')
  const configL  = $('export-publish-configure')
  const andBtn   = $('btn-export-and-publish')
  const progress = $('export-publish-progress')
  if (!wrap || !configL || !andBtn) return

  wrap.innerHTML = ''
  if (progress) { progress.style.display = 'none'; progress.textContent = '' }

  // Refresh service configuration (cheap IPC) — these aren't expected to
  // change mid-session but the user could have configured one in another
  // window so we read fresh each open.
  try {
    configuredCache.gdrive   = await window.api.cloudIsConfigured('google-drive') as boolean
    configuredCache.dropbox  = await window.api.cloudIsConfigured('dropbox') as boolean
    configuredCache.onedrive = await window.api.cloudIsConfigured('onedrive') as boolean
    const yt = await window.api.youtubeStatus()
    configuredCache.youtubeConnected = !!yt?.connected
  } catch { /* leave defaults — falsy */ }

  const podcastEnabled = settings.podcast?.enabled === true

  const haveAny = configuredCache.gdrive || configuredCache.dropbox || configuredCache.onedrive || podcastEnabled || (E.isVideoFile && configuredCache.youtubeConnected)
  configL.style.display = haveAny ? 'none' : ''
  // The "Eksporter og publiser" button is only meaningful if at least one
  // service is configured.
  ;(andBtn as HTMLElement).style.display = haveAny ? '' : 'none'

  function addRow(key: keyof PublishState, label: string, enabled: boolean, disabled = false, tooltip = ''): void {
    const row = document.createElement('label')
    row.className = 'export-publish-option' + (disabled ? ' is-disabled' : '')
    if (tooltip) row.title = tooltip
    const chk = document.createElement('input')
    chk.type = 'checkbox'
    chk.disabled = disabled || !enabled
    chk.checked = false
    chk.addEventListener('change', () => { publishSelections[key] = chk.checked })
    const span = document.createElement('span')
    span.textContent = label
    row.appendChild(chk)
    row.appendChild(span)
    wrap!.appendChild(row)
  }

  // Reset selections each time we open
  publishSelections.gdrive   = false
  publishSelections.dropbox  = false
  publishSelections.onedrive = false
  publishSelections.podcast  = false
  publishSelections.youtube  = false

  if (configuredCache.gdrive)   addRow('gdrive',   t('editor.exportPublishGdrive',   'Last opp til Google Drive'), true)
  if (configuredCache.dropbox)  addRow('dropbox',  t('editor.exportPublishDropbox',  'Last opp til Dropbox'),       true)
  if (configuredCache.onedrive) addRow('onedrive', t('editor.exportPublishOnedrive', 'Last opp til OneDrive'),      true)
  if (podcastEnabled)           addRow('podcast',  t('editor.exportPublishPodcast',  'Oppdater podcast RSS-feed'),  true)

  // Video files: surface YouTube as an actionable row. If user is connected,
  // checkbox enables upload; otherwise we render a "Koble til YouTube"-link
  // so they can opt-in inline without leaving the modal.
  if (E.isVideoFile) {
    if (configuredCache.youtubeConnected) {
      addRow('youtube', t('editor.exportPublishYoutube', 'Last opp video til YouTube (privat)'), true)
    } else {
      const row = document.createElement('div')
      row.className = 'export-publish-option export-publish-connect-row'
      const span = document.createElement('span')
      span.textContent = t('editor.exportPublishYoutube', 'Last opp video til YouTube')
      const link = document.createElement('a')
      link.href = '#'
      link.className = 'export-publish-connect-link'
      link.textContent = t('editor.exportPublishYoutubeConnect', '→ Koble til YouTube')
      link.addEventListener('click', async (e) => {
        e.preventDefault()
        link.textContent = t('editor.exportPublishYoutubeConnecting', 'Åpner Google-pålogging…')
        const res = await window.api.youtubeConnect()
        if (res?.ok) {
          configuredCache.youtubeConnected = true
          await renderPublishOptions()
        } else {
          link.textContent = `${t('editor.exportPublishYoutubeFailed', 'Tilkobling feilet')}: ${res?.error ?? ''}`.slice(0, 80)
        }
      })
      row.appendChild(span)
      row.appendChild(link)
      wrap.appendChild(row)
    }

    // Vimeo placeholder remains for later phase — it has a fundamentally
    // different OAuth+API model so it's a separate workstream.
    const vmLabel = t('editor.exportPublishVimeo', 'Last opp video til Vimeo')
    const phase2  = t('editor.exportPublishPhase2', 'Kommer i en senere versjon — krever separat OAuth-oppsett')
    addRow('gdrive', vmLabel, false, /*disabled*/ true, phase2)
  }
}

/**
 * Run the selected publishing actions for a freshly-exported file. Surfaces
 * progress in the export modal (which is still up — we don't close it
 * until publishing completes). Idempotent on its own — the underlying
 * cloud queue dedupes by file path + service.
 */
export async function runPublishingForExport(outputPath: string): Promise<void> {
  const progress = $('export-publish-progress')
  if (progress) { progress.style.display = ''; progress.classList.remove('is-error', 'is-success'); progress.textContent = '' }

  const tasks: { label: string; run: () => Promise<{ ok: boolean; error?: string; url?: string }> }[] = []
  if (publishSelections.gdrive) {
    tasks.push({ label: 'Google Drive', run: () => window.api.cloudUploadFile('google-drive', outputPath) as Promise<{ ok: boolean; error?: string }> })
  }
  if (publishSelections.dropbox) {
    tasks.push({ label: 'Dropbox', run: () => window.api.cloudUploadFile('dropbox', outputPath) as Promise<{ ok: boolean; error?: string }> })
  }
  if (publishSelections.onedrive) {
    tasks.push({ label: 'OneDrive', run: () => window.api.cloudUploadFile('onedrive', outputPath) as Promise<{ ok: boolean; error?: string }> })
  }
  if (publishSelections.youtube) {
    // Build metadata from the file name + chapter metadata.
    const title = (E.meta.title?.trim() || (outputPath.split(/[/\\]/).pop() ?? 'SundayRec opptak')).replace(/\.[^.]+$/, '')
    const description = (E.meta.description ?? '').slice(0, 5000)
    tasks.push({
      label: 'YouTube',
      run: async () => {
        // Subscribe to progress events for this upload so the user sees a
        // live percentage instead of a frozen "Laster opp…" string. The
        // unsubscribe call is fired when the upload-promise settles.
        const unsub = window.api.on?.('youtube-upload-progress', (payload: unknown) => {
          if (progress && payload && typeof payload === 'object') {
            const { uploadedBytes, totalBytes } = payload as { uploadedBytes: number; totalBytes: number }
            if (totalBytes > 0) {
              const pct = Math.floor((uploadedBytes / totalBytes) * 100)
              progress.textContent = `${t('editor.publishUploading', 'Laster opp til')} YouTube… ${pct}%`
            }
          }
        })
        try {
          const r = await window.api.youtubeUpload(outputPath, {
            title,
            description,
            privacyStatus: 'private',  // safe default — user changes from YouTube Studio if they want public
          })
          return { ok: !!r?.ok, error: r?.error, url: r?.url }
        } finally {
          unsub?.()
        }
      },
    })
  }

  let allOk = true
  const messages: string[] = []
  for (const task of tasks) {
    if (progress) progress.textContent = `${t('editor.publishUploading', 'Laster opp til')} ${task.label}…`
    try {
      const r = await task.run()
      if (r && r.ok === false) {
        allOk = false
        messages.push(`${task.label}: ${r.error ?? 'feil'}`)
      } else if (r && r.url) {
        messages.push(`${task.label}: ✓ (${r.url})`)
      } else {
        messages.push(`${task.label}: ✓`)
      }
    } catch (err) {
      allOk = false
      messages.push(`${task.label}: ${(err as Error).message}`)
    }
  }

  // Podcast RSS regen runs last (after any uploads complete, since RSS may
  // reference the just-uploaded cloud URLs).
  if (publishSelections.podcast) {
    if (progress) progress.textContent = t('editor.publishRssUpdating', 'Oppdaterer RSS-feed…')
    const service = settings.podcast?.service ?? 'google-drive'
    try {
      const r = await window.api.podcastRegenerate(service) as { ok: boolean; error?: string }
      if (r && r.ok === false) {
        allOk = false
        messages.push(`RSS: ${r.error ?? 'feil'}`)
      } else {
        messages.push(`RSS: ✓`)
      }
    } catch (err) {
      allOk = false
      messages.push(`RSS: ${(err as Error).message}`)
    }
  }

  if (progress) {
    progress.classList.toggle('is-success', allOk)
    progress.classList.toggle('is-error', !allOk)
    progress.textContent = (allOk ? `${t('editor.publishDone', '✓ Publisering ferdig')} — ` : `${t('editor.publishFailed', '✕ Publisering feilet')} — `) + messages.join(' · ')
  }
}

export function closeExportModal(): void {
  closeModal('editor-export-modal')
}

/** Localised label for a backend progress phase code. The codes themselves live
 *  in export-params.ts (one definition, pinned against the Rust seam by a test
 *  on each side) — never spell them out again here. */
function exportPhaseText(phase: string): string {
  return phase === EXPORT_PHASE_MEASURING
    ? t('editor.exportPhaseMeasuring', 'Måler lydstyrke…')
    : t('editor.exportPhaseEncoding', 'Eksporterer…')
}

let cancelWired = false

/**
 * Wire the progress row's Avbryt button once. The backend kills the render's
 * ffmpeg AND raises a cancel flag the export checks before every remaining
 * pass, so the export rejects with `cancelled`, which describeExportError turns
 * into a calm Norwegian sentence.
 *
 * The button is deliberately NOT disabled on click. It used to be — permanently,
 * because nothing re-enabled it — so a cancel that landed in one of the export's
 * child-less gaps (the source probe, the loudnorm-JSON parse, the jingle probes)
 * left the user staring at "Avbryter…" on a dead button while the export ran to
 * completion. A second click is harmless: cancel is idempotent.
 */
function wireExportCancel(): void {
  if (cancelWired) return
  cancelWired = true
  const cancelBtn = $('btn-editor-export-cancel') as HTMLButtonElement | null
  cancelBtn?.addEventListener('click', async () => {
    cancelBtn.textContent = t('editor.exportCancelling', 'Avbryter…')
    await window.api.editorCancelExport()
  })
}

export async function runExport(): Promise<void> {
  closeExportModal()
  const btn      = $('btn-editor-save') as HTMLButtonElement
  const progRow  = $('editor-export-progress-row')
  const progHost = $('editor-export-progress-host')
  const cancelBtn = $('btn-editor-export-cancel') as HTMLButtonElement | null
  const resultRow = $('editor-result-row')

  wireExportCancel()
  if (btn)     { btn.disabled = true; btn.textContent = t('editor.exportExporting') || 'Eksporterer…' }
  if (progRow) progRow.style.display = ''
  // Start indeterminate: ffmpeg's first -progress tick is a moment away (and the
  // mastering measure pass has no percentage at all), so a bar pinned at 0%
  // would read as hung. The listener below swaps in the real bar on tick one,
  // and the widget adds «ca. 2 min igjen» once its rate estimate settles — an
  // export of a service is the longest wait in the app.
  const progressUi = progHost ? attachProgress(progHost, { compact: true }) : null
  progressUi?.update(null, t('editor.exportExporting') || 'Eksporterer…')
  if (cancelBtn) { cancelBtn.disabled = false; cancelBtn.textContent = t('editor.exportCancel', 'Avbryt') }
  if (resultRow) { resultRow.style.display = 'none' }

  const fmt = (document.querySelector<HTMLElement>('#export-fmt-section .export-fmt-btn.active')?.dataset.fmt ?? 'mp3') as 'mp3'|'wav'|'flac'|'aac'
  // AAC has its OWN bitrate dropdown; mp3 (and the fallback) use #export-bitrate.
  // Reading #export-bitrate for every format made the AAC dropdown a dead control
  // (the user's AAC choice was silently ignored — the hidden mp3 select won).
  const bitrateSel = fmt === 'aac' ? 'export-aac-bitrate' : 'export-bitrate'
  const bitrate   = parseInt((($(bitrateSel)          as HTMLSelectElement)?.value  ?? '256'))
  const bitDepth  = parseInt((($('export-bitdepth')   as HTMLSelectElement)?.value  ?? '16')) as 16|24

  // Auto-save metadata before export
  if (E.metaDirty) await saveMetadata()

  let result: { ok: boolean; outputPath?: string; error?: string }

  // Audio-enhancement fields (channel repair + vocal chain + mastering preset).
  const channelRepair = E.channelRepairMode
    ? { mode: E.channelRepairMode, leftDb: E.channelRepairLeftDb, rightDb: E.channelRepairRightDb }
    : undefined
  // The advanced mixer (when enabled) overrides the preset → send full processing.
  const processing = E.useMixer ? mixerProcessing() : undefined
  const vocalChainPreset = E.useMixer ? undefined : (E.vocalChainPreset || undefined)
  const isVideoExport = E.isVideoFile && !E.videoExportAudioOnly

  const params = buildExportRequest({
    kind:         isVideoExport ? 'video' : 'audio',
    inputPath:    E.filePath,
    cutRegions:   E.cuts,
    duration:     E.duration,
    // '' = the default "Samme mappe" pill → the backend writes beside the source.
    outputFolder: E.exportOutputFolder,
    format:       fmt,
    bitrate,
    bitDepth,
    videoFormat:  E.videoFormat,
    videoCodec:   E.videoCodec,
    gainDb:       E.audioGainDb,
    // Jingles apply only to native AUDIO files. A video export drops them in the
    // seam (the video graph has nowhere to splice them), and extracting audio out
    // of a video exports the bare track — so sending a path either way only risks
    // failing the export on the backend's path guard for a file it then ignores.
    // The UI says so: `editor.jinglesVideoUnsupported`.
    introPath: (!E.isVideoFile && E.includeIntroOutro) ? settings.editorIntroPath : undefined,
    outroPath: (!E.isVideoFile && E.includeIntroOutro) ? settings.editorOutroPath : undefined,
    metadata:     E.meta,
    masterPreset: E.masterPreset,
    vocalChainPreset,
    processing,
    channelRepair,
  })

  // Live progress for the duration of THIS export only — a module-level
  // subscription would keep writing to the bar after the row is hidden.
  const unsub = window.api.on?.('editor-export-progress', (payload: unknown) => {
    const { pct, phase } = (payload ?? {}) as { pct?: number; phase?: string }
    if (typeof pct !== 'number' || !isFinite(pct)) return
    const shown = Math.max(0, Math.min(100, pct))
    const label = exportPhaseText(phase ?? '')
    // A concrete % arrived → a real bar. Zero is the mastering measure pass
    // announcing itself with no percentage of its own: name the phase, keep the
    // stripe moving, and do NOT feed the estimator a fraction that isn't one.
    progressUi?.update(shown > 0 ? shown / 100 : null, label)
  })

  try {
    result = isVideoExport
      ? await window.api.editorExportVideo(params)
      : await window.api.editorExportFile(params)
  } finally {
    unsub?.()
  }

  if (progRow) progRow.style.display = 'none'
  progressUi?.destroy()
  if (btn) { btn.disabled = false; btn.textContent = t('editor.save') || 'Eksporter' }

  const row  = $('editor-result-row')
  const text = $('editor-result-text')
  if (row) row.style.display = ''

  if (result.ok) {
    const fname = (result.outputPath ?? '').split(/[/\\]/).pop() ?? ''
    if (text) text.textContent = (t('editor.saveOk') || '✓ Eksportert') + (fname ? ' — ' + fname : '')
    if (row) row.setAttribute('data-ok', 'true')
    // The export modal closes on the way in, and the result line lives at the
    // very bottom of a workspace several screens tall — a successful export
    // used to announce itself somewhere the user could not see. Say it where
    // they ARE looking, with the one thing they want next (the file), and
    // bring the result line into view behind it.
    const out = result.outputPath
    toast(
      'success',
      t('editor.exportDoneToast', 'Eksportert{name}').replace('{name}', fname ? ` — ${fname}` : ''),
      out
        ? { action: { label: t('general.showInFolder', 'Vis i mappe'), onClick: () => { void window.api.revealFile(out) } } }
        : undefined,
    )
    row?.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
    clearEditorDraft()  // export succeeded — drop the autosave sidecar
    clearDirty()
    // Run publishing if user picked "Eksporter og publiser"
    if (E.publishAfterExport && result.outputPath) {
      await runPublishingForExport(result.outputPath)
    }
    E.publishAfterExport = false
  } else {
    if (text) text.textContent = describeExportError(result.error)
    if (row) row.removeAttribute('data-ok')
  }
}

// The codes the backend actually embeds in an export failure. Order matters only
// in that the first match wins; none of these is a substring of another.
//
// Every entry here is grep-verified against a real emitter in the Rust seam.
// Three used to be listed that NOTHING emits — `force_wav_replace_unsafe` and
// `invalid_cut_regions` died with the Electron save/replace layer, and
// `invalid_path` only ever came from the cloud-integrations commands — so their
// friendly messages were unreachable while two codes the export DOES produce
// fell through to the raw "✕ Feil: validation: invalid_format: xyz".
const EXPORT_ERROR_CODES = [
  'no_audio_remaining',
  'cancelled',
  'timeout',
  'file_not_found',
  'invalid_duration',
  // editor/mod.rs: the export format gate (`is_supported_export_format`).
  'invalid_format',
  // commands/path_guard.rs: a relative output folder / jingle path.
  'path must be absolute',
] as const

/**
 * Map an export error from the backend to a user-friendly Norwegian sentence.
 * Falls back to the raw text so an unfamiliar error still surfaces something
 * the user can search for.
 *
 * Matched by CONTAINMENT, not equality: `AppError` serializes as
 * "<category>: <code>" (e.g. "recording error: timeout"), so the old
 * `switch (err)` on the bare code never fired and every friendly message below
 * was dead — the user got "✕ Feil: validation: no_audio_remaining".
 */
export function describeExportError(err: string | undefined): string {
  const code = err ? EXPORT_ERROR_CODES.find((c) => err.includes(c)) : undefined
  switch (code) {
    case 'no_audio_remaining':
      return '✕ ' + t('editor.errNoAudioRemaining', 'Ingen lyd igjen — kuttene dekker hele opptaket. Fjern minst ett kutt før du eksporterer.')
    case 'cancelled':
      return '✕ ' + t('editor.errCancelled', 'Eksport avbrutt.')
    case 'timeout':
      return '✕ ' + t('editor.errTimeout', 'Eksporten tok for lang tid og ble stoppet. Prøv igjen, eller del filen i flere mindre opptak.')
    case 'file_not_found':
      return '✕ ' + t('editor.errFileNotFound', 'Originalfilen er ikke tilgjengelig — er disken frakoblet?')
    case 'invalid_duration':
      return '✕ ' + t('editor.errCutData', 'Intern feil i kuttdataene. Prøv å laste filen på nytt.')
    // No locale key yet for these two — `t` returns the fallback for a missing
    // key, so the Norwegian sentence ships today and adding
    // `editor.errInvalidFormat` / `editor.errPathNotAbsolute` to the locale
    // files later needs no code change here.
    case 'invalid_format':
      return '✕ ' + t('editor.errInvalidFormat', 'Formatet støttes ikke for eksport. Velg et annet format i eksportvinduet.')
    case 'path must be absolute':
      return '✕ ' + t('editor.errPathNotAbsolute', 'Ugyldig mappe eller filbane. Velg destinasjonsmappen på nytt.')
    default:
      return (t('editor.saveError') || '✕ Feil') + (err ? ': ' + err : '')
  }
}

export function updateExportFormatUI(fmt: string): void {
  const mp3  = $('export-mp3-opts')
  const wav  = $('export-wav-opts')
  const aac  = $('export-aac-opts')
  if (mp3) mp3.style.display = fmt === 'mp3' ? '' : 'none'
  if (wav) wav.style.display = fmt === 'wav' ? '' : 'none'
  if (aac) aac.style.display = fmt === 'aac' ? '' : 'none'
}
