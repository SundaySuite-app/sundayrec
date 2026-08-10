import { describe, expect, it } from 'vitest'
import no from '../../locales/no.json'
import en from '../../locales/en.json'
import {
  WARNING_COUNT_PARAMS,
  WARNING_KEYS,
  interpolate,
  toWarningView,
  warningParams,
  warningToastKind,
} from './backend-warning-core'
import type { BackendWarning } from '../../bindings/BackendWarning'

/** A `t` backed by a real locale tree, so the tests exercise the actual keys. */
function localizer(tree: Record<string, unknown>) {
  return (key: string, fallback: string): string => {
    const val = key
      .split('.')
      .reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], tree)
    return (val as string) ?? fallback
  }
}

/** A `tn` backed by a real locale tree: picks the CLDR form for `count` out of
 *  the plural GROUP the key now holds, exactly as `i18n.tn` does. */
function pluralizer(tree: Record<string, unknown>, lang: string) {
  return (key: string, count: number, fallback: string): string => {
    const node = key
      .split('.')
      .reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], tree)
    if (typeof node === 'string') return node
    if (!node || typeof node !== 'object') return fallback
    const group = node as Record<string, string>
    return group[new Intl.PluralRules(lang).select(count)] ?? group.other ?? fallback
  }
}

const t = localizer(no as Record<string, unknown>)
const tn = pluralizer(no as Record<string, unknown>, 'nb-NO')

function warning(over: Partial<BackendWarning> = {}): BackendWarning {
  return {
    code: 'disk_low',
    msg: 'Lite plass igjen',
    severity: 'warn',
    params: {},
    ...over,
  } as BackendWarning
}

describe('code → locale key', () => {
  it('has a key for every code the Rust side can emit', () => {
    // Mirrors `sundayrec_core::notify::code::ALL`, which is asserted to have
    // exactly these seven entries. A backend code with no entry here degrades
    // to the Norwegian `msg` — survivable, but not what we ship.
    expect(Object.keys(WARNING_KEYS).sort()).toEqual([
      'cloud_reauth_required',
      'cloud_upload_failed',
      'device_missing',
      'disk_low',
      'preroll_dead',
      'recovery_skipped',
      'review_overdue',
    ])
  })

  it('every key it points at actually exists in the primary locale', () => {
    for (const key of Object.values(WARNING_KEYS)) {
      expect(t(key, ''), `${key} missing from no.json`).not.toBe('')
    }
  })

  it('every key lives under the notify namespace', () => {
    for (const key of Object.values(WARNING_KEYS)) {
      expect(key.startsWith('notify.')).toBe(true)
    }
  })
})

describe('severity → toast kind', () => {
  it('routes error to the sticky kind and everything else to warn', () => {
    // `error` toasts do not auto-dismiss. A revoked cloud token is exactly the
    // message that must not vanish while the operator looks away.
    expect(warningToastKind('error')).toBe('error')
    expect(warningToastKind('warn')).toBe('warn')
    // Anything unrecognised is a warning, never silently an error.
    expect(warningToastKind(undefined)).toBe('warn')
    expect(warningToastKind('catastrophe')).toBe('warn')
  })
})

describe('parameters', () => {
  it('derives readable gigabytes from the exact byte count', () => {
    // The backend sends bytes because it must not guess at the user's units.
    const params = warningParams(warning({ params: { freeBytes: '1610612736' } }))
    expect(params.freeGb).toBe('1.5')
    expect(params.freeBytes).toBe('1610612736')
  })

  it('leaves freeGb absent when there is no byte count to derive it from', () => {
    expect(warningParams(warning()).freeGb).toBeUndefined()
    expect(warningParams(warning({ params: { freeBytes: 'nonsense' } })).freeGb).toBeUndefined()
  })

  it('interpolates only the placeholders it has values for', () => {
    expect(interpolate('{a} and {b}', { a: 'x' })).toBe('x and {b}')
    // A visible {b} is a bug report; a silently empty sentence is not.
    expect(interpolate('no placeholders', { a: 'x' })).toBe('no placeholders')
  })
})

