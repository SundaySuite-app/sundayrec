import { t, loadLocale, currentLang } from '../i18n'
import { settings, patchSettings } from '../state'
import { setVal } from '../helpers'
import { confirmDialog } from '../ui/dialog'
import { toast } from '../ui/toast'
import { openModal } from '../ui/modal-manager'
import {
  bindSetting,
  resyncBoundSettings,
  showSavedChip,
  type BindSettingOpts,
} from '../ui/bind-setting'
import { clearFieldErrors, setFieldError } from '../ui/field-error'
import { applyFeatureGate } from '../ui/feature-gate'
import { attachProgress, type ProgressHandle } from '../ui/progress'
import {
  canSendTestEmail,
  emailBlockReason,
  emailGateStatus,
  type EmailFacts,
} from '../ui/feature-gate-core'
import {
  hostOf,
  needsLocalConfirmation,
  nextAllowLocal,
  webhookUrlError,
} from '../ui/webhook-url-core'
import {
  applyParams,
  buildLearningSummaryView,
  buildLocalNudgeView,
  MAX_BOUNDARY_NUDGE_SEC,
  MIN_CORRECTIONS_FOR_NUDGE,
  type CopyLine,
} from './learning-summary-core'
import {
  AUTO_UPDATE_INTERVAL_MS,
  autoUpdateEnabled,
  planAutoUpdateSchedule,
} from './auto-update-schedule-core'

/** Every auto-applying System/Varsler control writes the same way. */
function generalBinding(extra: Partial<BindSettingOpts> = {}): BindSettingOpts {
  return { apply: () => collectGeneralSettings(), ...extra }
}

export function setupGeneralPage(): void {
  // AUTO-APPLY everywhere except the SMTP server card, which keeps an explicit
  // Lagre/Avbryt: a half-typed mail host that auto-saved would be a broken
  // alert path with no sign that anything was wrong.
  bindSetting('language-select', generalBinding({
    key: 'language',
    // The old hint said the change takes effect "after you press Lagre". There
    // is no Lagre any more, and there does not need to be — switch now.
    after: (value) => { if (value !== currentLang) void loadLocale(String(value)) },
  }))
  bindSetting('church-name',        generalBinding({ key: 'churchName' }))
  bindSetting('responsible-person', generalBinding({ key: 'responsiblePerson' }))

  bindSetting('opt-notify-start',     generalBinding({ key: 'notifyStart' }))
  bindSetting('opt-notify-stop',      generalBinding({ key: 'notifyStop' }))
  bindSetting('opt-reminder-minutes', generalBinding({ key: 'reminderMinutes' }))
  bindSetting('opt-email-error', generalBinding({
    key: 'emailOnError',
    after: () => { toggleEmailSection(); void refreshEmailGate() },
  }))
  bindSetting('email-address', generalBinding({
    key: 'emailAddress',
    validate: (value) => {
      const v = String(value ?? '').trim()
      if (!v) return null
      return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v)
        ? null
        : t('notify.errEmail', 'Skriv en gyldig e-postadresse, f.eks. navn@kirke.no')
    },
    // The recipient is half of what «Test e-post» needs; typing one must light
    // the button up without a tab reload.
    after: () => { void refreshEmailGate() },
  }))
  // The webhook URL carries an SSRF decision, so it is the one text field whose
  // save can ask a question. A LAN address (192.168.x, printer.local, a bare
  // hostname) is refused by the backend unless `webhookAllowLocal` is set, and
  // that flag is set HERE and only here — after the operator has confirmed, for
  // this address, that it is a device on their own network. Typing a public URL
  // clears it again: it is an opt-in for one address, not a mode.
  bindSetting('webhook-url', generalBinding({
    key: 'webhookUrl',
    validate: (value) => {
      const err = webhookUrlError(String(value ?? ''))
      return err ? t(err.key, err.fallback) : null
    },
    confirmIf: (value) => {
      const url = String(value ?? '').trim()
      if (!needsLocalConfirmation(url, !!settings.webhookAllowLocal)) return null
      return {
        title: t('notify.webhookLocalTitle', 'Denne adressen er på ditt lokale nett'),
        message: t(
          'notify.webhookLocalBody',
          'SundayRec sender varsler til {host}, som ligger på ditt eget nettverk og ikke på internett. Tillat det bare hvis du vet hvilket utstyr som svarer der.',
        ).replace('{host}', hostOf(url) ?? url),
        confirmLabel: t('notify.webhookLocalAllow', 'Ja, tillat lokalt nett'),
        cancelLabel: t('notify.webhookLocalDeny', 'Avbryt'),
      }
    },
    // Reached only when the guard above was absent or answered yes.
    apply: (value) => {
      const url = String(value ?? '').trim()
      patchSettings({ webhookAllowLocal: nextAllowLocal(url, true) })
      collectGeneralSettings()
    },
  }))
  bindSetting('opt-webhook-on-warn', generalBinding({ key: 'webhookOnWarn' }))

  bindSetting('opt-autostart',       generalBinding({ key: 'launchAtLogin' }))
  bindSetting('opt-ask-open-editor', generalBinding({ key: 'askOpenEditor' }))
  // The only auto-applying control whose effect must not wait for the save to
  // land. It is a promise to stop contacting a server, and PRIVACY.md makes
  // that promise without conditions — an `after` hook runs only on a successful
  // persist, so a failed write would leave the hourly check alive underneath a
  // switch that reads «av».
  bindSetting('opt-auto-update', generalBinding({
    key: 'autoUpdate',
    apply: () => { collectGeneralSettings(); applyAutoUpdateSchedule() },
  }))
  // Moving TO beta asks first; moving back to stable never does. Beta is the
  // ring that finds what QA did not, which is another way of saying it is the
  // ring where a version breaks — and the machine reading this may be the one
  // that records Sunday. Declining leaves the select where it was
  // (bindSetting's default revert restores the previous value).
  bindSetting('opt-update-channel', generalBinding({
    key: 'updateChannel',
    confirmIf: (value) => {
      if (String(value) !== 'beta') return null
      return {
        title: t('general.updateChannelBetaTitle', 'Bytte til beta-oppdateringer?'),
        message: t(
          'general.updateChannelBetaBody',
          'Beta-versjoner kommer til deg før de er prøvd på en ekte gudstjeneste. De kan inneholde feil som ikke er funnet ennå — også feil som ødelegger et opptak. Ikke bruk beta på maskinen som tar opp gudstjenesten, med mindre du er forberedt på å håndtere det. Du kan bytte tilbake til Stabil når som helst.',
        ),
        confirmLabel: t('general.updateChannelBetaConfirm', 'Ja, bruk beta'),
        cancelLabel: t('general.updateChannelBetaCancel', 'Bli på stabil'),
      }
    },
    after: () => paintActiveUpdateChannel(),
  }))

  setupSmtpCard()
  void refreshEmailGate()
  setupWebhookTest()
  setupTelemetryCard()
  setupLearningSummaryCard()
  setupLocalNudgeCard()

  document.getElementById('btn-show-onboarding')?.addEventListener('click', () => window.showOnboarding())

  // «Vis logg» / «Kopier siste logg» (E2.6). Both talk to the rotating file
  // log at <app-data>/logs/sundayrec.log (src-tauri/src/logfile.rs) — a
  // support artifact with no audio and no passwords in it, so there is
  // nothing here to confirm or gate before sending it to a screen.
  document.getElementById('btn-show-log')?.addEventListener('click', async () => {
    const ok = await window.api.logsReveal()
    if (!ok) toast('error', t('general.showLogFailed', 'Kunne ikke åpne loggmappen.'))
  })
  document.getElementById('btn-copy-log')?.addEventListener('click', async () => {
    try {
      // 200 KB is plenty for a support conversation; the backend clamps to
      // 512 KB (logfile::TAIL_MAX_BYTES) regardless of what is asked for.
      const text = await window.api.logsTail(200 * 1024)
      if (!text) { toast('info', t('general.logEmpty', 'Loggen er tom ennå.')); return }
      await navigator.clipboard.writeText(text)
      toast('success', t('general.logCopied', 'Logg kopiert'))
    } catch {
      toast('error', t('general.logCopyFailed', 'Kunne ikke kopiere loggen.'))
    }
  })

  // Steinberg ASIO attribution is required by the ASIO SDK licence and is only
  // relevant in the Windows build (the only build compiled with ASIO support).
  // Reveal the card on Windows; it stays hidden on macOS.
  if (/win/i.test(navigator.userAgent)) {
    const asioCard = document.getElementById('asio-attribution-card')
    if (asioCard) asioCard.style.display = ''
  }

  // «Lagre i nøkkelring» — the ONLY way the SMTP password reaches durable
  // storage. It goes to the OS keychain (macOS Keychain / Windows Credential
  // Manager), never to localStorage and never into the settings blob.
  document.getElementById('btn-save-smtp-pass')?.addEventListener('click', () => void saveSmtpPassword())

  document.getElementById('btn-clear-smtp-pass')?.addEventListener('click', async () => {
    const ok = await window.api.clearSmtpPassword().catch(() => false)
    if (!ok) { toast('error', t('notify.passClearFailed', 'Kunne ikke fjerne passordet fra nøkkelringen.')); return }
    const passInput = document.getElementById('email-pass') as HTMLInputElement | null
    if (passInput) passInput.value = ''
    toast('success', t('notify.passCleared', 'Passordet er fjernet fra nøkkelringen.'))
    // The stored password was half of the transport — re-check what the card
    // may still claim it can do.
    await refreshEmailGate()
  })

  // The Gmail-OAuth card (btn-email-gmail-connect/-disconnect) was REMOVED
  // from the markup in 2026-08: gmailConnect was a permanent-failure stub with
  // no `ok` field, so the connect button sat permanently disabled and the
  // disconnect path could never be reached. The honest gate is no dead button
  // at all — re-add the card together with the backend if Gmail OAuth is ever
  // built. (`EmailFacts.gmailConnected` below still reads the REAL
  // email_status answer, so a future backend lights the send path up without
  // renderer changes.)

  // Note: btn-export / btn-import / btn-restore handlers were removed in v4.31
  // when the System tab was simplified. The corresponding shim methods
  // (exportProfile / importProfile / resetSettings) had zero callers left after
  // that and were deleted from api-shim.ts + the window.api type (2026-08
  // audit) — re-add both the UI and the shim method together if this ever
  // comes back.

  // btn-test-email / btn-test-webhook have no working backend yet (2026-08
  // audit: testEmail/testWebhook were permanent-failure stubs, so every click
  // showed a fake "✕ Sending feilet" no matter what the user configured) — both
  // are disabled in the markup with an honest reason instead of wiring up a
  // guaranteed failure.

  // DELIBERATELY ungated by `autoUpdate`, and PRIVACY.md says so out loud: «Det
  // ene unntaket er om du selv trykker "Se etter oppdateringer nå", for da er
  // det du som har bedt om det.» What the toggle switches off is the app
  // contacting the server on its own; a press of this button is the operator
  // asking the question, and refusing to answer it would be a different broken
  // promise. Do not "fix" this by adding a gate.
  document.getElementById('btn-check-updates')?.addEventListener('click', async () => {
    setUpdateStatus('pending', t('update.checking', 'Sjekker etter oppdateringer…'))
    await window.api.checkForUpdates()
  })

  document.getElementById('btn-toast-install')?.addEventListener('click', () => window.api.installUpdate())
  document.getElementById('btn-restart-install')?.addEventListener('click', () => window.api.installUpdate())
  document.getElementById('btn-toast-close')?.addEventListener('click', () => {
    const toast = document.getElementById('update-toast')
    if (toast) toast.style.display = 'none'
  })

  // "Rediger — standardklipp"-kortet er fjernet i v4.31 — intro/outro settes
  // i editor-fanen og lagres direkte til Settings.editorIntroPath/OutroPath
  // derfra. updateEditorClipUI()-kallet under er en no-op nå men beholdes
  // som safe-shim for tilfelle ekstern kode trigger applyGeneralSettingsToUI.

  wireUpdateIpcListeners()
}

