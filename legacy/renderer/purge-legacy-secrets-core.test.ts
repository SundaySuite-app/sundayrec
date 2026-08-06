import { describe, expect, it } from 'vitest'

import { stripLegacySecrets } from './purge-legacy-secrets-core'

describe('stripLegacySecrets', () => {
  it('removes emailSmtpPass and reports changed', () => {
    const blob = JSON.stringify({ language: 'nb', emailSmtpPass: 'hunter2', emailSmtpPort: 587 })
    const { changed, out } = stripLegacySecrets(blob)
    expect(changed).toBe(true)
    const parsed = JSON.parse(out)
    expect(parsed).not.toHaveProperty('emailSmtpPass')
    // Everything else survives untouched.
    expect(parsed.language).toBe('nb')
    expect(parsed.emailSmtpPort).toBe(587)
  })

  it('removes the Electron-era emailSmtpPassEnc ciphertext too', () => {
    const blob = JSON.stringify({ emailSmtpPassEnc: 'base64ciphertext==' })
    const { changed, out } = stripLegacySecrets(blob)
    expect(changed).toBe(true)
    expect(JSON.parse(out)).not.toHaveProperty('emailSmtpPassEnc')
  })

  it('leaves emailSmtpPassSet alone (it is a flag, not a secret)', () => {
    const blob = JSON.stringify({ emailSmtpPassSet: true })
    const { changed, out } = stripLegacySecrets(blob)
    expect(changed).toBe(false)
    expect(JSON.parse(out)).toEqual({ emailSmtpPassSet: true })
  })

  it('is a no-op when neither key is present', () => {
    const blob = JSON.stringify({ language: 'nb', autoUpdate: true })
    const { changed, out } = stripLegacySecrets(blob)
    expect(changed).toBe(false)
    expect(out).toBe(blob)
  })

  it('is a no-op, not a throw, on corrupt JSON', () => {
    const blob = '{not valid json'
    const { changed, out } = stripLegacySecrets(blob)
    expect(changed).toBe(false)
    expect(out).toBe(blob)
  })

  it('is a no-op on missing/empty input', () => {
    expect(stripLegacySecrets(null)).toEqual({ changed: false, out: '' })
    expect(stripLegacySecrets(undefined)).toEqual({ changed: false, out: '' })
    expect(stripLegacySecrets('')).toEqual({ changed: false, out: '' })
  })

  it('is a no-op when the JSON is valid but not a plain object (array/primitive)', () => {
    expect(stripLegacySecrets('[1,2,3]')).toEqual({ changed: false, out: '[1,2,3]' })
    expect(stripLegacySecrets('"just a string"')).toEqual({ changed: false, out: '"just a string"' })
    expect(stripLegacySecrets('42')).toEqual({ changed: false, out: '42' })
  })

  it('removes both keys at once', () => {
    const blob = JSON.stringify({ emailSmtpPass: 'x', emailSmtpPassEnc: 'y', emailAddress: 'a@b.no' })
    const { changed, out } = stripLegacySecrets(blob)
    expect(changed).toBe(true)
    const parsed = JSON.parse(out)
    expect(parsed).not.toHaveProperty('emailSmtpPass')
    expect(parsed).not.toHaveProperty('emailSmtpPassEnc')
    expect(parsed.emailAddress).toBe('a@b.no')
  })
})
