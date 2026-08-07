import { describe, it, expect } from 'vitest'
import {
  createCompanionFeedbackTracker,
  type CompanionSuggestionEvent,
  type CompanionSuggestionKind,
} from './companion-feedback'

function harness() {
  const events: CompanionSuggestionEvent[] = []
  const tracker = createCompanionFeedbackTracker({ write: (e) => events.push(e) })
  return { events, tracker }
}

describe('createCompanionFeedbackTracker', () => {
  it('emits nothing until a batch is closed', () => {
    const { events, tracker } = harness()
    tracker.shown()
    tracker.accepted('title')
    expect(events).toEqual([])
  })

  it('a kind accepted and never touched again finalizes as accepted, not edited', () => {
    const { events, tracker } = harness()
    tracker.shown()
    tracker.accepted('title')
    tracker.reset()
    expect(events).toContainEqual({ kind: 'title', outcome: 'accepted', editedAfterAccept: false })
  })

  it('all three kinds left untouched finalize as left_alone', () => {
    const { events, tracker } = harness()
    tracker.shown()
    tracker.reset()
    expect(events).toHaveLength(3)
    expect(events).toEqual(
      expect.arrayContaining(
        (['title', 'description', 'chapters'] as CompanionSuggestionKind[]).map(kind => ({
          kind, outcome: 'left_alone', editedAfterAccept: false,
        })),
      ),
    )
  })

  it('edited() after accepted() writes accepted+edited immediately, at edit time', () => {
    const { events, tracker } = harness()
    tracker.shown()
    tracker.accepted('description')
    tracker.edited('description')
    // Written the instant the edit was reported — not deferred to reset().
    expect(events).toEqual([{ kind: 'description', outcome: 'accepted', editedAfterAccept: true }])
    tracker.reset()
    // reset() must not re-emit a second event for a kind already decided.
    expect(events.filter(e => e.kind === 'description')).toHaveLength(1)
  })

  it('edited() on a kind that was never accepted is a no-op', () => {
    const { events, tracker } = harness()
    tracker.shown()
    tracker.edited('title') // never accepted
    expect(events).toEqual([])
    tracker.reset()
    expect(events).toContainEqual({ kind: 'title', outcome: 'left_alone', editedAfterAccept: false })
  })

  it('edited() called twice for the same kind writes only once', () => {
    const { events, tracker } = harness()
    tracker.shown()
    tracker.accepted('title')
    tracker.edited('title')
    tracker.edited('title')
    expect(events.filter(e => e.kind === 'title')).toHaveLength(1)
  })

  it('accepted() called twice for the same kind is idempotent', () => {
    const { events, tracker } = harness()
    tracker.shown()
    tracker.accepted('chapters')
    tracker.accepted('chapters')
    tracker.reset()
    expect(events.filter(e => e.kind === 'chapters')).toHaveLength(1)
  })

  it('accepted() after edited() does not resurrect the pre-edit outcome', () => {
    // Defensive: a stray extra click on "use in metadata" after the user has
    // already rewritten the field must not downgrade `edited` back to false.
    const { events, tracker } = harness()
    tracker.shown()
    tracker.accepted('title')
    tracker.edited('title')
    tracker.accepted('title')
    tracker.reset()
    expect(events.filter(e => e.kind === 'title')).toEqual([
      { kind: 'title', outcome: 'accepted', editedAfterAccept: true },
    ])
  })

  it('a mixed batch finalizes each kind independently', () => {
    const { events, tracker } = harness()
    tracker.shown()
    tracker.accepted('title')
    tracker.edited('title')          // accepted, edited
    tracker.accepted('description')  // accepted, not edited
    // 'chapters' never touched          -> left_alone
    tracker.reset()
    expect(events).toEqual(
      expect.arrayContaining([
        { kind: 'title', outcome: 'accepted', editedAfterAccept: true },
        { kind: 'description', outcome: 'accepted', editedAfterAccept: false },
        { kind: 'chapters', outcome: 'left_alone', editedAfterAccept: false },
      ]),
    )
    expect(events).toHaveLength(3)
  })

  it('shown() finalizes the previous batch before starting a new one', () => {
    const { events, tracker } = harness()
    tracker.shown()
    tracker.accepted('title')
    // User clicks "Lag prekenhjelp" again before deciding on description/chapters.
    tracker.shown()
    expect(events).toEqual([
      { kind: 'title', outcome: 'accepted', editedAfterAccept: false },
      { kind: 'description', outcome: 'left_alone', editedAfterAccept: false },
      { kind: 'chapters', outcome: 'left_alone', editedAfterAccept: false },
    ])
  })

  it('reset() with no batch open writes nothing (no companion was ever shown)', () => {
    const { events, tracker } = harness()
    tracker.reset()
    expect(events).toEqual([])
  })

  it('reset() is safe to call twice — the second call must not fabricate a batch', () => {
    // This is the bug the null-vs-empty-batch distinction exists to prevent:
    // without it, reset() leaving an empty pending batch behind would make a
    // LATER reset() (or shown()) re-finalize it and emit bogus left_alone
    // events for suggestions nobody ever saw.
    const { events, tracker } = harness()
    tracker.shown()
    tracker.accepted('title')
    tracker.reset()
    events.length = 0
    tracker.reset()
    expect(events).toEqual([])
  })

  it('accepted()/edited() before any shown() touch nothing (no batch open)', () => {
    const { events, tracker } = harness()
    tracker.accepted('title')
    tracker.edited('title')
    expect(events).toEqual([])
  })
})
