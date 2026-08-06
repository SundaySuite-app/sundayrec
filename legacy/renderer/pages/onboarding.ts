import { settings, patchSettings } from '../state'
import { t, tArr } from '../i18n'
import { enhanceTimeInput } from '../time-input'
import { getAudioDevices, isBuiltInDevice } from '../audio/capture'
import { makeVuState, pushVuLevels, stopVuState } from '../audio/vu'
import { acquireVuFeed } from '../audio/vu-feed'
import { pickLR } from '../audio/vu-feed-core'

// ── VU state for audio test step ─────────────────────────────────
//
// Fed by the shared backend VU feed (audio/vu-feed.ts). The step used to open
// its own getUserMedia stream through a fixed 2-channel splitter, which both
// made the webview an owner of the input device (the Qu-5 hazard) and meant the
// test measured channels 0/1 of a 32-channel mixer no matter which channels the
// user had picked — a "lyden fungerer ✓" that proved nothing about the take.
let obVu = makeVuState()
/** Release handle for the feed subscription; non-null ⇔ the test is running. */
let obRelease: (() => void) | null = null

/**
 * Has the meter actually seen sound during THIS visit to step 3?
 *
 * The step's whole job is to prove the audio path works, and «Lyden fungerer →»
 * used to be clickable whether or not it did — so the wizard would cheerfully
 * confirm a muted mixer, and the first anyone knew was a silent recording of a
 * service. The button now waits for real signal.
 */
let obSignalSeen = false
/** dBFS above which we call it signal rather than room noise / a dead input.
 *  The step's own scale calls anything under -55 «venter på lyd» and -55..-40
 *  «svakt signal»; -45 sits inside that band, so speaking normally into a
 *  correctly-wired input clears it in a second, and a disconnected one never
 *  does. */
const OB_SIGNAL_DB = -45

function stopObVU(): void {
  if (obRelease) {
    const r = obRelease
    obRelease = null
    try { r() } catch { /* gone */ }
  }
  stopVuState(obVu); obVu = makeVuState()
}

// ── Device chosen in step 2 ──────────────────────────────────────
let pickedId:   string | null = null
let pickedName: string | null = null

// ── Public API ───────────────────────────────────────────────────
export function checkAndShowOnboarding(): void {
  if (settings.onboardingDone) return
  showOnboarding()
}

export function showOnboarding(): void {
  const el = document.getElementById('onboarding-overlay')
  if (!el) return
  el.style.transition = ''
  el.style.opacity    = '1'
  el.style.display    = 'flex'

  // Replace skip-all button to clear any stale listeners from a previous show
  const oldSkip = document.getElementById('ob-btn-skip-all')
  if (oldSkip) {
    const newSkip = oldSkip.cloneNode(true) as HTMLElement
    oldSkip.replaceWith(newSkip)
    newSkip.addEventListener('click', finish)
  }

  goTo(1)
}

// ── Finish wizard ─────────────────────────────────────────────────
function finish(): void {
  stopObVU()
  patchSettings({ onboardingDone: true })
  void window.api.saveSettings(settings)
  const el = document.getElementById('onboarding-overlay')
  if (!el) return
  el.style.transition = 'opacity .35s'
  el.style.opacity    = '0'
  setTimeout(() => { el.style.display = 'none'; el.style.opacity = '' }, 380)
}

// ── Step router ──────────────────────────────────────────────────
function goTo(step: number): void {
  stopObVU()
  const prog = document.getElementById('ob-progress')
  const body = document.getElementById('ob-body')
  if (!prog || !body) return

  if (step <= 4) {
    prog.innerHTML = [1, 2, 3, 4].map(i =>
      `<div class="ob-dot${i === step ? ' active' : i < step ? ' done' : ''}"></div>`
    ).join('')
  }

  // Reset transition first so the hide is instant, not animated
  body.style.transition = ''
  body.style.opacity    = '0'
  body.style.transform  = 'translateY(8px)'

  setTimeout(() => {
    if      (step === 1) s1(body)
    else if (step === 2) void s2(body)
    else if (step === 3) s3(body)
    else if (step === 4) s4(body)
    else                 { allDots(); sDone(body) }
    body.style.transition = 'opacity .22s, transform .22s'
    body.style.opacity    = '1'
    body.style.transform  = 'translateY(0)'
  }, 80)
}

