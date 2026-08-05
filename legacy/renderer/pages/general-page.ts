import { t, loadLocale, currentLang } from '../i18n'
import { settings, patchSettings } from '../state'
import { setVal } from '../helpers'
import { confirmDialog } from '../ui/dialog'
import { toast } from '../ui/toast'
import {
  bindSetting,
  resyncBoundSettings,
  showSavedChip,
  type BindSettingOpts,
} from '../ui/bind-setting'
import { clearFieldErrors, setFieldError } from '../ui/field-error'
import { applyFeatureGate } from '../ui/feature-gate'
import { canSendTestEmail, emailGateStatus } from '../ui/feature-gate-core'

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
  }))
  bindSetting('webhook-url', generalBinding({
    key: 'webhookUrl',
    validate: (value) => {
      const v = String(value ?? '').trim()
      if (!v) return null
      return /^https:\/\/\S+$/.test(v)
        ? null
        : t('notify.errWebhookUrl', 'Webhook-URL må begynne med https://')
    },
  }))
  bindSetting('opt-webhook-on-warn', generalBinding({ key: 'webhookOnWarn' }))

  bindSetting('opt-autostart',       generalBinding({ key: 'launchAtLogin' }))
  bindSetting('opt-show-on-startup', generalBinding({ key: 'showOnStartup' }))
  bindSetting('opt-ask-open-editor', generalBinding({ key: 'askOpenEditor' }))
  bindSetting('opt-auto-update',     generalBinding({ key: 'autoUpdate' }))

  setupSmtpCard()
  void refreshEmailGate()
  setupWebhookTest()

  document.getElementById('btn-show-onboarding')?.addEventListener('click', () => window.showOnboarding())

  // Steinberg ASIO attribution is required by the ASIO SDK licence and is only
  // relevant in the Windows build (the only build compiled with ASIO support).
  // Reveal the card on Windows; it stays hidden on macOS.
  if (/win/i.test(navigator.userAgent)) {
    const asioCard = document.getElementById('asio-attribution-card')
    if (asioCard) asioCard.style.display = ''
  }

  document.getElementById('btn-clear-smtp-pass')?.addEventListener('click', async () => {
    await window.api.clearSmtpPassword()
    const passInput = document.getElementById('email-pass') as HTMLInputElement | null
    const clearBtn  = document.getElementById('btn-clear-smtp-pass') as HTMLElement | null
    if (passInput) { passInput.value = ''; passInput.placeholder = '' }
    if (clearBtn)  clearBtn.style.display = 'none'
  })

  // Gmail OAuth connect (btn-email-gmail-connect) has no working backend yet
  // (2026-08 audit: gmailConnect was a permanent-failure stub with no `ok`
  // field, so a click always produced an empty "Kunne ikke koble til Google: "
  // alert) — the button is disabled in the markup with an honest reason
  // instead, so there is nothing to wire here until the feature is built.

  document.getElementById('btn-email-gmail-disconnect')?.addEventListener('click', async () => {
    const ok = await confirmDialog({
      title:        t('dialog.gmailDisconnectTitle', 'Koble fra Google-kontoen?'),
      message:      t('notify.emailGmailConfirmDisconnect', 'E-postvarsler vil falle tilbake til SMTP.'),
      confirmLabel: t('dialog.disconnect', 'Koble fra'),
      danger:       true,
    })
    if (!ok) return
    await window.api.gmailDisconnect()
    await refreshGmailStatus()
  })

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

/**
 * The e-mail gate — driven by the REAL `email_status` command.
 *
 * The night sweep hard-disabled «Send test» and the Gmail button in the markup
 * with a hardcoded "not available in this version" title, because the old shim
 * stubs reported a fabricated failure. That was honest but blind: it said the
 * same thing in a build WITH `--features email` as in one without.
 *
 * `email_status` answers both halves — whether the send path was compiled in
 * (`featureBuilt`) and whether a Gmail token is stored (`gmailConnected`) — so
 * the section can state which of the two is missing, and «Send test» is enabled
 * exactly when a transport exists and there is somewhere to send it.
 */
