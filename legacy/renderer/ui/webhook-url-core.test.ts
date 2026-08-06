import { describe, expect, it } from 'vitest'

import no from '../../locales/no.json'
import {
  classifyWebhookHost,
  classifyWebhookUrl,
  hostOf,
  isLocalWebhookUrl,
  needsLocalConfirmation,
  nextAllowLocal,
  webhookUrlError,
  type WebhookHostClass,
} from './webhook-url-core'

/**
 * The SAME table `crates/sundayrec-core/src/webhook.rs` asserts. Two
 * classifiers that disagree are worse than one: the panel would approve an
 * address the backend then silently refuses, or ask a question about an address
 * that never needed one. Kept literal on both sides so a divergence is a
 * failing test rather than a support ticket.
 */
const SHARED_TABLE: Array<[string, WebhookHostClass]> = [
  // Loopback
  ['127.0.0.1', 'loopback'],
  ['127.1.2.3', 'loopback'],
  ['::1', 'loopback'],
  ['::', 'loopback'],
  ['localhost', 'loopback'],
  // Private (RFC1918 + CGNAT + ULA)
  ['10.0.0.1', 'private'],
  ['10.255.255.255', 'private'],
  ['172.16.0.1', 'private'],
  ['172.31.255.254', 'private'],
  ['192.168.1.50', 'private'],
  ['100.64.0.1', 'private'],
  ['100.127.255.255', 'private'],
  ['0.0.0.0', 'private'],
  ['fc00::1', 'private'],
  ['fd12:3456:789a::1', 'private'],
  ['ff02::1', 'private'],
  ['nas.lan', 'private'],
  ['api.internal', 'private'],
  ['hass.home.arpa', 'private'],
  ['mixer', 'private'],
  // Link-local (incl. the cloud metadata endpoint and mDNS)
  ['169.254.1.1', 'linkLocal'],
  ['169.254.169.254', 'linkLocal'],
  ['fe80::1', 'linkLocal'],
  ['febf::1', 'linkLocal'],
  ['printer.local', 'linkLocal'],
  // Public — including everything that merely LOOKS private
  ['1.1.1.1', 'public'],
  ['8.8.8.8', 'public'],
  ['172.15.0.1', 'public'],
  ['172.32.0.1', 'public'],
  ['192.167.1.1', 'public'],
  ['100.63.255.255', 'public'],
  ['100.128.0.1', 'public'],
  ['11.0.0.1', 'public'],
  ['2606:4700:4700::1111', 'public'],
  ['hooks.slack.com', 'public'],
  ['localhost.example.com', 'public'],
  ['mylocal.com', 'public'],
]

const lookup = (key: string): unknown =>
  key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], no)

describe('host classification (mirrors sundayrec-core::webhook)', () => {
  it.each(SHARED_TABLE)('%s is %s', (host, want) => {
    expect(classifyWebhookHost(host)).toBe(want)
  })

  it('treats an IPv4-mapped IPv6 address as its IPv4 self', () => {
    // The classic bypass: an IPv6 literal that is really a loopback.
    expect(classifyWebhookHost('::ffff:127.0.0.1')).toBe('loopback')
    expect(classifyWebhookHost('::ffff:192.168.0.5')).toBe('private')
    expect(classifyWebhookHost('::ffff:169.254.169.254')).toBe('linkLocal')
  })

  it('ignores a trailing root dot', () => {
    expect(classifyWebhookHost('printer.local.')).toBe('linkLocal')
  })

  it('is invalid for nothing', () => {
    expect(classifyWebhookHost('')).toBe('invalid')
    expect(classifyWebhookHost('   ')).toBe('invalid')
  })
})

describe('hostOf', () => {
  it('strips scheme, port and path', () => {
    expect(hostOf('https://example.com/hook')).toBe('example.com')
    expect(hostOf('http://192.168.1.5:8123/api')).toBe('192.168.1.5')
    expect(hostOf('https://example.com?a=1')).toBe('example.com')
    expect(hostOf('https://[::1]:9000/x')).toBe('::1')
  })

  it('takes the host AFTER the userinfo', () => {
    // `https://hooks.slack.com@127.0.0.1/` looks like Slack and is loopback.
    expect(hostOf('https://hooks.slack.com@127.0.0.1/x')).toBe('127.0.0.1')
    expect(classifyWebhookUrl('https://hooks.slack.com@127.0.0.1/x')).toBe('loopback')
  })

  it('is null for a non-http URL', () => {
    expect(hostOf('ftp://example.com')).toBeNull()
    expect(hostOf('https://')).toBeNull()
    expect(hostOf('')).toBeNull()
  })
})

describe('webhookUrlError', () => {
  it('accepts an empty field — a webhook is optional', () => {
    expect(webhookUrlError('')).toBeNull()
    expect(webhookUrlError('   ')).toBeNull()
  })

  it('accepts the ordinary chat webhooks unchanged', () => {
    for (const url of [
      'https://hooks.slack.com/services/T/B/X',
      'https://discord.com/api/webhooks/1/abc',
      'https://kirka.example.org/sundayrec-hook',
    ]) {
      expect(webhookUrlError(url), url).toBeNull()
    }
  })

  it('rejects a non-http(s) URL', () => {
    expect(webhookUrlError('example.com')?.key).toBe('notify.errWebhookUrl')
    expect(webhookUrlError('ftp://example.com')?.key).toBe('notify.errWebhookUrl')
  })

  it('rejects plaintext to the open internet but allows it on the LAN', () => {
    // The payload names the church and what just failed; that does not cross
    // the internet in the clear. A control panel in the booth usually speaks
    // only http, and that traffic never leaves the building.
    expect(webhookUrlError('http://example.com/hook')?.key).toBe('notify.errWebhookHttp')
    expect(webhookUrlError('http://192.168.1.50/hook')).toBeNull()
    expect(webhookUrlError('http://hass.local:8123/api/webhook/x')).toBeNull()
    expect(webhookUrlError('http://localhost:9000/hook')).toBeNull()
  })

  it('names locale keys that exist in no.json', () => {
    for (const url of ['example.com', 'http://example.com/hook']) {
      const err = webhookUrlError(url)
      expect(err).not.toBeNull()
      expect(lookup(err!.key), `${err!.key} missing from no.json`).toBeTypeOf('string')
    }
  })
})

describe('the local-network opt-in', () => {
  it('asks only for a local address that is not already allowed', () => {
    expect(needsLocalConfirmation('http://192.168.1.50/hook', false)).toBe(true)
    expect(needsLocalConfirmation('http://192.168.1.50/hook', true)).toBe(false)
    expect(needsLocalConfirmation('https://hooks.slack.com/services/T/B/X', false)).toBe(false)
  })

  it('is an opt-in for ONE address, not a mode', () => {
    // Confirming a LAN address grants it…
    expect(nextAllowLocal('http://192.168.1.50/hook', true)).toBe(true)
    // …declining does not…
    expect(nextAllowLocal('http://192.168.1.50/hook', false)).toBe(false)
    // …and switching to a public URL clears it, so a later switch back to a
    // DIFFERENT LAN address asks again instead of inheriting the permission.
    expect(nextAllowLocal('https://hooks.slack.com/services/T/B/X', true)).toBe(false)
  })

  it('agrees with isLocalWebhookUrl about what "local" means', () => {
    for (const [host, cls] of SHARED_TABLE) {
      const url = `http://${host.includes(':') ? `[${host}]` : host}/hook`
      const local = cls === 'loopback' || cls === 'private' || cls === 'linkLocal'
      expect(isLocalWebhookUrl(url), url).toBe(local)
    }
  })
})
