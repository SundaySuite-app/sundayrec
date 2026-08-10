/**
 * feature-gate-core — deciding what a section is allowed to claim.
 *
 * SundayRec ships several panels whose backend is not in this build: cloud
 * backup needs an OAuth client id that is not compiled in, and e-mail sending
 * is behind a default-off cargo feature. Until tonight those panels looked
 * exactly like working ones — a «Koble til» button that fails, a «Send test»
 * that reports a failure it invented.
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

/** What `email_status` + the keychain mean for the panel. */
export interface EmailFacts {
  /** Compiled with `--features email` (in `default` since v0.10). */
  featureBuilt: boolean
  /** A Gmail refresh token is in the keychain. */
  gmailConnected: boolean
  /** The user has filled in SMTP host + user. */
  smtpConfigured: boolean
  /** An SMTP password is available: stored in the OS keychain, or typed into
   *  the field right now (the backend prefers the typed one). Without it an
   *  authenticated submission gets `missing_password`, so a host + username
   *  alone is NOT a working transport. */
  smtpPasswordAvailable: boolean
}

/**
 * Gate status for the e-mail card.
 *
 * Only two outcomes now: without the cargo feature there is no send path at all,
 * whatever the user types ('unavailable' — saying so is the whole point), and
 * with it the card is LIVE.
 *
 * It deliberately never returns 'unconfigured': `mapGate` makes every non-`ok`
 * status `inert`, and on THIS card the inert controls would include the
 * recipient field, the SMTP fields and the enable toggle — i.e. the only way to
 * configure it. A card that disables itself until it is configured can never be
 * configured. "Not set up yet" is said by a hint line next to «Test e-post»
 * instead, which leaves the controls usable.
 */
export function emailGateStatus(facts: EmailFacts): GateStatus {
  return facts.featureBuilt ? 'ok' : 'unavailable'
}

/** Whether a message could actually leave the machine: a build that can send,
 *  plus either Gmail or a COMPLETE SMTP set (host + user + a password). */
export function hasEmailTransport(facts: EmailFacts): boolean {
  if (!facts.featureBuilt) return false
  if (facts.gmailConnected) return true
  return facts.smtpConfigured && facts.smtpPasswordAvailable
}

/** Whether «Send test» may be pressed: a transport AND somewhere to send it. */
export function canSendTestEmail(facts: EmailFacts, hasRecipient: boolean): boolean {
  return hasEmailTransport(facts) && hasRecipient
}

/** Why «Test e-post» is disabled — so the card can say it out loud instead of
 *  leaving a dead button. `null` when it is pressable. */
export type EmailBlockReason = 'noFeature' | 'noTransport' | 'noRecipient' | null

export function emailBlockReason(facts: EmailFacts, hasRecipient: boolean): EmailBlockReason {
  if (!facts.featureBuilt) return 'noFeature'
  if (!hasEmailTransport(facts)) return 'noTransport'
  if (!hasRecipient) return 'noRecipient'
  return null
}