function allDots(): void {
  const prog = document.getElementById('ob-progress')
  if (prog) prog.innerHTML = [1,2,3,4].map(() => `<div class="ob-dot done"></div>`).join('')
}

/**
 * «‹ Tilbake» for steps 2–4.
 *
 * The wizard was one-way: a volunteer who picked the wrong mixer on step 2 and
 * noticed on step 3 had no way back except finishing the whole thing and
 * reopening it from Innstillinger. Every step after the first now has a way
 * back to the one before.
 */
function backButton(step: number): string {
  return `<button class="ob-text-btn ob-back-btn" id="ob-back-${step}">${esc(t('onboarding.back', '‹ Tilbake'))}</button>`
}

function wireBack(step: number): void {
  document.getElementById(`ob-back-${step}`)?.addEventListener('click', () => goTo(step - 1))
}

// ── Step 1: Welcome ──────────────────────────────────────────────
function s1(body: HTMLElement): void {
  body.innerHTML = `
    <div class="ob-icon">
      <svg viewBox="0 0 24 24"><path d="M12 2a3 3 0 00-3 3v6a3 3 0 006 0V5a3 3 0 00-3-3zM7 11a5 5 0 0010 0h2a7 7 0 01-14 0zm5 8h4v2h-4z"/></svg>
    </div>
    <h2 class="ob-title">${esc(t('onboarding.welcomeTitle', 'Velkommen til SundayRec'))}</h2>
    <p class="ob-desc">${esc(t('onboarding.welcomeDesc', 'La oss sette opp programmet på noen minutter, slik at alt er klart til søndagen.'))}</p>
    <div class="ob-features">
      <div class="ob-feature"><span>🎛️</span><span>${esc(t('onboarding.featDevice', 'Velg mikser eller lydkort'))}</span></div>
      <div class="ob-feature"><span>✅</span><span>${esc(t('onboarding.featTest', 'Test at lyden fungerer'))}</span></div>
      <div class="ob-feature"><span>📅</span><span>${esc(t('onboarding.featSchedule', 'Sett opp automatisk ukentlig opptak'))}</span></div>
    </div>
    <div class="ob-actions">
      <button class="btn-primary" id="ob-n1" style="justify-content:center">${esc(t('onboarding.startBtn', 'Kom i gang →'))}</button>
      <button class="ob-text-btn" id="ob-s1">${esc(t('onboarding.skipAll', 'Hopp over — sett opp manuelt'))}</button>
    </div>`
  document.getElementById('ob-n1')?.addEventListener('click', () => goTo(2))
  document.getElementById('ob-s1')?.addEventListener('click', finish)
}

