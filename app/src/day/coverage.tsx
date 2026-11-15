import { useMemo } from "react"
import { collectorRow, duration, hourLabel, macRow, timeRange, totals, type BarSegment, type BarState, type Ms, type WindowNote } from "../lib/day"
import type { DayResponse } from "../server/day"

const words: Record<BarState, string> = {
  recorded: "Recorded",
  away: "Away (locked, asleep or idle)",
  paused: "Paused",
  gap: "Not recorded",
  future: "Still to come",
  withheld: "Withheld",
}
const textWords: Record<BarState, string> = {
  recorded: "Screen text recorded",
  away: "Away",
  paused: "Screen text paused (locked screen, excluded app or secure input)",
  gap: "No screen text recorded",
  future: "Still to come",
  withheld: "Withheld",
}

function ticks(window: Ms) {
  const start = new Date(window.start)
  const out: number[] = []
  for (let h = 0; h <= 24; h += 3) {
    const at = new Date(start.getFullYear(), start.getMonth(), start.getDate(), h).getTime()
    if (at >= window.start && at <= window.end) out.push(at)
  }
  return out
}

function pct(at: number, window: Ms) {
  return ((at - window.start) / (window.end - window.start)) * 100
}

function Row({
  label,
  segments,
  window,
  describe,
  onPick,
}: {
  label: string
  segments: BarSegment[]
  window: Ms
  describe: (s: BarSegment) => string
  onPick?: (at: number) => void
}) {
  return (
    <div className="cov-row">
      <span className="cov-row-label">{label}</span>
      <div className="cov-track" role="list" aria-label={label}>
        {segments.map((s) => {
          const text = `${describe(s)}, ${timeRange(s.start, s.end)} (${duration(s.end - s.start)})`
          const style = { left: `${pct(s.start, window)}%`, width: `${Math.max(0.15, pct(s.end, window) - pct(s.start, window))}%` }
          return onPick && s.state !== "future" ? (
            <button type="button" role="listitem" key={s.start} className="cov-seg" data-state={s.state} style={style} title={text} aria-label={text} onClick={() => onPick(s.start)} />
          ) : (
            <span role="listitem" key={s.start} className="cov-seg" data-state={s.state} style={style} title={text} aria-label={text} />
          )
        })}
      </div>
    </div>
  )
}

/**
 * Which parts of the day were recorded, which are gaps and which were paused,
 * straight from the server's per-collector coverage. Nothing is smoothed over:
 * a two-second gap is drawn as a gap.
 */
function CoverageBar({
  day,
  window,
  now,
  notes,
  onPick,
  compact,
}: {
  day: DayResponse
  window: Ms
  now: number
  notes?: WindowNote[]
  onPick?: (at: number) => void
  compact?: boolean
}) {
  const mac = useMemo(() => macRow(day, window, now), [day, window, now])
  const text = useMemo(() => collectorRow(day.coverage, "text", window, now), [day, window, now])
  const sums = totals(mac)
  const gaps = mac.filter((s) => s.state === "gap").length
  const account: BarSegment[] | null = notes
    ? notes
        .filter((n) => n.status !== "nothing-recorded")
        .map((n) => ({ start: n.start, end: n.end, state: n.status === "written" ? "recorded" : n.status === "hidden" ? "withheld" : n.status === "future" ? "future" : "gap" }) as BarSegment)
    : null
  const tickList = ticks(window)
  return (
    <div className="coverage" data-compact={compact ? "" : undefined}>
      <div className="cov-rows">
        <Row label="Mac" segments={mac} window={window} describe={(s) => words[s.state]} onPick={onPick} />
        <Row label="Screen text" segments={text} window={window} describe={(s) => textWords[s.state]} onPick={onPick} />
        {account && (
          <Row
            label="Account"
            segments={account}
            window={window}
            describe={(s) =>
              s.state === "recorded" ? "Written up by the AI" : s.state === "withheld" ? "Written up, but withheld from this app" : s.state === "future" ? "Still to come" : "Recorded, not written up yet"
            }
            onPick={onPick}
          />
        )}
        {now > window.start && now < window.end && <span className="cov-now" style={{ left: `calc(var(--cov-label) + (100% - var(--cov-label)) * ${pct(now, window) / 100})` }} aria-hidden />}
      </div>
      <div className="cov-axis" aria-hidden>
        {tickList.map((at) => (
          <span key={at} style={{ left: `${pct(at, window)}%` }}>
            {hourLabel(at)}
          </span>
        ))}
      </div>
      {!compact && (
        <dl className="cov-legend">
          {(["recorded", "away", "paused", "gap"] as const).map((state) => (
            <div key={state} className="cov-legend-item">
              <dt>
                <span className="cov-swatch" data-state={state} aria-hidden />
                {words[state].replace(/ \(.*\)/, "")}
              </dt>
              <dd>
                {duration(sums[state])}
                {state === "gap" && gaps > 0 && <span className="cov-legend-note"> in {gaps === 1 ? "1 stretch" : `${gaps} stretches`}</span>}
              </dd>
            </div>
          ))}
        </dl>
      )}
    </div>
  )
}

export { CoverageBar }