async function refreshEmailGate(): Promise<void> {
  const facts = {
    featureBuilt: false,
    gmailConnected: false,
    smtpConfigured: !!(settings.emailSmtp ?? '').trim() && !!(settings.emailSmtpUser ?? '').trim(),
  }
  try {
    const status = await window.api.emailStatus()
    facts.featureBuilt = !!status.featureBuilt
    facts.gmailConnected = !!status.gmailConnected
  } catch {
    /* keep the pessimistic defaults — a failed status check is not a send path */
  }

  const gate = emailGateStatus(facts)
  applyFeatureGate('email-notify-card', {
    status: gate,
    chipText: gate === 'unavailable'
      ? t('gate.chipUnavailable', 'Ikke tilgjengelig')
      : t('gate.chipUnconfigured', 'Ikke konfigurert'),
    explanation: gate === 'unavailable'
      ? t('notify.gateNoFeature', 'E-postutsending er ikke bygget inn i denne versjonen. Varsler om feilede opptak må hentes fra Hjem-siden eller en webhook inntil videre.')
      : t('notify.gateNoTransport', 'Ingen sendemetode er satt opp ennå. Logg inn med Google, eller fyll ut SMTP-feltene under Avansert.'),
  })

  // «Send test» is enabled ONLY when a transport exists and we know where to
  // send it. The recipient check lives in the same pure helper as the gate.
  const recipient = (document.getElementById('email-address') as HTMLInputElement | null)?.value.trim()
    ?? settings.emailAddress ?? ''
  const testBtn = document.getElementById('btn-test-email') as HTMLButtonElement | null
  if (testBtn) {
    const allowed = canSendTestEmail(facts, !!recipient)
    testBtn.disabled = !allowed
    testBtn.title = allowed
      ? ''
      : recipient
        ? t('notify.gateTestNoTransport', 'Sett opp en sendemetode først.')
        : t('notify.gateTestNoRecipient', 'Skriv inn en mottakeradresse først.')
    if (allowed && !testBtn.dataset.wired) {
      testBtn.dataset.wired = '1'
      testBtn.addEventListener('click', () => void sendTestEmail())
    }
  }
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
      pass: (document.getElementById('email-pass') as HTMLInputElement | null)?.value || undefined,
      from: settings.emailSmtpUser || recipient,
    })
    toast(res.ok ? 'success' : 'error',
      res.ok
        ? t('notify.testEmailSent', 'Test-e-post sendt til {to}').replace('{to}', recipient)
        : t('notify.testEmailFailed', 'Test-e-posten gikk ikke gjennom: {err}').replace('{err}', res.error ?? '—'))
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
 * Host, username and password are a set: applying them one keystroke at a time
 * would leave the alert path pointing at a half-typed server, and the failure
 * would only show up the day a recording actually fails. So this card validates
 * as a unit and writes on «Lagre»; «Avbryt» puts the stored values back.
 */
function setupSmtpCard(): void {
  const host = document.getElementById('email-smtp') as HTMLInputElement | null
  const user = document.getElementById('email-user') as HTMLInputElement | null
  const pass = document.getElementById('email-pass') as HTMLInputElement | null
  const saveBtn   = document.getElementById('btn-smtp-save')
  const cancelBtn = document.getElementById('btn-smtp-cancel')
  if (!host || !saveBtn) return

  saveBtn.addEventListener('click', async () => {
    clearFieldErrors(document.getElementById('email-smtp-advanced'))
    const hostVal = host.value.trim()
    const userVal = user?.value.trim() ?? ''
    // An empty card is a valid state: it means "no SMTP, use Gmail".
    if (hostVal && !/^[\w.-]+\.[a-z]{2,}$/i.test(hostVal)) {
      setFieldError(host, t('notify.errSmtpHost', 'Skriv et servernavn, f.eks. smtp.gmail.com'))
      return
    }
    if (hostVal && !userVal) {
      setFieldError(user, t('notify.errSmtpUser', 'Brukernavnet er e-postadressen du sender fra'))
      return
    }
    patchSettings({
      emailSmtp:     hostVal,
      emailSmtpUser: userVal,
      emailSmtpPass: pass?.value ?? '',
      emailSmtpPort: +((document.getElementById('email-port') as HTMLInputElement | null)?.value ?? 587),
    })
    const ok = await window.api.saveSettings(settings).catch(() => false)
    if (!ok) { toast('error', t('general.saveFailed', 'Kunne ikke lagre innstillingen')); return }
    if (pass) pass.value = ''
    showSavedChip(saveBtn.parentElement)
    // A newly-filled SMTP server can flip the gate from «ikke konfigurert» to
    // usable — re-check rather than making the user reopen the tab.
    void refreshEmailGate()
  })

  cancelBtn?.addEventListener('click', () => {
    clearFieldErrors(document.getElementById('email-smtp-advanced'))
    setVal('email-smtp', settings.emailSmtp ?? '')
    setVal('email-user', settings.emailSmtpUser ?? '')
    setVal('email-port', settings.emailSmtpPort ?? 587)
    if (pass) pass.value = ''
  })
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
  const hideProgress = (): void => {
    const wrap = document.getElementById('update-progress-wrap')
    if (wrap) wrap.style.display = 'none'
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
    const wrap = document.getElementById('update-progress-wrap')
    const bar  = document.getElementById('update-progress-bar') as HTMLElement | null
    if (wrap) wrap.style.display = 'block'
    if (bar)  bar.style.width   = pct + '%'
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

  // Auto-check on launch + hourly, mirroring the Electron app's updater (which
  // checked on startup and every 60 min). Listeners above are registered first,
  // so the synthesized update-* events reach the UI. Gated on the autoUpdate
  // setting; the manual "Se etter oppdateringer" button always works.
  if (settings.autoUpdate !== false) {
    void window.api.checkForUpdates()
    setInterval(() => { void window.api.checkForUpdates() }, 60 * 60 * 1000)
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
  setCheckbox('opt-show-on-startup',  !!settings.showOnStartup)
  setCheckbox('opt-auto-update',      settings.autoUpdate !== false)
  setCheckbox('opt-ask-open-editor',  settings.askOpenEditor !== false)
  setVal('email-address', settings.emailAddress   ?? '')
  setVal('email-smtp',    settings.emailSmtp      ?? '')
  setVal('email-port',    settings.emailSmtpPort  ?? 587)
  setVal('email-user',    settings.emailSmtpUser  ?? '')
  setVal('webhook-url',   settings.webhookUrl     ?? '')
  setCheckbox('opt-webhook-on-warn', !!settings.webhookOnWarn)
  const passInput = document.getElementById('email-pass') as HTMLInputElement | null
  const clearBtn  = document.getElementById('btn-clear-smtp-pass') as HTMLElement | null
  if (passInput) {
    passInput.value = ''
    passInput.placeholder = settings.emailSmtpPassSet ? '••••••••' : ''
  }
  if (clearBtn) clearBtn.style.display = settings.emailSmtpPassSet ? 'inline' : 'none'
  toggleEmailSection()
  // Best-effort — failures are non-fatal (the SMTP path still works).
  void refreshGmailStatus()

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
  ;['app-version', 'sidebar-version', 'hero-app-version'].forEach(id => {
    const el = document.getElementById(id)
    if (el) el.textContent = displayVersion
  })

  setUpdateStatus('', t('update.checkHint', 'Klikk «Se etter oppdateringer» for å sjekke'))
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
    showOnStartup:     !!(document.getElementById('opt-show-on-startup')   as HTMLInputElement | null)?.checked,
    autoUpdate:        !!(document.getElementById('opt-auto-update')       as HTMLInputElement | null)?.checked,
    askOpenEditor:     !!(document.getElementById('opt-ask-open-editor')   as HTMLInputElement | null)?.checked
  })
}

