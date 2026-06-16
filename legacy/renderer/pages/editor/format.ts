// Pure time formatters for the editor timeline / cut list / ruler.

export function formatTime(s: number): string {
  const h   = Math.floor(s / 3600)
  const m   = Math.floor((s % 3600) / 60)
  const sec = Math.floor(s % 60)
  return h > 0
    ? `${h}:${String(m).padStart(2,'0')}:${String(sec).padStart(2,'0')}`
    : `${m}:${String(sec).padStart(2,'0')}`
}

export function formatDuration(s: number): string {
  if (s < 1)   return `${(s * 1000).toFixed(0)}ms`
  if (s < 60)  return `${s.toFixed(1)}s`
  // Round to whole seconds FIRST, then split — rounding `s % 60` independently
  // can carry to 60 ("1m 60s" for s=119.6), an out-of-range label.
  if (s < 3600) { const total = Math.round(s); return `${Math.floor(total / 60)}m ${total % 60}s` }
  return `${Math.floor(s / 3600)}t ${Math.floor((s % 3600) / 60)}m`
}