// ── Step 2: Pick device ───────────────────────────────────────────
async function s2(body: HTMLElement): Promise<void> {
  pickedId   = settings.deviceId   ?? null
  pickedName = settings.deviceName ?? null

  body.innerHTML = `
    <h2 class="ob-title">${esc(t('onboarding.deviceTitle', 'Hvilken lydenhet bruker dere?'))}</h2>
    <p class="ob-desc">${esc(t('onboarding.deviceDesc', 'Velg mikseren eller lydkortet som er koblet til datamaskinen.'))}</p>
    <div id="ob-dev-list" class="ob-dev-list"><div class="ob-loading">${esc(t('onboarding.deviceSearching', 'Leter etter lydenheter…'))}</div></div>
    <div class="ob-actions">
      <button class="btn-primary" id="ob-n2" style="justify-content:center">${esc(t('onboarding.deviceUse', 'Bruk valgt enhet →'))}</button>
      <button class="ob-text-btn" id="ob-s2">${esc(t('onboarding.skipStep', 'Hopp over dette steget'))}</button>
      ${backButton(2)}
    </div>`

  const list    = document.getElementById('ob-dev-list')!
  const devices = await getAudioDevices()

  if (!devices.length) {
    list.innerHTML = `<p class="ob-empty">${esc(t('onboarding.deviceNone', 'Ingen lydenheter funnet. Kontroller at mikseren er koblet til via USB.'))}</p>`
  } else {
    const preferred = devices.find(d => !isBuiltInDevice(d.label)) ?? devices[0]
    if (!pickedId) { pickedId = preferred?.deviceId ?? null; pickedName = preferred?.label ?? null }

    list.innerHTML = devices.map(d => {
      const builtIn  = isBuiltInDevice(d.label)
      const selected = d.deviceId === pickedId
      return `<div class="ob-dev-card${selected ? ' sel' : ''}" data-id="${esc(d.deviceId)}" data-name="${esc(d.label)}">
        <span class="ob-dev-emoji">${builtIn ? '💻' : '🎛️'}</span>
        <div class="ob-dev-info">
          <div class="ob-dev-name">${esc(d.label || t('onboarding.deviceUnknown', 'Ukjent enhet'))}</div>
          <div class="ob-dev-hint">${esc(builtIn ? t('onboarding.deviceBuiltinHint', 'Innebygd mikrofon — passer til testing') : t('onboarding.deviceMixerHint', 'Mikser / lydkort — anbefalt for gudstjeneste'))}</div>
        </div>
        <span class="ob-badge${builtIn ? ' warn' : ' ok'}">${esc(builtIn ? t('onboarding.badgeNotRecommended', 'Ikke anbefalt') : t('onboarding.badgeRecommended', 'Anbefalt ✓'))}</span>
      </div>`
    }).join('')

    list.querySelectorAll<HTMLElement>('.ob-dev-card').forEach(card => {
      card.addEventListener('click', () => {
        list.querySelectorAll('.ob-dev-card').forEach(c => c.classList.remove('sel'))
        card.classList.add('sel')
        pickedId   = card.dataset.id   ?? null
        pickedName = card.dataset.name ?? null
      })
    })
  }

  document.getElementById('ob-n2')?.addEventListener('click', () => {
    // OPPGAVE 2: validate that at least one audio device is selected
    const audioDeviceOk = pickedId || settings.deviceId
    if (!audioDeviceOk) {
      showOnboardingError(t('onboarding.errNoDevice', 'Velg en lydenhet før du fortsetter'))
      return
    }
    if (pickedId) {
      patchSettings({ deviceId: pickedId, deviceName: pickedName })
      void window.api.saveSettings(settings)
    }
    goTo(3)
  })
  document.getElementById('ob-s2')?.addEventListener('click', () => goTo(3))
  wireBack(2)
}

