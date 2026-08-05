import { describe, expect, it } from 'vitest'
import {
  canSendTestEmail,
  cloudGateStatus,
  emailGateStatus,
  liveBlockReason,
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

  it('is unconfigured when the build can send but nothing is set up', () => {
    expect(emailGateStatus(facts())).toBe('unconfigured')
  })
})

describe('canSendTestEmail', () => {
  it('needs a working transport AND somewhere to send it', () => {
    const built = { featureBuilt: true, gmailConnected: true, smtpConfigured: false }
    expect(canSendTestEmail(built, true)).toBe(true)
    expect(canSendTestEmail(built, false)).toBe(false)
    expect(
      canSendTestEmail({ ...built, featureBuilt: false }, true),
    ).toBe(false)
  })
})

describe('liveBlockReason', () => {
  it('names the specific thing that is missing', () => {
    expect(liveBlockReason({ total: 0, enabled: 0, ready: 0 })).toBe('noDestinations')
    expect(liveBlockReason({ total: 2, enabled: 0, ready: 0 })).toBe('noEnabled')
    expect(liveBlockReason({ total: 2, enabled: 1, ready: 0 })).toBe('noKey')
  })

  it('returns null when the button may be pressed', () => {
    expect(liveBlockReason({ total: 2, enabled: 1, ready: 1 })).toBeNull()
  })
})