describe('toWarningView', () => {
  it('localizes on the code and fills in the parameters', () => {
    const view = toWarningView(
      warning({ code: 'disk_low', params: { freeBytes: '1073741824' } }),
      t,
      tn,
    )
    expect(view?.kind).toBe('warn')
    expect(view?.text).toContain('1.0 GB')
    expect(view?.text).not.toContain('{')
  })

  it('names the device the operator has to go and find', () => {
    const view = toWarningView(
      warning({
        code: 'device_missing',
        severity: 'error',
        msg: 'Lydenheten «Qu-5» er ikke tilkoblet.',
        params: { device: 'Qu-5' },
      }),
      t,
      tn,
    )
    expect(view?.kind).toBe('error')
    expect(view?.text).toContain('Qu-5')
  })

  it('really translates — a German user does not get the Norwegian msg', () => {
    // This is the entire reason the payload carries a code at all. The old
    // consumer showed `msg` verbatim, which is always Norwegian.
    const view = toWarningView(
      warning({ code: 'cloud_reauth_required', msg: 'Skylagringen må kobles til på nytt.' }),
      localizer(en as Record<string, unknown>),
      pluralizer(en as Record<string, unknown>, 'en'),
    )
    expect(view?.text).toContain('Cloud storage')
    expect(view?.text).not.toContain('Skylagringen')
  })

  it('falls back to the backend wording for a code it has never heard of', () => {
    // The backend is allowed to learn a new warning before this table does. A
    // true sentence in the wrong language beats silence — and silence is what
    // this channel produced for months.
    const view = toWarningView(
      warning({ code: 'something_new', msg: 'Noe uventet skjedde.' }),
      t,
      tn,
    )
    expect(view?.text).toBe('Noe uventet skjedde.')
  })

  it('falls back to the bare code when there is no wording at all', () => {
    expect(toWarningView(warning({ code: 'something_new', msg: null }), t, tn)?.text).toBe(
      'something_new',
    )
  })

  it('picks the plural form for a count-governed warning', () => {
    // review_overdue reads «har ventet i {days} dager» — one string per count
    // before this. Norwegian needs «1 dag»; Polish needs a third form for 2–4.
    const norsk = (days: string) =>
      toWarningView(
        warning({ code: 'review_overdue', msg: null, params: { days, episode: 'Gudstjeneste' } }),
        t,
        tn,
      )?.text
    expect(norsk('1')).toBe('En episode har ventet i 1 dag på gjennomgang: Gudstjeneste')
    expect(norsk('5')).toBe('En episode har ventet i 5 dager på gjennomgang: Gudstjeneste')
  })

  it('lists every count-governed warning against a real plural group', () => {
    for (const [code, param] of Object.entries(WARNING_COUNT_PARAMS)) {
      const node = WARNING_KEYS[code]
        .split('.')
        .reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], no as Record<string, unknown>)
      expect(typeof node, `${code} → ${WARNING_KEYS[code]} must be a plural group`).toBe('object')
      expect(param.length).toBeGreaterThan(0)
    }
  })

  it('survives every shape a malformed payload can take', () => {
    // This crosses an IPC boundary. A warning that throws while telling you
    // about a problem is worse than the problem.
    expect(toWarningView(undefined, t, tn)).toBeNull()
    expect(toWarningView(null, t, tn)).toBeNull()
    expect(toWarningView('a string', t, tn)).toBeNull()
    expect(toWarningView({}, t, tn)).toBeNull()
    expect(toWarningView({ code: '', msg: '' }, t, tn)).toBeNull()
    // A code with no params still renders — the placeholder simply stays.
    expect(toWarningView({ code: 'device_missing' }, t, tn)?.text).toContain('{device}')
  })
})