// ── Step 3: Audio test ────────────────────────────────────────────
function s3(body: HTMLElement): void {
  obSignalSeen = false
  body.innerHTML = `
    <h2 class="ob-title">${esc(t('onboarding.testTitle', 'Test at lyden fungerer'))}</h2>
    <p class="ob-desc">${esc(t('onboarding.testDesc', 'Si noe i mikrofonen, eller spill lyd gjennom mikseren. Sjekk at indikatoren beveger seg.'))}</p>
    <div class="ob-vu-wrap">
      <div class="ob-vu-track"><div class="ob-vu-fill" id="ob-vu-fill"></div></div>
      <div class="ob-vu-lbl" id="ob-vu-lbl">${esc(t('onboarding.vuWaiting', 'Venter på lyd…'))}</div>
    </div>
    <div class="ob-actions">
      <button class="btn-primary" id="ob-n3" style="justify-content:center" disabled>${esc(t('onboarding.testOk', 'Lyden fungerer →'))}</button>
      <div class="ob-hint" id="ob-n3-hint">${esc(t('onboarding.testWaitHint', 'Knappen åpner seg så snart måleren fanger opp lyd.'))}</div>
      <button class="ob-text-btn" id="ob-s3">${esc(t('onboarding.skipStep', 'Hopp over dette steget'))}</button>
      ${backButton(3)}
    </div>`

  /** Open the «Lyden fungerer →» button — the step has proved its point. */
  const markSignalSeen = (): void => {
    if (obSignalSeen) return
    obSignalSeen = true
    const btn = document.getElementById('ob-n3') as HTMLButtonElement | null
    const hint = document.getElementById('ob-n3-hint')
    if (btn) btn.disabled = false
    if (hint) hint.textContent = t('onboarding.testHeardHint', 'Lyd registrert — du kan gå videre.')
  }

  const devChannels = pickedId ? (settings.deviceChannels?.[pickedId] ?? null) : null
  const pick = {
    mode: settings.channels ?? 'stereo',
    chL: devChannels?.channelL ?? 0,
    chR: devChannels?.channelR ?? 1,
  }
  obVu = makeVuState()
  obRelease = acquireVuFeed({
    deviceName: pickedName ?? settings.deviceName ?? null,
    pick: () => pick,
    onLevels: (l, r, levels) => {
      const pk = pickLR(levels.peak_dbfs, pick.mode, pick.chL, pick.chR)
      pushVuLevels(obVu, l, r, pk.l, pk.r)
      // The gate reads the RMS, the same quantity the old getDbFS meter gated
      // on — so -45 dBFS still means what it meant.
      const db = Math.max(obVu.smL, obVu.smR)
      // One moment of real signal is enough, and it stays unlocked (a meter
      // that dips back to silence between words must not re-lock the button
      // under the user's cursor).
      if (db >= OB_SIGNAL_DB) markSignalSeen()
      const fill = document.getElementById('ob-vu-fill')
      const lbl  = document.getElementById('ob-vu-lbl')
      if (!fill || !lbl) return
      fill.style.width = Math.max(0, Math.min(100, (db + 60) / 60 * 100)) + '%'
      if      (db >= -3)  { fill.style.background = 'var(--red)';    lbl.textContent = t('onboarding.vuTooLoud', 'For høyt — skru ned volumet på mikseren'); lbl.style.color = 'var(--red)' }
      else if (db >= -12) { fill.style.background = 'var(--orange)'; lbl.textContent = t('onboarding.vuLoudOk', 'Høyt, men OK');                           lbl.style.color = 'var(--orange)' }
      else if (db >= -40) { fill.style.background = 'var(--green)';  lbl.textContent = t('onboarding.vuGood', '✓ Bra signal — lyden fungerer');          lbl.style.color = 'var(--green)' }
      else if (db > -55)  { fill.style.background = 'var(--text3)';  lbl.textContent = t('onboarding.vuWeak', 'Svakt signal — sjekk kabelen');          lbl.style.color = 'var(--text3)' }
      else                { fill.style.background = 'var(--text4)';  lbl.textContent = t('onboarding.vuWaiting', 'Venter på lyd…');                         lbl.style.color = 'var(--text3)' }
    },
    onState: (state) => {
      if (state !== 'failed') return
      const lbl = document.getElementById('ob-vu-lbl')
      if (lbl) {
        lbl.textContent = t('onboarding.vuError', 'Kunne ikke starte lydtest — sjekk at tillatelse er gitt i systeminnstillingene.')
        lbl.style.color = 'var(--orange)'
      }
      // The meter never got to run, so it can never open the button. Don't trap
      // the user behind a test that cannot be taken: point at the way past.
      const hint = document.getElementById('ob-n3-hint')
      if (hint) hint.textContent = t('onboarding.testBlockedHint', 'Lydtesten kunne ikke starte. Gå videre og test lyden fra Innstillinger → Lyd senere.')
      const skip = document.getElementById('ob-s3')
      if (skip) skip.textContent = t('onboarding.testSkipAfterError', 'Fortsett uten lydtest →')
    },
  })

  document.getElementById('ob-n3')?.addEventListener('click', () => goTo(4))
  document.getElementById('ob-s3')?.addEventListener('click', () => goTo(4))
  wireBack(3)
}

