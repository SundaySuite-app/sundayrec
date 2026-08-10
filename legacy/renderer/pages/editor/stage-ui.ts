/**
 * Stage-kapitler UI — «↧ Stage-kapitler»-knapp i analyse-panelet.
 *
 * Vises kun når Stage-integrasjon er skrudd på. Lar brukeren velge
 * Stage-manifestet (service-manifest.json) og kaller `stage_import_apply`, som
 * aligner timestamps → ChapterMarker[] → .meta.json + skriver .service.json.
 * Tegner waveform om igjen (kapitler vises som cyan streker på tidslinjen).
 *
 * Manifestet leses som INNHOLD, ikke sti: en `<input type="file">` i webviewen
 * gir aldri fra seg filsystem-stien (den gamle Electron-koden leste `.path`,
 * som ikke finnes i Tauri — knappen kunne aldri ha virket). `File.text()` er
 * derimot alltid lesbart, og backend trenger uansett bare JSON-teksten.
 */

import { t } from '../../i18n'
import { E } from './state'
import { drawWaveform } from './waveform'

const $ = (id: string) => document.getElementById(id)

/** Kall én gang fra setupEditorPage — kobler knappen. */
export function setupStageUi(): void {
  $('btn-stage-import')?.addEventListener('click', runStageImport)
}

/** Oppdaterer synligheten basert på integrasjonsinnstillingene. Kalles ved
 *  fil-last (loadFile) og ved settings-endring. */
export async function updateStageButton(): Promise<void> {
  const btn = $('btn-stage-import') as HTMLElement | null
  if (!btn) return
  let show = false
  try {
    if (E.filePath) {
      const s = await window.api.getIntegrationSettings()
      show = !!s.enabled && !!s.stage?.enabled
    }
  } catch { show = false }
  btn.style.display = show ? '' : 'none'
}

async function runStageImport(): Promise<void> {
  if (!E.filePath) return
  const btn = $('btn-stage-import') as HTMLButtonElement | null

  const manifestJson = await pickManifestText()
  if (manifestJson === null) return

  // was_streamed er alltid false siden v0.14: SundayRec er et opptaksprogram
  // og har ingen strømme-funksjon lenger (feltet består i stage_import_apply
  // for kontraktens skyld).
  if (btn) { btn.textContent = '…'; (btn as HTMLButtonElement).disabled = true }
  try {
    const res = await window.api.stageImport(E.filePath, manifestJson, false)
    if (res.ok) {
      // Refresh metadata in the editor — re-read the sidecar just written.
      const meta = await window.api.editorReadMeta?.(E.filePath) as { chapters?: unknown[] } | null
      if (meta?.chapters && Array.isArray(meta.chapters)) {
        E.meta.chapters = meta.chapters as typeof E.meta.chapters
        drawWaveform()
      }
      if (btn) btn.textContent = `✓ ${res.chapterCount} ${t('integrations.stageChapters', 'kapitler')}, ${res.songCount} ${t('integrations.stageSongs', 'sanger')}`
      setTimeout(() => { if (btn) { btn.textContent = '↧ Stage-kapitler'; (btn as HTMLButtonElement).disabled = false } }, 3000)
    } else {
      // Vis den EKTE grunnen — «✕ Feil» uten forklaring var det gamle mønsteret.
      if (btn) {
        btn.textContent = res.error === 'invalid_manifest'
          ? t('integrations.stageInvalidManifest', '✕ Ugyldig manifest')
          : res.error === 'recording_not_in_history'
            ? t('integrations.stageNotInHistory', '✕ Opptaket er ikke i historikken')
            : `✕ ${res.error ?? t('integrations.stageFailed', 'Feil')}`
      }
      setTimeout(() => { if (btn) { btn.textContent = '↧ Stage-kapitler'; (btn as HTMLButtonElement).disabled = false } }, 3500)
    }
  } catch {
    if (btn) { btn.textContent = `✕ ${t('integrations.stageFailed', 'Feil')}`; setTimeout(() => { if (btn) { btn.textContent = '↧ Stage-kapitler'; (btn as HTMLButtonElement).disabled = false } }, 2500) }
  }
}

/** Åpne en fil-velger for JSON-filer og les innholdet. `null` = avbrutt. */
function pickManifestText(): Promise<string | null> {
  return new Promise(resolve => {
    const input = document.createElement('input')
    input.type = 'file'
    input.accept = '.json,application/json'
    input.style.display = 'none'
    document.body.appendChild(input)
    input.addEventListener('change', () => {
      const file = input.files?.[0]
      document.body.removeChild(input)
      if (!file) { resolve(null); return }
      file.text().then(resolve, () => resolve(null))
    })
    input.addEventListener('cancel', () => { document.body.removeChild(input); resolve(null) })
    input.click()
  })
}
