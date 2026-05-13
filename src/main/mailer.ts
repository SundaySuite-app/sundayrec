import nodemailer from 'nodemailer'
import type { Settings } from '../types'

function esc(str: unknown): string {
  return String(str ?? '').replace(/[&<>"']/g, m =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m] ?? m)
  )
}

export async function sendError(settings: Settings, errorMessage: string): Promise<void> {
  if (!settings.emailAddress || !settings.emailSmtp) return

  const transporter = nodemailer.createTransport({
    host: settings.emailSmtp,
    port: settings.emailSmtpPort || 587,
    secure: settings.emailSmtpPort === 465,
    auth: settings.emailSmtpUser
      ? { user: settings.emailSmtpUser, pass: settings.emailSmtpPass }
      : undefined
  })

  const church = settings.churchName || 'SundayRec'
  const date   = new Date().toLocaleDateString('nb-NO', {
    weekday: 'long', year: 'numeric', month: 'long', day: 'numeric'
  })

  try {
    await transporter.sendMail({
      from: `"SundayRec" <${settings.emailSmtpUser || 'noreply@sundayrec.app'}>`,
      to: settings.emailAddress,
      subject: `⚠️ Opptaksfeil — ${church} — ${date}`,
      text: [
        `Hei ${settings.responsiblePerson || ''},`,
        '',
        `Det oppstod en feil under planlagt opptak hos ${church}:`,
        '',
        `Feil: ${errorMessage}`,
        `Dato: ${date}`,
        '',
        'Vennligst sjekk at lydmikseren er koblet til og prøv et manuelt opptak.',
        '',
        'Hilsen SundayRec'
      ].join('\n'),
      html: `
        <p>Hei ${esc(settings.responsiblePerson || '')},</p>
        <p>Det oppstod en feil under planlagt opptak hos <strong>${esc(church)}</strong>:</p>
        <blockquote style="background:#fee;padding:12px;border-left:4px solid #f05;">
          <strong>Feil:</strong> ${esc(errorMessage)}<br>
          <strong>Dato:</strong> ${esc(date)}
        </blockquote>
        <p>Vennligst sjekk at lydmikseren er koblet til og prøv et manuelt opptak.</p>
        <p>Hilsen SundayRec</p>
      `
    })
  } catch (err) {
    console.error('Failed to send error email:', (err as Error).message)
  }
}