// ── Step 4: Schedule ─────────────────────────────────────────────
function s4(body: HTMLElement): void {
  const dayLabels  = tArr('schedule.days', ['Man', 'Tir', 'Ons', 'Tor', 'Fre', 'Lør', 'Søn'])
  let selectedDay  = 6  // default: Sunday

  const getSlotForDay = (d: number) => settings.slots?.find(s => s.days.includes(d))

  const existing = getSlotForDay(selectedDay)
  const defStart = existing?.start ?? '11:00'
  const defDur   = existing
    ? (() => {
        const [sh, sm] = existing.start.split(':').map(Number)
        const [eh, em] = existing.stop.split(':').map(Number)
        return (eh * 60 + em) - (sh * 60 + sm)
      })()
    : 90

  body.innerHTML = `
    <h2 class="ob-title">${esc(t('onboarding.schedTitle', 'Ukentlig automatisk opptak'))}</h2>
    <p class="ob-desc">${esc(t('onboarding.schedDesc', 'SundayRec kan starte og stoppe opptaket automatisk — uten at noen trenger å huske det.'))}</p>
    <div class="ob-sched-card">
      <label class="ob-toggle-row">
        <span>${esc(t('onboarding.schedEnable', 'Aktiver automatisk ukentlig opptak'))}</span>
        <input type="checkbox" id="ob-on" checked style="width:auto;cursor:pointer">
      </label>
      <div id="ob-sched-fields">
        <div class="ob-field-row ob-field-row--col">
          <span class="ob-field-lbl">${esc(t('onboarding.schedDay', 'Dag'))}</span>
          <div class="ob-day-picker" id="ob-day-picker">
            ${dayLabels.map((d, i) => `<button type="button" class="ob-day-btn${i === 6 ? ' sel' : ''}" data-day="${i}">${esc(d)}</button>`).join('')}
          </div>
        </div>
        <div class="ob-field-row">
          <span class="ob-field-lbl">${esc(t('onboarding.schedStartsAt', 'Gudstjenesten starter kl.'))}</span>
          <input type="time" class="form-input" id="ob-start" value="${defStart}" style="max-width:110px">
        </div>
        <div class="ob-field-row">
          <span class="ob-field-lbl">${esc(t('onboarding.schedDuration', 'Varer ca.'))}</span>
          <div style="display:flex;align-items:center;gap:6px">
            <input type="number" class="form-input" id="ob-dur" min="15" max="360" value="${defDur}" style="max-width:80px;text-align:right">
            <span style="color:var(--text3);font-size:13px">${esc(t('onboarding.schedMinutes', 'minutter'))}</span>
          </div>
        </div>
      </div>
    </div>
    <div class="ob-actions">
      <button class="btn-primary" id="ob-n4" style="justify-content:center">${esc(t('onboarding.schedFinish', 'Fullfør oppsett →'))}</button>
      <button class="ob-text-btn" id="ob-s4">${esc(t('onboarding.schedSkip', 'Hopp over — legg til manuelt under Planlegging'))}</button>
      ${backButton(4)}
    </div>`

  enhanceTimeInput(document.getElementById('ob-start') as HTMLInputElement | null)

  const chk    = document.getElementById('ob-on') as HTMLInputElement
  const fields = document.getElementById('ob-sched-fields')!
  const syncFields = () => {
    fields.style.opacity       = chk.checked ? '1' : '.4'
    fields.style.pointerEvents = chk.checked ? '' : 'none'
  }
  syncFields()
  chk.addEventListener('change', syncFields)

  document.getElementById('ob-day-picker')?.querySelectorAll<HTMLElement>('.ob-day-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('#ob-day-picker .ob-day-btn').forEach(b => b.classList.remove('sel'))
      btn.classList.add('sel')
      selectedDay = +(btn.dataset.day ?? '6')
      const slot   = getSlotForDay(selectedDay)
      const startIn = document.getElementById('ob-start') as HTMLInputElement
      const durIn   = document.getElementById('ob-dur')   as HTMLInputElement
      if (slot) {
        startIn.value = slot.start
        const [sh, sm] = slot.start.split(':').map(Number)
        const [eh, em] = slot.stop.split(':').map(Number)
        durIn.value = String((eh * 60 + em) - (sh * 60 + sm))
      } else {
        startIn.value = '11:00'
        durIn.value   = '90'
      }
    })
  })

  const save = () => {
    if (chk.checked) {
      const startVal = (document.getElementById('ob-start') as HTMLInputElement)?.value ?? '11:00'
      const durMin   = Math.max(15, parseInt((document.getElementById('ob-dur') as HTMLInputElement)?.value ?? '90') || 90)
      const [sh, sm] = startVal.split(':').map(Number)
      const endMin   = sh * 60 + sm + durMin
      const stopVal  = `${String(Math.floor(endMin / 60)).padStart(2, '0')}:${String(endMin % 60).padStart(2, '0')}`

      // MERGE, never replace. This used to filter out every existing slot on
      // the chosen weekday before pushing the new one — so a church with a
      // morning AND an evening service on Sunday lost the evening the moment
      // anyone reopened the wizard from Innstillinger («Åpne oppstartsveileder»
      // is offered as a harmless refresher). Onboarding must never delete a
      // schedule the user built by hand.
      const existing  = settings.slots ?? []
      const duplicate = existing.some(
        s => s.days.length === 1 && s.days[0] === selectedDay && s.start === startVal && s.stop === stopVal,
      )
      if (!duplicate) {
        patchSettings({ slots: [...existing, { days: [selectedDay], start: startVal, stop: stopVal }] })
        void window.api.saveSettings(settings)
      }
    }
    goTo(5)
  }

  document.getElementById('ob-n4')?.addEventListener('click', save)
  document.getElementById('ob-s4')?.addEventListener('click', () => goTo(5))
  wireBack(4)
}