/** The password field, if the Varsler tab is in the DOM. */
function passField(): HTMLInputElement | null {
  return document.getElementById('email-pass') as HTMLInputElement | null
}

/** Monotonic ticket so only the newest `refreshEmailGate` run paints. */
let emailGateSeq = 0

/**
 * Paint the password row from the REAL keychain state.
 *
 * The old code read `settings.emailSmtpPassSet`, an Electron-era flag the Tauri
 * backend never set, so the row could never show "saved" and «Fjern» was
 * permanently hidden.
 */
function paintSmtpPasswordState(stored: boolean): void {
  const input = passField()
  const clearBtn = document.getElementById('btn-clear-smtp-pass') as HTMLElement | null
  const storedHint = document.getElementById('email-pass-state') as HTMLElement | null
  if (input) {
    input.placeholder = stored ? t('notify.smtpPassSaved', '••••••••  (lagret)') : ''
  }
  if (clearBtn) clearBtn.style.display = stored ? 'inline' : 'none'
  if (storedHint) storedHint.style.display = stored ? '' : 'none'
}

/** Write the typed password to the OS keychain. Shared by the «Lagre i
 *  nøkkelring» button and the SMTP card's «Lagre» — a user who types a password
 *  and presses the card's save button must not have it silently dropped, which
 *  is exactly what happened before (the shim stripped it on the way to storage
 *  and no command could store it). Returns false only on a real failure. */
async function saveSmtpPassword(): Promise<boolean> {
  const input = passField()
  const value = input?.value ?? ''
  if (!value.trim()) {
    toast('error', t('notify.passEmpty', 'Skriv inn passordet først.'))
    return false
  }
  try {
    await window.api.emailSetSmtpPassword(value)
  } catch {
    toast('error', t('notify.passSaveFailed', 'Kunne ikke lagre passordet i nøkkelringen.'))
    return false
  }
  // Never keep the plaintext in the DOM once it is safely in the keychain.
  if (input) input.value = ''
  toast('success', t('notify.passSaved', 'Passordet er lagret i nøkkelringen.'))
  await refreshEmailGate()
  return true
}

/**
 * The e-mail card — driven by the REAL `email_status` + keychain state.
 *
 * History: the send path sat behind a cargo feature that shipped in no release,
 * so this card honestly said «Ikke tilgjengelig». The feature is in `default`
 * now, so the card is live — and being live is not cosmetic: `applyFeatureGate`
 * makes every gated section `inert`, and this section's controls are the
 * recipient field, the SMTP fields and the enable toggle. Gating it "until it
 * is configured" would have made it impossible to configure. So the gate now
 * fires ONLY for a build that genuinely cannot send, and "not set up yet" is a
 * hint line next to «Test e-post» that leaves everything usable.
 */