function toggleEmailSection(): void {
  const emailSect = document.getElementById('email-section')
  const emailErr  = document.getElementById('opt-email-error') as HTMLInputElement | null
  if (emailSect && emailErr) emailSect.style.display = emailErr.checked ? 'block' : 'none'
}

/**
 * Read the current Gmail-OAuth status from main and update the
 * email-OAuth-card on screen accordingly. Two states:
 *   • Not connected → show "Logg inn med Google" button + default sub-text
 *   • Connected → show "Koble fra"-knapp + "Sender via <email>"-sub-text
 *
 * Also flips the Avansert SMTP <details> closed when Gmail is connected,
 * since the SMTP fields are no longer required.
 */
async function refreshGmailStatus(): Promise<void> {
  let status: { connected: boolean; email?: string; needsReauth?: boolean } = { connected: false }
  try { status = await window.api.gmailStatus() } catch { /* gmail not available — keep defaults */ }

  const connectBtn    = document.getElementById('btn-email-gmail-connect') as HTMLElement | null
  const disconnectBtn = document.getElementById('btn-email-gmail-disconnect') as HTMLElement | null
  const statusEl      = document.getElementById('email-gmail-status') as HTMLElement | null
  const smtpAdvanced  = document.getElementById('email-smtp-advanced') as HTMLDetailsElement | null

  if (status.connected) {
    if (connectBtn)    connectBtn.style.display = 'none'
    if (disconnectBtn) disconnectBtn.style.display = ''
    if (statusEl) {
      const reauth = status.needsReauth ? ' ' + t('notify.emailGmailReauth', '⚠ Krever ny pålogging') : ''
      statusEl.textContent = t('notify.emailGmailSendsAs', 'Sender via') + ' ' + (status.email ?? '—') + reauth
      statusEl.style.color = status.needsReauth ? 'var(--red)' : 'var(--green)'
    }
    // Auto-collapse the SMTP advanced section — Gmail handles the send now.
    if (smtpAdvanced) smtpAdvanced.open = false
  } else {
    if (connectBtn)    connectBtn.style.display = ''
    if (disconnectBtn) disconnectBtn.style.display = 'none'
    if (statusEl) {
      statusEl.textContent = t('notify.emailGmailDesc', 'Send via din Gmail-konto — ingen SMTP-konfig.')
      statusEl.style.color = ''
    }
  }
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