// ── Done ──────────────────────────────────────────────────────────
function sDone(body: HTMLElement): void {
  body.innerHTML = `
    <div class="ob-done-ring">
      <svg viewBox="0 0 24 24"><path fill-rule="evenodd" d="M16.707 8.293a1 1 0 010 1.414l-6 6a1 1 0 01-1.414 0l-3-3a1 1 0 111.414-1.414L10 13.586l5.293-5.293a1 1 0 011.414 0z" clip-rule="evenodd"/></svg>
    </div>
    <h2 class="ob-title">${esc(t('onboarding.doneTitle', 'Alt er klart!'))}</h2>
    <p class="ob-desc">${esc(t('onboarding.doneDesc', 'SundayRec er klar til å ta opp gudstjenester. Du kan endre alle innstillinger i menyen når som helst.'))}</p>
    <div class="ob-tips">
      <div class="ob-tip"><strong>${esc(t('onboarding.tipHomeTitle', 'Hjem'))}</strong> — ${esc(t('onboarding.tipHomeText', 'Sjekk status, kjør test-opptak, og se neste planlagte opptak'))}</div>
      <div class="ob-tip"><strong>${esc(t('onboarding.tipScheduleTitle', 'Tidsplan'))}</strong> — ${esc(t('onboarding.tipScheduleText', 'Legg til ukentlige opptak og spesialdager (jul, påske)'))}</div>
      <div class="ob-tip"><strong>${esc(t('onboarding.tipAudioTitle', 'Innstillinger → Lyd'))}</strong> — ${esc(t('onboarding.tipAudioText', 'Bytt mikser, juster volum og kjør test-opptak'))}</div>
      <div class="ob-tip"><strong>${esc(t('onboarding.tipPublishTitle', 'Innstillinger → Deling'))}</strong> <em>${esc(t('onboarding.tipPublishOptional', '(valgfritt)'))}</em> — ${esc(t('onboarding.tipPublishText', 'Koble til Google Drive / Dropbox / OneDrive for automatisk sky-backup'))}</div>
    </div>
    <div class="ob-actions">
      <button class="btn-primary" id="ob-done" style="justify-content:center">${esc(t('onboarding.doneOpen', 'Åpne SundayRec →'))}</button>
    </div>`
  document.getElementById('ob-done')?.addEventListener('click', finish)
}

function showOnboardingError(msg: string): void {
  // Find or create an error element inside ob-body
  const body = document.getElementById('ob-body')
  if (!body) return
  let errEl = body.querySelector<HTMLElement>('.ob-error')
  if (!errEl) {
    errEl = document.createElement('div')
    errEl.className = 'ob-error'
    errEl.style.cssText = 'color:var(--red);font-size:13px;padding:6px 0 2px;text-align:center'
    // Insert before the actions div
    const actions = body.querySelector('.ob-actions')
    if (actions) body.insertBefore(errEl, actions)
    else body.appendChild(errEl)
  }
  errEl.textContent = msg
  setTimeout(() => { if (errEl) errEl.textContent = '' }, 4000)
}

function esc(s: unknown): string {
  return String(s ?? '').replace(/[&<>"']/g, m =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m] ?? m))
}