async function refreshEmailGate(): Promise<void> {
  // Several things ask for a refresh (page setup, applying settings, every
  // `after:` hook, saving the password), so two runs can easily overlap. Read
  // ALL the async facts first, drop the result if a newer run has started, and
  // paint in one synchronous block — otherwise two interleaved runs each paint
  // half the card and it ends up describing a state that never existed. (Seen
  // for real: the password row said "not stored" while the hint line said the
  // transport was ready.)
  const seq = ++emailGateSeq
  const [stored, status] = await Promise.all([
    window.api.emailHasSmtpPassword().catch(() => false),
    // A failed status check is not a send path — fall back to the pessimistic
    // answer rather than to the previous one.
    window.api.emailStatus().catch(() => ({ featureBuilt: false, gmailConnected: false })),
  ])
  if (seq !== emailGateSeq) return

  paintSmtpPasswordState(stored)
  const facts: EmailFacts = {
    featureBuilt: !!status.featureBuilt,
    gmailConnected: !!status.gmailConnected,
    smtpConfigured: !!(settings.emailSmtp ?? '').trim() && !!(settings.emailSmtpUser ?? '').trim(),
    // A password typed but not yet saved still authenticates: the backend
    // prefers the request's over the keychain's (`resolve_smtp_password`).
    smtpPasswordAvailable: stored || !!passField()?.value.trim(),
  }

  applyFeatureGate('email-notify-card', {
    status: emailGateStatus(facts),
    chipText: t('gate.chipUnavailable', 'Ikke tilgjengelig'),
    explanation: t('notify.gateNoFeature', 'E-postutsending er ikke bygget inn i denne versjonen. Varsler om feilede opptak må hentes fra Hjem-siden eller en webhook inntil videre.'),
  })

  // «Send test» is enabled ONLY when a transport exists and we know where to
  // send it. The reason it is not is stated out loud rather than left as a
  // tooltip on a dead button.
  const recipient = (document.getElementById('email-address') as HTMLInputElement | null)?.value.trim()
    ?? settings.emailAddress ?? ''
  const reason = emailBlockReason(facts, !!recipient)
  const hintText =
    reason === 'noTransport'
      ? t('notify.gateNoTransport', 'Ingen sendemetode er satt opp ennå. Fyll ut SMTP-feltene under «Oppsett av e-postserver (SMTP)».')
      : reason === 'noRecipient'
        ? t('notify.gateTestNoRecipient', 'Skriv inn en mottakeradresse først.')
        : ''
  const hint = document.getElementById('email-status-hint')
  if (hint) {
    hint.textContent = hintText
    hint.style.display = hintText ? '' : 'none'
  }

  const testBtn = document.getElementById('btn-test-email') as HTMLButtonElement | null
  if (testBtn) {
    const allowed = canSendTestEmail(facts, !!recipient)
    testBtn.disabled = !allowed
    testBtn.title = allowed ? '' : hintText
    if (!testBtn.dataset.wired) {
      testBtn.dataset.wired = '1'
      testBtn.addEventListener('click', () => void sendTestEmail())
    }
  }
}

/** Backend error codes worth translating instead of showing raw. Anything else
 *  is a server/network message the user genuinely needs to read verbatim. */
function emailErrorText(err: string | undefined): string {
  if (err?.includes('missing_password')) {
    return t('notify.errNoPassword', 'Mangler SMTP-passord. Skriv det inn og trykk «Lagre i nøkkelring».')
  }
  if (err?.includes('feature_disabled')) {
    return t('notify.gateNoFeature', 'E-postutsending er ikke bygget inn i denne versjonen. Varsler om feilede opptak må hentes fra Hjem-siden eller en webhook inntil videre.')
  }
  if (err?.includes('no_config: smtp host')) {
    return t('notify.errSmtpHost', 'Skriv et servernavn, f.eks. smtp.gmail.com')
  }
  return err ?? '—'
}

/** Send the backend's localized "email works" message through whichever
 *  transport is actually available. Only reachable when the gate says ok. */
async function sendTestEmail(): Promise<void> {
  const btn = document.getElementById('btn-test-email') as HTMLButtonElement | null
  const recipient = (document.getElementById('email-address') as HTMLInputElement | null)?.value.trim() ?? ''
  if (!recipient) return
  const useGmail = !(settings.emailSmtp ?? '').trim()
  if (btn) btn.disabled = true
  try {
    const res = await window.api.testEmail({
      transport: useGmail ? 'gmail' : 'smtp',
      recipient,
      language: settings.language ?? 'no',
      host: settings.emailSmtp,
      port: settings.emailSmtpPort,
      user: settings.emailSmtpUser,
      // Send the typed password only when there IS one; `undefined` lets the
      // backend fall back to the keychain, which is the normal case (the field
      // is emptied as soon as the password is stored).
      pass: passField()?.value.trim() ? passField()?.value : undefined,
      // Likewise for the sender: blank means "derive it" backend-side, which
      // reproduces the old `emailSmtpUser || recipient` exactly.
      from: (settings.emailSmtpFrom ?? '').trim() || undefined,
    })
    toast(res.ok ? 'success' : 'error',
      res.ok
        ? t('notify.testEmailSent', 'Test-e-post sendt til {to}').replace('{to}', recipient)
        : t('notify.testEmailFailed', 'Test-e-posten gikk ikke gjennom: {err}').replace('{err}', emailErrorText(res.error)))
  } finally {
    if (btn) btn.disabled = false
  }
}

/**
 * The webhook test is NOT gated: `email_test_webhook` is a real command on
 * every build (a plain 10 s POST, no cargo feature). The night sweep disabled
 * the button along with the e-mail ones because the shim stubbed it; the stub
 * is gone, so the button works — it just needs a URL to aim at.
 */
function setupWebhookTest(): void {
  const btn = document.getElementById('btn-test-webhook') as HTMLButtonElement | null
  const urlEl = document.getElementById('webhook-url') as HTMLInputElement | null
  if (!btn || !urlEl) return

  const sync = (): void => {
    const url = urlEl.value.trim()
    btn.disabled = !/^https:\/\/\S+$/.test(url)
    btn.title = btn.disabled ? t('notify.gateWebhookNoUrl', 'Lim inn en webhook-URL først.') : ''
  }
  urlEl.addEventListener('input', sync)
  urlEl.addEventListener('change', sync)
  sync()

  btn.addEventListener('click', async () => {
    const url = urlEl.value.trim()
    if (!url) return
    btn.disabled = true
    try {
      const res = await window.api.testWebhook(url)
      toast(res.ok ? 'success' : 'error',
        res.ok
          ? t('notify.testWebhookOk', 'Webhooken svarte — varsler kommer fram.')
          : t('notify.testWebhookFailed', 'Webhooken svarte ikke. Sjekk adressen.'))
    } finally {
      sync()
    }
  })
}

/**
 * The SMTP server card — one of the three EXPLICIT-save exceptions.
 *
 * Host, username, sender and password are a set: applying them one keystroke at
 * a time would leave the alert path pointing at a half-typed server, and the
 * failure would only show up the day a recording actually fails. So this card
 * validates as a unit and writes on «Lagre»; «Avbryt» puts the stored values
 * back. The password takes a different route from the rest — the OS keychain,
 * not the settings blob — but «Lagre» still flushes it, because a user who
 * types a password and presses Lagre must not have it silently discarded.
 */
