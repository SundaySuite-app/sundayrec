/**
 * feature-gate-core — deciding what a section is allowed to claim.
 *
 * SundayRec ships several panels whose backend is not in this build: cloud
 * backup needs an OAuth client id that is not compiled in, e-mail sending is
 * behind a default-off cargo feature, thumbnails have no Rust side at all, and
 * the live-stats emitter was never written. Until tonight those panels looked
 * exactly like working ones — a «Koble til» button that fails, a «Send test»
 * that reports a failure it invented, a statistics grid frozen at 0.
 *
 * A volunteer cannot tell "you configured this wrong" from "this does not exist
 * yet", and will spend a Saturday evening trying. So each such section states
 * its status once, at the top, and turns its controls off.
 *
 * The mapping from a backend fact to a user-facing status lives here, pure.
 */

/** What the section can actually do right now. */
export type GateStatus =
  /** Backed, configured, usable — no banner, nothing disabled. */
  | 'ok'
  /** The feature exists in this build but has not been set up. */
  | 'unconfigured'
  /** Not present in this build at all. Nothing the user can do about it. */
  | 'unavailable'

export interface GateInput {
  status: GateStatus
  /** Short badge, e.g. «Ikke konfigurert». Defaults per status. */
  chipText?: string
  /** One or two sentences saying what is missing and who can fix it. */
  explanation?: string
  /** Optional extra line — where to look, what to ask for. */
  docsHint?: string
}

/** What the renderer should paint. */
export interface GateView {
  showBanner: boolean
  /** Set `inert` on the section's controls. */
  disabled: boolean
  variant: GateStatus
  chipText: string
  explanation: string
  docsHint?: string
}

/** Norwegian defaults; the DOM layer passes translated strings in `GateInput`. */
const DEFAULT_CHIP: Record<GateStatus, string> = {
  ok: '',
  unconfigured: 'Ikke konfigurert',
  unavailable: 'Ikke tilgjengelig',
}

const DEFAULT_EXPLANATION: Record<GateStatus, string> = {
  ok: '',
  unconfigured: 'Denne funksjonen er ikke satt opp ennå.',
  unavailable: 'Denne funksjonen er ikke bygget inn i denne versjonen.',
}

/**
 * Turn a status into a render plan.
 *
 * `ok` renders nothing and disables nothing — a gate must be invisible when the
 * feature works, or it becomes the wallpaper users learn to ignore.
 */
export function mapGate(input: GateInput): GateView {
  const { status } = input
  if (status === 'ok') {
    return {
      showBanner: false,
      disabled: false,
      variant: 'ok',
      chipText: '',
      explanation: '',
    }
  }
  return {
    showBanner: true,
    disabled: true,
    variant: status,
    chipText: input.chipText?.trim() || DEFAULT_CHIP[status],
    explanation: input.explanation?.trim() || DEFAULT_EXPLANATION[status],
    docsHint: input.docsHint?.trim() || undefined,
  }
}

/** `cloud_is_configured` → status. There is nothing to configure in the UI when
 *  the build has no OAuth client id, so an unconfigured build is 'unavailable'
 *  to the operator and 'unconfigured' only to whoever builds it. */
export function cloudGateStatus(isConfigured: boolean): GateStatus {
  return isConfigured ? 'ok' : 'unconfigured'
}

/** What `email_status` means for the panel. */
export interface EmailFacts {
  /** Compiled with `--features email`. */
  featureBuilt: boolean
  /** A Gmail refresh token is in the keychain. */
  gmailConnected: boolean
  /** The user has filled in SMTP host + user. */
  smtpConfigured: boolean
}

/**
 * Without the cargo feature there is no send path at all, whatever the user
 * types — that is 'unavailable', not 'unconfigured', and saying so is the whole
 * point (the old panel let you fill in an SMTP server and then reported a
 * fabricated send failure).
 */
export function emailGateStatus(facts: EmailFacts): GateStatus {
  if (!facts.featureBuilt) return 'unavailable'
  return facts.gmailConnected || facts.smtpConfigured ? 'ok' : 'unconfigured'
}

/** Whether «Send test» may be pressed. Same rule, expressed once. */
export function canSendTestEmail(facts: EmailFacts, hasRecipient: boolean): boolean {
  return emailGateStatus(facts) === 'ok' && hasRecipient
}

/**
 * Why the live START button is disabled. The button knew; it just never said.
 * Returns null when it is enabled.
 */
export type LiveBlockReason = 'noDestinations' | 'noEnabled' | 'noKey' | null

export interface LiveDestinationFacts {
  /** Destination exists in settings. */
  total: number
  /** Enabled for this session. */
  enabled: number
  /** Enabled AND holding a stream key. */
  ready: number
}

export function liveBlockReason(facts: LiveDestinationFacts): LiveBlockReason {
  if (facts.total === 0) return 'noDestinations'
  if (facts.enabled === 0) return 'noEnabled'
  if (facts.ready === 0) return 'noKey'
  return null
}
