/**
 * Webhook-URL decisions — pure, DOM-free, i18n-free (E1.4).
 *
 * MIRRORS `crates/sundayrec-core/src/webhook.rs`. The backend is the authority:
 * it re-classifies every URL and re-resolves the host before each POST, so
 * nothing here is a security boundary. This exists so the settings panel can ask
 * the right question at the moment the operator types the address, without a
 * network round-trip and without saving a webhook that would then silently never
 * fire.
 *
 * The policy (the owner's, not a default): a webhook pointed at the LOCAL
 * network is blocked unless the operator explicitly allows THAT address.
 * Churches legitimately webhook LAN devices — a control panel in the booth, a
 * Home Assistant box that lights the ON AIR sign — so a blanket block would
 * break a real use. Plaintext `http://` stays allowed for those; it is refused
 * for public hosts, where the payload (the church's name and what just failed)
 * would cross the open internet in the clear.
 *
 * Keep the two classifiers in step. The Rust side carries the exhaustive
 * address tables; this one carries the same rules in the same order.
 */

export type WebhookHostClass = 'public' | 'loopback' | 'private' | 'linkLocal' | 'invalid'

/** A localizable string: the locale key plus its Norwegian fallback. */
export interface WebhookCopy {
  key: string
  fallback: string
}

/** mDNS — resolvable only inside the broadcast domain. */
const LINK_LOCAL_SUFFIXES = ['.local']
/** LAN names resolved by an ordinary local DNS server. */
const PRIVATE_SUFFIXES = ['.lan', '.internal', '.home', '.home.arpa', '.intranet']
const LOOPBACK_NAMES = ['localhost', 'localhost.localdomain', 'ip6-localhost']

/**
 * The host of an `http(s)://` URL: no scheme, no userinfo, no port, lowercased.
 * An IPv6 literal comes back without its brackets.
 */
export function hostOf(url: string): string | null {
  const lower = (url ?? '').trim().toLowerCase()
  const rest = lower.startsWith('http://')
    ? lower.slice(7)
    : lower.startsWith('https://')
      ? lower.slice(8)
      : null
  if (rest === null) return null
  const authority = rest.split(/[/?#]/)[0].split('@').pop() ?? ''
  if (!authority) return null
  if (authority.startsWith('[')) {
    const end = authority.indexOf(']')
    return end > 1 ? authority.slice(1, end) : null
  }
  const host = authority.split(':')[0]
  return host || null
}

function classifyIpv4(host: string): WebhookHostClass | null {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host)
  if (!m) return null
  const [a, b] = [Number(m[1]), Number(m[2])]
  if ([a, b, Number(m[3]), Number(m[4])].some(n => n > 255)) return 'invalid'
  if (a === 127) return 'loopback'
  if (a === 169 && b === 254) return 'linkLocal'
  if (a === 10) return 'private'
  if (a === 172 && b >= 16 && b <= 31) return 'private'
  if (a === 192 && b === 168) return 'private'
  if (a === 100 && b >= 64 && b <= 127) return 'private' // CGNAT / Tailscale
  if (a === 0) return 'private'
  if (a >= 224) return 'private' // multicast + reserved
  return 'public'
}

function classifyIpv6(host: string): WebhookHostClass | null {
  if (!host.includes(':')) return null
  const h = host.toLowerCase()
  // An IPv4-mapped address is its IPv4 self — the classic bypass.
  const mapped = /^::ffff:(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})$/.exec(h)
  if (mapped) return classifyIpv4(mapped[1])
  if (h === '::1' || h === '::') return 'loopback'
  const head = h.split(':')[0]
  if (/^fe[89ab]/.test(head)) return 'linkLocal' // fe80::/10
  if (/^f[cd]/.test(head)) return 'private' // fc00::/7
  if (/^ff/.test(head)) return 'private' // multicast
  return 'public'
}

/** Classify a host by name, without resolving it. Mirrors `classify_host`. */
export function classifyWebhookHost(host: string): WebhookHostClass {
  const h = (host ?? '').trim().replace(/\.$/, '').toLowerCase()
  if (!h) return 'invalid'
  const v4 = classifyIpv4(h)
  if (v4) return v4
  const v6 = classifyIpv6(h)
  if (v6) return v6
  if (LOOPBACK_NAMES.includes(h)) return 'loopback'
  if (LINK_LOCAL_SUFFIXES.some(s => h.endsWith(s))) return 'linkLocal'
  if (PRIVATE_SUFFIXES.some(s => h.endsWith(s))) return 'private'
  // A single label with no dot is a LAN short-name; a public host has a domain.
  if (!h.includes('.')) return 'private'
  return 'public'
}

/** Classify a whole URL by its host. */
export function classifyWebhookUrl(url: string): WebhookHostClass {
  const host = hostOf(url)
  return host === null ? 'invalid' : classifyWebhookHost(host)
}

/** Whether the URL points at the operator's own network (the class needing an opt-in). */
export function isLocalWebhookUrl(url: string): boolean {
  const c = classifyWebhookUrl(url)
  return c === 'loopback' || c === 'private' || c === 'linkLocal'
}

/**
 * The inline field error for a webhook URL, or `null` when it is acceptable.
 * An empty field is acceptable (a webhook is optional).
 */
export function webhookUrlError(url: string): WebhookCopy | null {
  const v = (url ?? '').trim()
  if (!v) return null
  const lower = v.toLowerCase()
  if (!lower.startsWith('http://') && !lower.startsWith('https://')) {
    return {
      key: 'notify.errWebhookUrl',
      fallback: 'Webhook-URL må begynne med https://',
    }
  }
  if (classifyWebhookUrl(v) === 'invalid') {
    return {
      key: 'notify.errWebhookUrl',
      fallback: 'Webhook-URL må begynne med https://',
    }
  }
  // Plaintext is fine for a box in your own building, never for the internet.
  if (lower.startsWith('http://') && !isLocalWebhookUrl(v)) {
    return {
      key: 'notify.errWebhookHttp',
      fallback:
        'Bruk https:// for adresser på internett. http:// er bare tillatt for utstyr på ditt eget nett.',
    }
  }
  return null
}

/**
 * Whether saving `url` should ask the operator to confirm a local address.
 *
 * Only asks when the answer would CHANGE something: a public URL never asks,
 * and an address already allowed does not ask again.
 */
export function needsLocalConfirmation(url: string, alreadyAllowed: boolean): boolean {
  return isLocalWebhookUrl(url) && !alreadyAllowed
}

/**
 * What `webhookAllowLocal` must become for `url`, given the operator's answer.
 *
 * The flag is an opt-in for ONE address, not a mode: typing a public URL clears
 * it, so a later switch back to a LAN address asks again rather than inheriting
 * a permission granted for something else.
 */
export function nextAllowLocal(url: string, confirmed: boolean): boolean {
  return isLocalWebhookUrl(url) ? confirmed : false
}