function setupSmtpCard(): void {
  const host = document.getElementById('email-smtp') as HTMLInputElement | null
  const user = document.getElementById('email-user') as HTMLInputElement | null
  const from = document.getElementById('email-from') as HTMLInputElement | null
  const pass = document.getElementById('email-pass') as HTMLInputElement | null
  const saveBtn   = document.getElementById('btn-smtp-save')
  const cancelBtn = document.getElementById('btn-smtp-cancel')
  if (!host || !saveBtn) return

  // A typed password counts as a transport even before it is saved, so the test
  // button must react to it — but only on the EDGE (empty ↔ filled), since a
  // full refresh costs two IPC round-trips and this fires per keystroke.
  let passWasFilled = false
  pass?.addEventListener('input', () => {
    const filled = !!pass.value.trim()
    if (filled === passWasFilled) return
    passWasFilled = filled
    void refreshEmailGate()
  })

  saveBtn.addEventListener('click', async () => {
    clearFieldErrors(document.getElementById('email-smtp-advanced'))
    const hostVal = host.value.trim()
    const userVal = user?.value.trim() ?? ''
    const fromVal = from?.value.trim() ?? ''
    // An empty card is a valid state: it means "no SMTP, use Gmail".
    if (hostVal && !/^[\w.-]+\.[a-z]{2,}$/i.test(hostVal)) {
      setFieldError(host, t('notify.errSmtpHost', 'Skriv et servernavn, f.eks. smtp.gmail.com'))
      return
    }
    if (hostVal && !userVal) {
      setFieldError(user, t('notify.errSmtpUser', 'Brukernavnet er e-postadressen du sender fra'))
      return
    }
    if (fromVal && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(fromVal)) {
      setFieldError(from, t('notify.errEmail', 'Skriv en gyldig e-postadresse, f.eks. navn@kirke.no'))
      return
    }
    patchSettings({
      emailSmtp:     hostVal,
      emailSmtpUser: userVal,
      emailSmtpFrom: fromVal,
      // Runtime-only mirror (stripped before persistence) — the real password
      // goes to the keychain just below.
      emailSmtpPass: pass?.value ?? '',
      emailSmtpPort: +((document.getElementById('email-port') as HTMLInputElement | null)?.value ?? 587),
    })
    const ok = await window.api.saveSettings(settings).catch(() => false)
    if (!ok) { toast('error', t('general.saveFailed', 'Kunne ikke lagre innstillingen')); return }
    // Flush a typed password to the keychain as part of the same set. Its own
    // toast covers the outcome, so only report the field save when there was
    // nothing to flush.
    if (pass?.value.trim()) await saveSmtpPassword()
    else showSavedChip(saveBtn.parentElement)
    if (pass) pass.value = ''
    // A newly-filled SMTP server can flip «Test e-post» from blocked to
    // pressable — re-check rather than making the user reopen the tab.
    void refreshEmailGate()
  })

  cancelBtn?.addEventListener('click', () => {
    clearFieldErrors(document.getElementById('email-smtp-advanced'))
    setVal('email-smtp', settings.emailSmtp ?? '')
    setVal('email-user', settings.emailSmtpUser ?? '')
    setVal('email-from', settings.emailSmtpFrom ?? '')
    setVal('email-port', settings.emailSmtpPort ?? 587)
    // «Avbryt» discards the TYPED password only. A password already in the
    // keychain is not touched — that is what «Fjern» is for.
    if (pass) pass.value = ''
  })
}

/**
 * «Personvern og diagnostikk» — the opt-in telemetry card (E3.7).
 *
 * Consent state lives entirely behind telemetry_consent_get/set (the
 * `app_setting` bag in src-tauri — see crates/sundayrec-core/telemetry/
 * consent.rs), NOT in the `Settings` blob `bindSetting` normally reads and
 * writes through `collectGeneralSettings()`. So the toggle below uses the
 * same `apply: () => {}` / `persist: async () => …` shape
 * integrations-page.ts already established for exactly this situation (a
 * control whose truth lives outside Settings).
 *
 * No `confirmIf` on enabling: the toggle's own description plus the details
 * this card links to (the preview, the privacy notice) already give informed
 * consent, and layering a second "are you sure?" on top of a purely opt-IN
 * action would be friction with no matching friction on the way back out —
 * which is itself a shape of dark pattern this feature is trying to avoid.
 */
function setupTelemetryCard(): void {
  void refreshTelemetryCard()

  bindSetting('opt-telemetry-consent', {
    key: 'telemetryConsent',
    apply: () => {},
    persist: async () => {
      const checked = (document.getElementById('opt-telemetry-consent') as HTMLInputElement | null)?.checked ?? false
      const result = await window.api.telemetryConsentSet(checked)
      if (!result) return false
      void refreshTelemetryCard()
      return true
    },
  })

  document.getElementById('btn-telemetry-preview')?.addEventListener('click', () => void showTelemetryPreview())
  document.getElementById('btn-telemetry-delete')?.addEventListener('click', () => void deleteTelemetryData())
  document.getElementById('telemetry-privacy-link')?.addEventListener('click', e => {
    e.preventDefault()
    void window.api.openPrivacyPolicy()
  })
}

/**
 * Paint the toggle + queue line from the REAL backend state, and rebase
 * `bindSetting`'s baseline for the toggle to match.
 *
 * That last part matters: `bindSetting` captures its "previous value"
 * baseline synchronously when `setupTelemetryCard()` calls it, which is
 * BEFORE this async read can possibly resolve — so without
 * `resyncBoundSettings()` here, a consent that was already granted in an
 * earlier session would paint the checkbox as checked while the binder still
 * believed the baseline was unchecked. The very next click (unchecking it)
 * would then look like "no real change" and silently not persist. Mirrors
 * exactly what `applyGeneralSettingsToUI()` does after its own `setCheckbox`
 * calls, for the same reason.
 */
async function refreshTelemetryCard(): Promise<void> {
  const toggle = document.getElementById('opt-telemetry-consent') as HTMLInputElement | null
  const consent = await window.api.telemetryConsentGet().catch(() => null)
  if (toggle) toggle.checked = !!consent?.active
  resyncBoundSettings()

  const statusEl = document.getElementById('telemetry-queue-status')
  if (!statusEl) return
  if (!consent?.active) { statusEl.style.display = 'none'; return }

  const q = await window.api.telemetryQueueStatus().catch(() => null)
  const parts: string[] = []
  if (q && q.pending > 0) {
    parts.push(q.pending === 1
      ? t('general.telemetryQueuePendingOne', '1 rapport venter på å bli sendt.')
      : t('general.telemetryQueuePendingMany', '{n} rapporter venter på å bli sendt.').replace('{n}', String(q.pending)))
  }
  if (q && q.failed > 0) {
    parts.push(t('general.telemetryQueueFailed', '{n} kunne ikke sendes.').replace('{n}', String(q.failed)))
  }
  if (q?.oldestAt) {
    parts.push(t('general.telemetryQueueOldestSince', 'Eldste er fra {date}.').replace('{date}', new Date(q.oldestAt).toLocaleDateString(currentLang)))
  }
  if (q?.lastError) {
    parts.push(t('general.telemetryQueueLastError', 'Siste feil: {error}').replace('{error}', q.lastError))
  }
  statusEl.textContent = parts.join(' ')
  statusEl.style.display = parts.length ? '' : 'none'
}

/**
 * «Vis hva som sendes» — the transparency affordance the privacy text
 * promises. Opens the modal FIRST (with a placeholder), then fills it — the
 * preview is a real IPC round-trip, and an instant open with "…" reads better
 * than a click that visibly does nothing for a beat.
 *
 * A failed fetch shows an honest error, never a fabricated payload: the whole
 * point of this surface is that what it displays is real (see
 * telemetry_preview_payload's own doc comment in
 * src-tauri/src/commands/telemetry.rs).
 */
