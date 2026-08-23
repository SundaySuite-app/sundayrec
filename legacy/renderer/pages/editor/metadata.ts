import { E, $ } from './state'

// ── Metadata ─────────────────────────────────────────────────

export async function saveMetadata(): Promise<void> {
  if (!E.filePath) return
  await window.api.editorSaveMeta(E.filePath, E.meta)
  E.metaDirty = false
  const btn = $('btn-meta-save')
  if (btn) { btn.textContent = '✓ Lagret'; setTimeout(() => { btn.textContent = 'Lagre metadata' }, 1500) }
}

export function renderMetaPanel(): void {
  const titleEl = $('meta-title') as HTMLInputElement | null
  const spkEl   = $('meta-speaker') as HTMLInputElement | null
  const descEl  = $('meta-description') as HTMLTextAreaElement | null
  if (titleEl) titleEl.value = E.meta.title
  if (spkEl)   spkEl.value   = E.meta.speaker
  if (descEl)  descEl.value  = E.meta.description
}
