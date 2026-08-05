import { settings } from './state'
import { t } from './i18n'
import { confirmDialog } from './ui/dialog'

/**
 * Called right after the user schedules an automatic recording (a weekly slot or a
 * special day). If the app isn't set to launch at login it won't be RUNNING at the
 * scheduled time — so a planned recording silently never fires after a reboot or a
 * quit. Remind the user and offer to turn it on in one click.
 *
 * `saveSettings` registers the OS login item (via the shim's syncLaunchAtLogin), so
 * flipping the flag + saving is all that's needed.
 */
export async function remindAutostartIfNeeded(): Promise<void> {
  if (settings.launchAtLogin) return // already armed — nothing to nag about

  // The copy is deliberately about THIS recording, not about a setting. The
  // volunteer just scheduled something; the question they can answer is "will
  // it happen?", not "do you want launchAtLogin enabled?". Both buttons say
  // what they do, so neither one is the scary unlabelled option.
  const ok = await confirmDialog({
    title:        t('dialog.autostartTitle', 'Skal opptaket starte av seg selv?'),
    message:      t(
      'schedule.autostartReminder',
      'Opptaket er lagt til. Men SundayRec starter ikke automatisk med maskinen, så et planlagt opptak uteblir hvis maskinen har vært avslått eller programmet er lukket. Vi kan slå på automatisk oppstart nå — det tar ingen plass på skjermen, programmet kjører stille i bakgrunnen.',
    ),
    confirmLabel: t('dialog.autostartConfirm', 'Slå på autostart'),
    cancelLabel:  t('dialog.notNow', 'Ikke nå'),
  })
  if (!ok) return

  settings.launchAtLogin = true
  await window.api.saveSettings(settings)
}
