import { describe, expect, it } from 'vitest'
import {
  canSendTestEmail,
  cloudGateStatus,
  emailBlockReason,
  emailGateStatus,
  hasEmailTransport,
  mapGate,
} from './feature-gate-core'

describe('mapGate', () => {
  it('is invisible when the feature works', () => {
    const view = mapGate({ status: 'ok', chipText: 'ignored', explanation: 'ignored' })
    expect(view).toEqual({
      showBanner: false,
      disabled: false,
      variant: 'ok',
      chipText: '',
      explanation: '',
    })
  })

  it('disables and explains when unconfigured', () => {
    const view = mapGate({ status: 'unconfigured' })
    expect(view.showBanner).toBe(true)
    expect(view.disabled).toBe(true)
    expect(view.chipText).toBe('Ikke konfigurert')
    expect(view.explanation).not.toBe('')
  })

  it('distinguishes "not built" from "not set up"', () => {
    expect(mapGate({ status: 'unavailable' }).chipText).toBe('Ikke tilgjengelig')
    expect(mapGate({ status: 'unavailable' }).explanation).not.toBe(
      mapGate({ status: 'unconfigured' }).explanation,
    )
  })

  it('prefers caller-supplied (translated) copy and trims blanks away', () => {
    const view = mapGate({
      status: 'unconfigured',
      chipText: '  Ikke satt opp  ',
      explanation: ' Be utvikleren om en build med OAuth-nøkkel. ',
      docsHint: '  ',
    })
    expect(view.chipText).toBe('Ikke satt opp')
    expect(view.explanation).toBe('Be utvikleren om en build med OAuth-nøkkel.')
    expect(view.docsHint).toBeUndefined()
  })
})

describe('cloudGateStatus', () => {
  it('maps the real cloud_is_configured predicate', () => {
    expect(cloudGateStatus(true)).toBe('ok')
    expect(cloudGateStatus(false)).toBe('unconfigured')
  })
})

describe('emailGateStatus', () => {
  const facts = (o: Partial<Parameters<typeof emailGateStatus>[0]> = {}) => ({
    featureBuilt: true,
    gmailConnected: false,
    smtpConfigured: false,
    smtpPasswordAvailable: false,
    ...o,
  })

  it('is unavailable without the cargo feature, whatever the user typed', () => {
    expect(emailGateStatus(facts({ featureBuilt: false, smtpConfigured: true }))).toBe(
      'unavailable',
    )
    expect(emailGateStatus(facts({ featureBuilt: false, gmailConnected: true }))).toBe(
      'unavailable',
    )
  })

  it('is ok via either transport', () => {
    expect(emailGateStatus(facts({ gmailConnected: true }))).toBe('ok')
    expect(emailGateStatus(facts({ smtpConfigured: true }))).toBe('ok')
  })

  // The card must stay usable while it is being set up. `mapGate` inerts every
  // non-'ok' status, and this card's controls ARE the configuration — an
  // 'unconfigured' gate here would disable the recipient field, the SMTP fields
  // and the enable toggle, leaving no way to ever reach 'ok'.
  it('never gates a build that CAN send, so the card can be configured at all', () => {
    expect(emailGateStatus(facts())).toBe('ok')
    expect(mapGate({ status: emailGateStatus(facts()) }).disabled).toBe(false)
  })
})

describe('hasEmailTransport', () => {
  const smtp = {
    featureBuilt: true,
    gmailConnected: false,
    smtpConfigured: true,
    smtpPasswordAvailable: true,
  }

  it('accepts Gmail without any SMTP fields', () => {
    expect(
      hasEmailTransport({ ...smtp, gmailConnected: true, smtpConfigured: false, smtpPasswordAvailable: false }),
    ).toBe(true)
  })

  // Host + username with no password reaches the server and is rejected —
  // the backend answers `missing_password`. That is not a transport.
  it('requires a password alongside an SMTP host + username', () => {
    expect(hasEmailTransport(smtp)).toBe(true)
    expect(hasEmailTransport({ ...smtp, smtpPasswordAvailable: false })).toBe(false)
    expect(hasEmailTransport({ ...smtp, smtpConfigured: false })).toBe(false)
  })

  it('is false without the cargo feature', () => {
    expect(hasEmailTransport({ ...smtp, featureBuilt: false })).toBe(false)
  })
})

describe('canSendTestEmail', () => {
  it('needs a working transport AND somewhere to send it', () => {
    const built = {
      featureBuilt: true,
      gmailConnected: true,
      smtpConfigured: false,
      smtpPasswordAvailable: false,
    }
    expect(canSendTestEmail(built, true)).toBe(true)
    expect(canSendTestEmail(built, false)).toBe(false)
    expect(
      canSendTestEmail({ ...built, featureBuilt: false }, true),
    ).toBe(false)
  })
})

describe('emailBlockReason', () => {
  const base = {
    featureBuilt: true,
    gmailConnected: true,
    smtpConfigured: false,
    smtpPasswordAvailable: false,
  }

  it('names the one thing that is missing, most fundamental first', () => {
    expect(emailBlockReason({ ...base, featureBuilt: false }, true)).toBe('noFeature')
    expect(emailBlockReason({ ...base, gmailConnected: false }, true)).toBe('noTransport')
    expect(emailBlockReason(base, false)).toBe('noRecipient')
    expect(emailBlockReason(base, true)).toBeNull()
  })
})