async function showTelemetryPreview(): Promise<void> {
  const body = document.getElementById('telemetry-preview-body')
  const hint = document.getElementById('telemetry-preview-hint')
  if (!body || !hint) return
  body.textContent = '…'
  hint.textContent = ''
  openModal('telemetry-preview-modal')

  const preview = await window.api.telemetryPreviewPayload().catch(() => null)
  if (!preview) {
    body.textContent = ''
    hint.textContent = t('general.telemetryPreviewFailed', 'Kunne ikke hente forhåndsvisningen.')
    return
  }
  body.textContent = preview.json
  const hints = [
    preview.isNextPayload
      ? t('general.telemetryPreviewNextHint', 'Dette er nøyaktig det som sendes neste gang.')
      : t('general.telemetryPreviewHistoryHint', 'Diagnostikk er av. Dette viser formen på dataene fra din egen historikk lokalt — ingenting sendes.'),
  ]
  if (preview.isEmpty) hints.push(t('general.telemetryPreviewEmptyHint', 'Ingenting å sende akkurat nå.'))
  hint.textContent = hints.join(' ')
}

/**
 * «Slett mine data» — regenerates the install id and clears the local
 * outbox. The confirm copy says exactly this and nothing more: today's
 * version cannot reach into Sunday Suite's server and delete rows already
 * received (that arrives in a later version), and claiming otherwise would
 * be the one thing a privacy control must never do.
 */
async function deleteTelemetryData(): Promise<void> {
  const ok = await confirmDialog({
    title: t('general.telemetryDeleteConfirmTitle', 'Slette dine data?'),
    message: t(
      'general.telemetryDeleteConfirmBody',
      'Dette bytter ut den anonyme install-IDen din med en ny, og tømmer rapporter i kø som ikke er sendt ennå — begge deler skjer lokalt, med én gang. Deretter ber SundayRec serveren om å slette alt som ligger der under den gamle IDen. Er maskinen offline, sendes forespørselen så snart den er på nett. Anonyme totaltall blir stående, siden de ikke inneholder noen install-ID og ikke kan knyttes til deg.',
    ),
    confirmLabel: t('general.telemetryDeleteConfirmYes', 'Ja, tøm og bytt ID'),
    cancelLabel: t('general.telemetryDeleteConfirmNo', 'Avbryt'),
    danger: true,
  })
  if (!ok) return

  const success = await window.api.telemetryRegenerateInstallId().catch(() => false)
  toast(success ? 'success' : 'error',
    success
      ? t('general.telemetryDeletedToast', 'Dataene er tømt lokalt, og du har fått en ny anonym ID.')
      : t('general.telemetryDeleteFailedToast', 'Kunne ikke slette dataene. Prøv igjen.'))
  if (success) void refreshTelemetryCard()
}

/**
 * «Hva appen har lagt merke til» (E8.T) — wires the load button. Nothing
 * fires on tab-open: unlike `setupTelemetryCard`'s `refreshTelemetryCard()`,
 * this card starts life as a title, two lines of static explanation and a
 * button, and stays that way until the operator asks for more.
 *
 * That is a deliberate departure from `search-page.ts`'s transcript index
 * (which DOES build itself on first tab visit): a transcript index feeds a
 * search box the operator is about to type into, so the cost is paid once,
 * right before it is needed. Nobody is about to search this card — it is a
 * "what has this thing been doing" check the operator runs on their own
 * schedule, so there is no moment where fetching it unprompted pays for
 * itself, and every moment where doing so during a live recording would not.
 */
function setupLearningSummaryCard(): void {
  document
    .getElementById('btn-learning-summary-load')
    ?.addEventListener('click', () => void loadLearningSummary())
}

/** Re-entrancy guard — a second click while the first round-trip is still in
 *  flight must not fire a second one (or paint over its own "Henter…"). */
let learningSummaryLoading = false

async function loadLearningSummary(): Promise<void> {
  if (learningSummaryLoading) return
  const statusEl = document.getElementById('learning-summary-status')
  const emptyEl = document.getElementById('learning-summary-empty')
  const bodyEl = document.getElementById('learning-summary-body')
  if (!statusEl || !emptyEl || !bodyEl) return

  // The read this triggers walks every recording's `<stem>.feedback.json`
  // sidecar off disk. Fine at rest; not something to run while a service is
  // being captured, so the button stays clickable (a volunteer mid-recording
  // should not have to wonder why a whole card vanished) but the read itself
  // never happens — checked HERE, at the moment of the click, rather than by
  // disabling the button on a recording-start event, so a recording that
  // starts between page-load and click is still caught.
  if (window.__isRecording) {
    bodyEl.style.display = 'none'
    emptyEl.style.display = 'none'
    statusEl.textContent = t('general.learningUnavailableRecording', 'Ikke tilgjengelig mens opptak pågår.')
    statusEl.style.display = ''
    return
  }

  learningSummaryLoading = true
  bodyEl.style.display = 'none'
  emptyEl.style.display = 'none'
  statusEl.textContent = t('general.learningLoading', 'Henter…')
  statusEl.style.display = ''

  try {
    const summary = await window.api.learningFeedbackSummary()
    if (!summary) {
      // A real IPC failure — NEVER rendered as the empty state. All-zeros is
      // a legitimate answer ("nothing corrected yet"); a failed round-trip
      // must not look like it, or a broken read reads as "you're all caught
      // up" instead of "try again".
      statusEl.textContent = t('general.learningLoadFailed', 'Kunne ikke hente oversikten. Prøv igjen.')
      return
    }
    renderLearningSummary(summary)
  } finally {
    learningSummaryLoading = false
  }
}

function renderLearningSummary(summary: import('../../bindings/LearningSummary').LearningSummary): void {
  const statusEl = document.getElementById('learning-summary-status')
  const emptyEl = document.getElementById('learning-summary-empty')
  const bodyEl = document.getElementById('learning-summary-body')
  if (!statusEl || !emptyEl || !bodyEl) return
  statusEl.style.display = 'none'

  const view = buildLearningSummaryView(summary)
  if (view.isEmpty) {
    // The common case, and the one that must not look broken or accusatory —
    // a fresh install has corrected nothing, and that is normal, not an
    // error state. `emptyEl`'s text is the static, translated
    // "nothing yet — this is normal" line baked into index.html.
    emptyEl.style.display = ''
    bodyEl.style.display = 'none'
    return
  }
  emptyEl.style.display = 'none'
  bodyEl.style.display = ''

  paintLearningLine('learning-summary-sermon-pick', view.sermonPick)
  paintLearningLine('learning-summary-start-tendency', view.startTendency)
  paintLearningLine('learning-summary-end-tendency', view.endTendency)
  paintLearningLine('learning-summary-companion', view.companion)
}

/**
 * «Hva appen har justert» (E10) — the switch, the four sentences, and the
 * reset.
 *
 * Unlike `setupLearningSummaryCard` above, this one DOES paint itself on tab
 * open (`populateGeneral` calls `refreshLocalNudgeCard`). The reason is the
 * difference in what the read costs and what the card is for: the summary walks
 * every recording's sidecar off disk and answers a question the operator came
 * looking for, while this reads one settings row and answers a question they did
 * not ask — "is this thing changing what happens on Sunday, and by how much".
 * A card that only tells you that after you press a button is a card that never
 * tells most people at all, and an invisible adjustment is the whole failure
 * this feature was built to avoid.
 */
function setupLocalNudgeCard(): void {
  bindSetting('opt-local-adaptivity', generalBinding({
    key: 'localAdaptivity',
    // Repaint AFTER the persist, not on the change event: the card describes
    // what the backend will actually do next Sunday, and a switch whose card
    // updated optimistically would claim an adjustment that a failed write left
    // un-armed.
    after: () => { void refreshLocalNudgeCard() },
  }))
  document
    .getElementById('btn-local-nudge-reset')
    ?.addEventListener('click', () => void resetLocalNudge())
}

/** Re-entrancy guard, same reasoning as `learningSummaryLoading`. */
let localNudgeLoading = false

async function refreshLocalNudgeCard(): Promise<void> {
  if (localNudgeLoading) return
  const hintEl = document.getElementById('local-nudge-toggle-hint')
  const statusEl = document.getElementById('local-nudge-status')
  if (!hintEl || !statusEl) return

  hintEl.textContent = applyParams(
    t(
      'general.localNudgeToggleHint',
      'Når dette er på, flytter appen sitt eget forslag til preken-start og -slutt etter hvordan du pleier å rette det. Aldri mer enn {limit} sekunder, og aldri før du har rettet minst {min} opptak.',
    ),
    { limit: String(MAX_BOUNDARY_NUDGE_SEC), min: String(MIN_CORRECTIONS_FOR_NUDGE) },
  )

  // Reading the nudge re-folds every recording's sidecar (see
  // `learning::refresh_nudge` for why it is a recompute and not a cached read),
  // so it inherits the learning summary's refusal above: the switch and its
  // explanation stay on screen — a volunteer mid-recording should not find a
  // card missing — but the read itself waits.
  if (window.__isRecording) {
    for (const id of ['local-nudge-headline', 'local-nudge-start', 'local-nudge-end', 'local-nudge-at-limit']) {
      const el = document.getElementById(id)
      if (el) el.style.display = 'none'
    }
    const resetBtn = document.getElementById('btn-local-nudge-reset')
    if (resetBtn) resetBtn.style.display = 'none'
    statusEl.textContent = t('general.learningUnavailableRecording', 'Ikke tilgjengelig mens opptak pågår.')
    statusEl.style.display = ''
    return
  }

  localNudgeLoading = true
  try {
    const nudge = await window.api.learningLocalNudge()
    if (!nudge) {
      // A real IPC failure, never rendered as "nothing adjusted" — the same
      // distinction the learning summary makes, and it matters more here: an
      // adjustment shown as absent is exactly the invisible state this card
      // exists to prevent.
      statusEl.textContent = t(
        'general.localNudgeLoadFailed',
        'Kunne ikke hente hva appen har justert. Prøv igjen.',
      )
      statusEl.style.display = ''
      return
    }
    statusEl.style.display = 'none'
    renderLocalNudge(nudge)
  } finally {
    localNudgeLoading = false
  }
}

function renderLocalNudge(nudge: import('../../bindings/LocalNudge').LocalNudge): void {
  const enabled = !!(document.getElementById('opt-local-adaptivity') as HTMLInputElement | null)
    ?.checked
  const view = buildLocalNudgeView(nudge, enabled)
  paintLearningLine('local-nudge-headline', view.headline)
  paintLearningLine('local-nudge-start', view.start)
  paintLearningLine('local-nudge-end', view.end)
  paintLearningLine('local-nudge-at-limit', view.atLimit)
  const resetBtn = document.getElementById('btn-local-nudge-reset')
  if (resetBtn) resetBtn.style.display = view.canReset ? '' : 'none'
}

/**
 * The one click that puts the detector back to the shipped constants.
 *
 * No confirmation dialog. Every other destructive button in this app asks first
 * because the thing it destroys cannot be rebuilt — a recording, a queue entry,
 * a login item. This one throws away a derived number that the corrections on
 * disk can produce again the moment the operator switches adaptivity back on,
 * so a dialog would be asking permission to undo something undoable, and would
 * put one more step between a confused volunteer and a recorder that behaves
 * like every other install.
 */
async function resetLocalNudge(): Promise<void> {
  const nudge = await window.api.learningLocalNudgeReset()
  if (!nudge) {
    toast('error', t('general.localNudgeLoadFailed', 'Kunne ikke hente hva appen har justert. Prøv igjen.'))
    return
  }
  // The backend cleared the flag as well as the value; the checkbox has to
  // follow, or the card would read "off" from a switch still showing "on".
  setCheckbox('opt-local-adaptivity', false)
  patchSettings({ localAdaptivity: false })
  renderLocalNudge(nudge)
  toast(
    'success',
    t(
      'general.localNudgeResetDone',
      'Nullstilt. Appen gjetter nå på nøyaktig samme måte som en nyinstallert app, og lærer ikke mer før du slår det på igjen.',
    ),
  )
}

/** One optional sentence: hidden when `line` is `null` (e.g. the trim
 *  tendency is still `Unclear`), translated + interpolated otherwise. */
function paintLearningLine(id: string, line: CopyLine | null): void {
  const el = document.getElementById(id)
  if (!el) return
  if (!line) {
    el.style.display = 'none'
    return
  }
  el.textContent = applyParams(t(line.key, line.fallback), line.params)
  el.style.display = ''
}

// Lifetime `window.api.on` subscriptions + the hourly auto-check interval —
// unsubscribes kept, wiring guarded so a re-run of setupGeneralPage can never
// stack duplicate handlers (or a second interval).
let updateIpcWired = false
const updateIpcUnsubs: Array<(() => void) | undefined> = []
function wireUpdateIpcListeners(): void {
  if (updateIpcWired) return
  updateIpcWired = true

  // Update events from the api-shim bridge. The updater flow is identical on
  // every platform now (Tauri downloads only when installUpdate is invoked) —
  // the old `_isMac` special-casing hid ALL download progress on macOS, so a
  // click on "install" looked completely dead for the whole ~40 MB download.
  const updateButtons = (): HTMLButtonElement[] =>
    ['btn-toast-install', 'btn-restart-install']
      .map(id => document.getElementById(id) as HTMLButtonElement | null)
      .filter((b): b is HTMLButtonElement => b !== null)
  const setUpdateButtons = (label: string | null, opts?: { disabled?: boolean; show?: boolean }): void => {
    for (const b of updateButtons()) {
      if (label !== null) b.textContent = label
      if (opts?.disabled !== undefined) b.disabled = opts.disabled
      if (opts?.show !== undefined) b.style.display = opts.show ? 'inline-flex' : 'none'
    }
  }
  // The update download's bar + remaining time. Attached on the first progress
  // event and torn down when the download ends, so a second round starts with a
  // fresh rate estimate rather than the previous download's.
  let updateUi: ProgressHandle | null = null
  const hideProgress = (): void => {
    updateUi?.destroy()
    updateUi = null
    const host = document.getElementById('update-progress-host')
    if (host) host.style.display = 'none'
  }

  updateIpcUnsubs.push(window.api.on('update-checking',          () => setUpdateStatus('pending', t('update.checking', 'Sjekker etter oppdateringer…'))))
  updateIpcUnsubs.push(window.api.on('update-not-available',     () => {
    // Also retire any stale install/restart button from an earlier round — a
    // leftover "Start på nytt og installer" on an up-to-date app is a button
    // that provably does nothing (rig-observed on 0.4.5).
    setUpdateStatus('ok', t('update.upToDate', 'Du er oppdatert'))
    setUpdateButtons(null, { show: false, disabled: false })
    hideProgress()
    hideToast()
  }))
  updateIpcUnsubs.push(window.api.on('update-available',         (info: unknown) => {
    const v = (info as { version: string }).version
    setUpdateStatus('ready', t('update.availableInstall', 'Versjon {v} tilgjengelig — klikk for å laste ned og installere').replace('{v}', v))
    // The label must say what the click DOES from here: download + install.
    setUpdateButtons(`↓ ${t('update.btnDownloadInstall', 'Last ned og installer')} v${v}`, { show: true, disabled: false })
    showUpdateToast(
      t('update.toastAvailableTitle', 'Oppdatering tilgjengelig'),
      t('update.toastAvailableInstall', 'Versjon {v} — klikk for å laste ned og installere').replace('{v}', v),
      true
    )
  }))
  updateIpcUnsubs.push(window.api.on('update-download-progress', (prog: unknown) => {
    const pct  = Math.round((prog as { percent?: number }).percent ?? 0)
    const host = document.getElementById('update-progress-host')
    if (host) {
      host.style.display = 'block'
      if (!updateUi) updateUi = attachProgress(host, { compact: true })
      updateUi.update(Math.max(0, Math.min(1, pct / 100)), t('update.btnDownloading', 'Laster ned…'))
    }
    setUpdateStatus('pending', t('update.downloading', 'Laster ned… {pct}%').replace('{pct}', String(pct)))
    setUpdateButtons(t('update.btnDownloading', 'Laster ned…'), { disabled: true })
    setToastProgress(pct)
  }))
  updateIpcUnsubs.push(window.api.on('update-downloaded', (info: unknown) => {
    const v = (info as { version: string }).version
    hideProgress()
    setUpdateButtons(`↺ ${t('update.btnRestartInstall', 'Start på nytt og installer')}`, { show: true, disabled: false })
    setUpdateStatus('ready', t('update.readyInstall', 'Versjon {v} er klar — start på nytt for å installere').replace('{v}', v))
    showUpdateToast(
      t('update.toastReadyTitle', 'Klar for installasjon'),
      t('update.toastReadyText', 'Versjon {v} er lastet ned').replace('{v}', v),
      true
    )
  }))
  updateIpcUnsubs.push(window.api.on('update-restarting', () => {
    setUpdateStatus('pending', t('update.restarting', 'Starter på nytt…'))
    setUpdateButtons(t('update.restarting', 'Starter på nytt…'), { disabled: true })
  }))
  updateIpcUnsubs.push(window.api.on('update-error', (msg: unknown) => {
    hideProgress()
    setUpdateButtons(null, { disabled: false })
    // The dead-man's switch in api-shim fires this when the process is still
    // alive after a relaunch request — tell the user exactly what to do
    // instead of the generic "check failed" text.
    if (msg === 'restart_failed') {
      setUpdateStatus('error', t('update.restartFailed', 'Omstarten skjedde ikke — avslutt appen og åpne den på nytt, så er oppdateringen aktiv'))
    } else {
      setUpdateStatus('error', t('update.error', 'Kunne ikke sjekke for oppdateringer'))
    }
    console.warn('Update error:', msg)
  }))

  // The schedule is NOT armed here. `setupGeneralPage` runs before
  // `loadSettings`, so `settings` is still `{}` at this point and the gate this
  // block used to carry could only ever read "on" — an operator who had switched
  // «Oppdater automatisk» off got a request to the update server on every launch
  // regardless. Arming waits for `applyGeneralSettingsToUI`, which runs as soon
  // as the persisted blob arrives; the listeners registered above are in place
  // long before that, so the synthesized update-* events still reach the UI.
}

/**
 * The hourly auto-check's interval, or null when nothing is scheduled. Lives at
 * module scope because the timer outlives the call that armed it: the toggle
 * has to be able to cancel a timer someone else started, which the old
 * fire-and-forget `setInterval` made impossible.
 */
let autoUpdateTimer: ReturnType<typeof setInterval> | null = null

/**
 * Make the update schedule match `settings.autoUpdate`, in both directions.
 *
 * PRIVACY.md: «Slår du den av, tar appen ikke kontakt med serveren — verken ved
 * oppstart eller den vanlige sjekken hver time.» That promise is about the
 * running app, not about the next launch, so this is called on every change of
 * the setting rather than once at wire-up.
 *
 * Safe to call repeatedly: `planAutoUpdateSchedule` only reports transitions, so
 * re-arming an already-armed schedule is a no-op instead of a second timer.
 */
function applyAutoUpdateSchedule(): void {
  const action = planAutoUpdateSchedule(
    autoUpdateTimer !== null,
    autoUpdateEnabled(settings.autoUpdate),
  )
  if (action.stop && autoUpdateTimer !== null) {
    clearInterval(autoUpdateTimer)
    autoUpdateTimer = null
  }
  if (action.start) {
    void window.api.checkForUpdates()
    autoUpdateTimer = setInterval(() => { void window.api.checkForUpdates() }, AUTO_UPDATE_INTERVAL_MS)
  }
}

export function applyGeneralSettingsToUI(): void {
  setVal('language-select', settings.language ?? 'no')
  setVal('church-name',        settings.churchName        ?? '')
  setVal('responsible-person', settings.responsiblePerson ?? '')
  setCheckbox('opt-notify-start',  settings.notifyStart !== false)
  setCheckbox('opt-notify-stop',   settings.notifyStop  !== false)
  const reminderSel = document.getElementById('opt-reminder-minutes') as HTMLSelectElement | null
  if (reminderSel) reminderSel.value = String(settings.reminderMinutes ?? 0)
  setCheckbox('opt-email-error',   !!settings.emailOnError)
  setCheckbox('opt-autostart',        !!settings.launchAtLogin)
  // …then correct it from the OS. The stored boolean and the real login item
  // drift: a user can remove the item by hand, a reinstall or migration can drop
  // it, and `set_launch_at_login` can fail. A checkbox that stays ticked while no
  // login item exists is a promise that scheduled recordings survive a reboot —
  // the one promise this app cannot afford to break quietly. `get_launch_at_login`
  // reads the OS; the setting follows it, not the other way round.
  void syncAutostartFromOs()
  setCheckbox('opt-auto-update',      autoUpdateEnabled(settings.autoUpdate))
  // The persisted answer has just landed — this is the first moment the gate can
  // be evaluated against what the operator actually chose, and the only place
  // that arms the schedule at startup.
  applyAutoUpdateSchedule()
  setVal('opt-update-channel',        settings.updateChannel ?? 'stable')
  paintActiveUpdateChannel()
  setCheckbox('opt-ask-open-editor',  settings.askOpenEditor !== false)
  setCheckbox('opt-local-adaptivity', !!settings.localAdaptivity)
  void refreshLocalNudgeCard()
  setVal('email-address', settings.emailAddress   ?? '')
  setVal('email-smtp',    settings.emailSmtp      ?? '')
  setVal('email-port',    settings.emailSmtpPort  ?? 587)
  setVal('email-user',    settings.emailSmtpUser  ?? '')
  setVal('email-from',    settings.emailSmtpFrom  ?? '')
  setVal('webhook-url',   settings.webhookUrl     ?? '')
  setCheckbox('opt-webhook-on-warn', !!settings.webhookOnWarn)
  // The password never comes back from storage — the field starts empty and the
  // «lagret» state is read from the OS keychain, not from a settings flag. (It
  // used to read `settings.emailSmtpPassSet`, an Electron-era field the Tauri
  // backend never wrote, so the row could never show that a password existed.)
  const passInput = document.getElementById('email-pass') as HTMLInputElement | null
  if (passInput) passInput.value = ''
  void refreshEmailGate()
  toggleEmailSection()

  // Version display — show full semver (vX.Y.Z) so brukere ser også patch-
  // releases (hotfixes). Tidligere truncated til major.minor noe som skjulte
  // hotfix-info som "v4.30.1 fixet OAuth-secrets-i-CI".
  const raw = (window as unknown as { appVersion?: string }).appVersion ?? ''
  const displayVersion = (() => {
    // 0.x.y → "Beta" prefix (legacy pre-release labeling)
    const beta = raw.match(/^0\.(\d+)\.(\d+)/)
    if (beta) {
      const major = parseInt(beta[1]), minor = parseInt(beta[2])
      return minor === 0 ? `Beta ${major}` : `Beta ${major}.${minor}`
    }
    // Modern releases: show full vMAJOR.MINOR.PATCH (full semver)
    const rel = raw.match(/^(\d+)\.(\d+)\.(\d+)/)
    if (rel) return `v${rel[1]}.${rel[2]}.${rel[3]}`
    // Fallback: anything else with at least two parts
    const fallback = raw.match(/^(\d+)\.(\d+)/)
    if (fallback) return `v${fallback[1]}.${fallback[2]}`
    return raw || '—'
  })()
  // ('hero-app-version' sto i denne listen, men elementet finnes ikke i
  // markupen — skrivingen traff ingenting og er fjernet.)
  ;['app-version', 'sidebar-version'].forEach(id => {
    const el = document.getElementById(id)
    if (el) el.textContent = displayVersion
  })

  setUpdateStatus('', t('update.checkHint', 'Klikk «Se etter oppdateringer nå» for å sjekke'))
  updateEditorClipUI()
  // The DOM now mirrors settings — rebase the bindings' baselines.
  resyncBoundSettings()
  // Settings only just arrived: re-evaluate the e-mail gate now that we can see
  // whether SMTP is filled in (setupGeneralPage runs before loadSettings).
  void refreshEmailGate()
}

/**
 * Read the auto-applying System + Varsler controls into `settings`.
 *
 * The SMTP server fields are deliberately NOT read here: they belong to the
 * explicit-save card (`setupSmtpCard`), and picking them up from a neighbouring
 * toggle's save would silently commit a half-typed mail server.
 */
function collectGeneralSettings(): void {
  patchSettings({
    language:          (document.getElementById('language-select') as HTMLSelectElement | null)?.value ?? 'no',
    churchName:        (document.getElementById('church-name')        as HTMLInputElement | null)?.value ?? '',
    responsiblePerson: (document.getElementById('responsible-person') as HTMLInputElement | null)?.value ?? '',
    notifyStart:       !!(document.getElementById('opt-notify-start') as HTMLInputElement | null)?.checked,
    notifyStop:        !!(document.getElementById('opt-notify-stop')  as HTMLInputElement | null)?.checked,
    reminderMinutes:   parseInt((document.getElementById('opt-reminder-minutes') as HTMLSelectElement | null)?.value ?? '0') || 0,
    emailOnError:      !!(document.getElementById('opt-email-error')  as HTMLInputElement | null)?.checked,
    emailAddress:      (document.getElementById('email-address')     as HTMLInputElement | null)?.value ?? '',
    webhookUrl:        (document.getElementById('webhook-url')       as HTMLInputElement | null)?.value.trim() || undefined,
    webhookOnWarn:     !!(document.getElementById('opt-webhook-on-warn') as HTMLInputElement | null)?.checked,
    launchAtLogin:     !!(document.getElementById('opt-autostart')         as HTMLInputElement | null)?.checked,
    autoUpdate:        !!(document.getElementById('opt-auto-update')       as HTMLInputElement | null)?.checked,
    // Anything the select cannot produce is not a channel; the backend applies
    // the same fallback (UpdateChannel::parse), so the two ends agree.
    updateChannel:     (document.getElementById('opt-update-channel') as HTMLSelectElement | null)?.value === 'beta' ? 'beta' : 'stable',
    askOpenEditor:     !!(document.getElementById('opt-ask-open-editor')   as HTMLInputElement | null)?.checked,
    localAdaptivity:   !!(document.getElementById('opt-local-adaptivity')   as HTMLInputElement | null)?.checked
  })
}

function toggleEmailSection(): void {
  const emailSect = document.getElementById('email-section')
  const emailErr  = document.getElementById('opt-email-error') as HTMLInputElement | null
  if (emailSect && emailErr) emailSect.style.display = emailErr.checked ? 'block' : 'none'
}

function setCheckbox(id: string, val: boolean): void {
  const el = document.getElementById(id) as HTMLInputElement | null
  if (el) el.checked = val
}

/**
 * Make the "Start automatisk" checkbox tell the truth about the OS login item.
 *
 * `get_launch_at_login` asks the OS; when it disagrees with the stored setting,
 * the OS wins and the setting is corrected in place (no debounced save storm —
 * `patchSettings` + the checkbox, and the next real save carries it). Silent on
 * any failure: a probe we could not run is not evidence either way.
 */
async function syncAutostartFromOs(): Promise<void> {
  try {
    const real = await window.api.getLaunchAtLogin?.()
    if (typeof real !== 'boolean') return
    setCheckbox('opt-autostart', real)
    if (real !== !!settings.launchAtLogin) {
      console.info('[general] launch-at-login differed from the OS — trusting the OS:', real)
      patchSettings({ launchAtLogin: real })
    }
  } catch {
    /* the OS could not be asked — leave the stored value alone */
  }
}

export function updateEditorClipUI(): void {
  const introPath = settings.editorIntroPath ?? ''
  const outroPath = settings.editorOutroPath ?? ''

  const introDisplay = document.getElementById('general-editor-intro-display')
  const outrDisplay  = document.getElementById('general-editor-outro-display')
  const introClear   = document.getElementById('btn-clear-editor-intro')
  const outrClear    = document.getElementById('btn-clear-editor-outro')

  if (introDisplay) introDisplay.textContent = introPath
    ? introPath.split(/[\\/]/).pop() ?? introPath
    : t('general.noClipSelected', 'Ingen fil valgt')
  if (outrDisplay)  outrDisplay.textContent  = outroPath
    ? outroPath.split(/[\\/]/).pop() ?? outroPath
    : t('general.noClipSelected', 'Ingen fil valgt')

  if (introClear) (introClear as HTMLElement).style.display = introPath ? '' : 'none'
  if (outrClear)  (outrClear  as HTMLElement).style.display = outroPath ? '' : 'none'
}

export function setUpdateStatus(dotCls: string, text: string): void {
  const dot = document.getElementById('update-status-dot')
  const txt = document.getElementById('update-status-text')
  if (dot) dot.className  = 'update-status-dot' + (dotCls ? ' ' + dotCls : '')
  if (txt) txt.textContent = text
}

/**
 * Say which feed «Se etter oppdateringer» asks, right next to the button.
 *
 * Nobody should be able to find out they are on beta by being handed a broken
 * version on a Sunday morning — a channel is a decision that has to keep being
 * visible after it is made, not just at the moment it is taken.
 */
export function paintActiveUpdateChannel(): void {
  const el = document.getElementById('update-channel-active')
  if (!el) return
  const beta = settings.updateChannel === 'beta'
  el.classList.toggle('is-beta', beta)
  el.textContent = beta
    ? t('general.updateChannelActiveBeta', 'Denne maskinen henter BETA-versjoner — nyere, og ikke prøvd på en gudstjeneste.')
    : t('general.updateChannelActiveStable', 'Denne maskinen henter stabile versjoner.')
}

function showUpdateToast(title: string, text: string, showInstall = false): void {
  const toast   = document.getElementById('update-toast')
  const titleEl = document.getElementById('update-toast-title')
  const textEl  = document.getElementById('update-toast-text')
  const actions = document.getElementById('update-toast-actions')
  const progEl  = document.getElementById('update-toast-progress')
  if (!toast) return
  if (titleEl) titleEl.textContent = title
  if (textEl)  textEl.textContent  = text
  if (actions) actions.style.display = showInstall ? 'block' : 'none'
  if (progEl)  progEl.style.display  = showInstall ? 'none'  : 'block'
  // Re-trigger animation
  toast.style.display = 'none'
  requestAnimationFrame(() => { toast.style.display = 'flex' })
}

function setToastProgress(pct: number): void {
  const bar = document.getElementById('update-toast-bar') as HTMLElement | null
  if (bar) bar.style.width = pct + '%'
}

function hideToast(): void {
  const toast = document.getElementById('update-toast')
  if (toast) toast.style.display = 'none'
}
