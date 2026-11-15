import { CalendarDays, ChevronLeft, ChevronRight, Radio, ZoomIn, ZoomOut } from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"
import { Link, useNavigate, useParams, useSearchParams } from "react-router"
import { useCitations } from "../day/citations"
import { CoverageBar } from "../day/coverage"
import { DayPicker, DaysSidebar } from "../day/days"
import {
  appOf,
  appTone,
  clip,
  clock,
  clockSeconds,
  dayName,
  dayRange,
  duration,
  fullDayName,
  hourLabel,
  isDayKey,
  shiftDay,
  timeRange,
  todayKey,
  toMs,
  type Ms,
} from "../lib/day"
import { dayApi, dayCache, useLoad, type DayResponse, type Point, type Span } from "../server/day"
import { PageHeader, useRegisterCommands } from "../shell"
import { Button, Callout, Card, Empty, Page, Segmented, Skeleton } from "../ui"
import "../day/day.css"

const zooms = [
  { value: "day", label: "Whole day", ms: 0 },
  { value: "6h", label: "6 h", ms: 6 * 3600_000 },
  { value: "2h", label: "2 h", ms: 2 * 3600_000 },
  { value: "30m", label: "30 min", ms: 30 * 60_000 },
] as const
type Zoom = (typeof zooms)[number]["value"]

const steps = [5, 10, 15, 30, 60, 120, 180].map((m) => m * 60_000)

function tickList(view: Ms) {
  const span = view.end - view.start
  const step = steps.find((s) => span / s <= 8) ?? 3 * 3600_000
  const out: number[] = []
  const d = new Date(view.start)
  // Align to local midnight, then step.
  const midnight = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
  for (let at = midnight + Math.ceil((view.start - midnight) / step) * step; at <= view.end; at += step) out.push(at)
  return out
}

function pct(at: number, view: Ms) {
  return ((at - view.start) / (view.end - view.start)) * 100
}

const presenceWords: Record<string, string> = { active: "At the Mac", idle: "Idle", locked: "Locked", device_asleep: "Asleep", display_off: "Display off" }
const attentionWords: Record<string, string> = {
  engaged: "Typing or clicking",
  reading: "Reading",
  watching: "Watching",
  probably_away: "Probably away",
  locked_or_asleep: "Locked or asleep",
  unknown: "Unknown",
}

function SpanLane({
  label,
  spans,
  view,
  tone,
  text,
  describe,
  onPick,
  selected,
}: {
  label: string
  spans: Span[]
  view: Ms
  tone: (s: Span) => string
  text?: (s: Span) => string
  describe: (s: Span) => string
  onPick: (s: Span) => void
  selected?: number
}) {
  return (
    <div className="lane">
      <span className="lane-label">{label}</span>
      <div className="lane-track">
        {spans.map((s, i) => {
          const c = clip(toMs(s), view)
          if (!c) return null
          const left = pct(c.start, view)
          const width = Math.max(0.12, pct(c.end, view) - left)
          const title = `${describe(s)} · ${timeRange(s.start, s.end)} (${duration(Date.parse(s.end) - Date.parse(s.start))})`
          return (
            <button
              key={`${s.start}-${i}`}
              type="button"
              className="lane-span"
              data-tone={tone(s)}
              data-selected={selected !== undefined && s.evidence[0] === selected ? "" : undefined}
              data-uncertain={s.uncertain ? "" : undefined}
              style={{ left: `${left}%`, width: `${width}%` }}
              title={title}
              aria-label={title}
              onClick={() => onPick(s)}
            >
              {text && width > 6 && <span className="lane-span-text">{text(s)}</span>}
            </button>
          )
        })}
      </div>
    </div>
  )
}

function TickLane({ label, points, view, tone, onPick, describe }: { label: string; points: Point[]; view: Ms; tone: string; onPick: (p: Point) => void; describe: (p: Point) => string }) {
  return (
    <div className="lane lane-ticks">
      <span className="lane-label">{label}</span>
      <div className="lane-track">
        {points.map((p) => {
          const at = Date.parse(p.at)
          if (at < view.start || at > view.end) return null
          const title = `${describe(p)} · ${clockSeconds(at)}`
          return <button key={p.evidence} type="button" className="lane-tick" data-tone={tone} style={{ left: `${pct(at, view)}%` }} title={title} aria-label={title} onClick={() => onPick(p)} />
        })}
      </div>
    </div>
  )
}

function ActivityLane({ activity, view }: { activity: DayResponse["activity"]; view: Ms }) {
  // Buckets sized so the lane has at most ~240 bars.
  const bucket = Math.max(60_000, Math.ceil((view.end - view.start) / 240 / 60_000) * 60_000)
  const bars = new Map<number, number>()
  for (const [at, keys, clicks, scrolls] of activity) {
    const t = Date.parse(at)
    if (t < view.start || t >= view.end) continue
    const slot = view.start + Math.floor((t - view.start) / bucket) * bucket
    bars.set(slot, (bars.get(slot) ?? 0) + keys + clicks + Math.min(scrolls, 50))
  }
  const peak = Math.max(1, ...bars.values())
  return (
    <div className="lane lane-activity">
      <span className="lane-label">Input</span>
      <div className="lane-track" title="Keys, clicks and scrolls per stretch. Only counts are stored, never which keys.">
        {[...bars.entries()].map(([at, n]) => (
          <span key={at} className="lane-bar" style={{ left: `${pct(at, view)}%`, width: `${Math.max(0.2, (bucket / (view.end - view.start)) * 100 - 0.05)}%`, height: `${Math.max(8, (n / peak) * 100)}%` }} />
        ))}
      </div>
    </div>
  )
}

function textLabel(p: Point) {
  const where = p.app_name || p.surface || p.app_bundle_id || "Screen"
  return `${p.kind === "media.still" ? "Screenshot text" : "Screen text"} · ${where}${p.preview ? ` · ${p.preview}` : ""}`
}

function TimelinePage() {
  const params = useParams()
  const navigate = useNavigate()
  const [search, setSearch] = useSearchParams()
  const key = isDayKey(params.day) ? params.day : todayKey()
  const isToday = key === todayKey()
  const window = useMemo(() => {
    const { from, to } = dayRange(key)
    return { start: from.getTime(), end: to.getTime() }
  }, [key])
  const day = useLoad(`day:${key}`, () => dayApi.day(new Date(window.start), new Date(window.end)), dayCache, isToday ? 120_000 : 0)
  const data = day.data && Date.parse(day.data.window.start) === window.start ? day.data : undefined
  const [now] = useState(() => Date.now())
  const at = search.get("at")
  const atMs = at ? Date.parse(at) : NaN
  const [zoom, setZoom] = useState<Zoom>(Number.isFinite(atMs) ? "2h" : "day")
  const [center, setCenter] = useState<number>(Number.isFinite(atMs) ? atMs : (window.start + window.end) / 2)
  const [selected, setSelected] = useState<number | undefined>()
  const { open, sheet } = useCitations()
  const listRef = useRef<HTMLDivElement>(null)

  // A new day or a new ?at= moves the view.
  useEffect(() => {
    if (Number.isFinite(atMs)) {
      setZoom("2h")
      setCenter(atMs)
    } else {
      setZoom("day")
      setCenter((window.start + window.end) / 2)
    }
  }, [key, atMs, window.start, window.end])

  const view = useMemo<Ms>(() => {
    const span = zooms.find((z) => z.value === zoom)!.ms || window.end - window.start
    let start = center - span / 2
    start = Math.max(window.start, Math.min(start, window.end - span))
    return { start, end: start + span }
  }, [zoom, center, window])

  const lanes = useMemo(() => {
    const by = (kind: string) => (data?.lanes ?? []).filter((l) => l.kind === kind).flatMap((l) => l.spans)
    return {
      focus: by("focus").sort((a, b) => Date.parse(a.start) - Date.parse(b.start)),
      presence: by("presence"),
      attention: by("attention"),
      others: (data?.lanes ?? []).filter((l) => !["focus", "presence", "attention"].includes(l.kind)),
    }
  }, [data])
  const texts = useMemo(() => (data?.points ?? []).filter((p) => p.kind === "text.snapshot" || p.kind === "media.still"), [data])
  const clicks = useMemo(() => (data?.points ?? []).filter((p) => p.kind === "ui.click"), [data])
  const otherPoints = useMemo(() => (data?.points ?? []).filter((p) => !["text.snapshot", "media.still", "ui.click", "focus.span", "presence.span", "media.chunk", "policy.gap"].includes(p.kind)), [data])

  // Select the moment that holds ?at= once the day is loaded.
  useEffect(() => {
    if (!data || !Number.isFinite(atMs)) return
    const hit = lanes.focus.find((s) => Date.parse(s.start) <= atMs && Date.parse(s.end) > atMs)
    if (hit) setSelected(hit.evidence[0])
  }, [data, atMs, lanes.focus])
  useEffect(() => {
    if (selected === undefined) return
    listRef.current?.querySelector(`[data-moment="${selected}"]`)?.scrollIntoView({ block: "nearest" })
  }, [selected, view])

  const moments = useMemo(() => {
    const inView = lanes.focus.filter((s) => clip(toMs(s), view))
    return inView.map((s) => {
      const span = toMs(s)
      const count = texts.filter((p) => {
        const t = Date.parse(p.at)
        return t >= span.start && t < span.end
      }).length
      return { span: s, count }
    })
  }, [lanes.focus, texts, view])

  const exposure = useMemo(() => {
    const sums = new Map<string, number>()
    for (const s of lanes.focus) {
      const c = clip(toMs(s), view)
      if (!c) continue
      const app = appOf(s)
      sums.set(app, (sums.get(app) ?? 0) + c.end - c.start)
    }
    return [...sums.entries()].sort((a, b) => b[1] - a[1])
  }, [lanes.focus, view])
  const [windowExposure, setWindowExposure] = useState(false)
  const titleExposure = useMemo(() => {
    const sums = new Map<string, number>()
    for (const s of lanes.focus) {
      const c = clip(toMs(s), view)
      if (!c) continue
      sums.set(s.label, (sums.get(s.label) ?? 0) + c.end - c.start)
    }
    return [...sums.entries()].sort((a, b) => b[1] - a[1])
  }, [lanes.focus, view])

  const openSpan = (s: Span, what: string) => {
    if (!s.evidence.length) return
    setSelected(s.evidence[0])
    open({
      title: what,
      description: `${timeRange(s.start, s.end)} · ${duration(Date.parse(s.end) - Date.parse(s.start))}`,
      sources: s.evidence.slice(0, 1).map((id) => ({ id })),
    })
  }
  const openPoint = (p: Point, what: string) => open({ title: what, description: clockSeconds(p.at), sources: [{ id: p.evidence }] })

  const zoomIndex = zooms.findIndex((z) => z.value === zoom)
  const pan = (dir: number) => setCenter(center + dir * ((view.end - view.start) * 0.75))
  const go = (k: string) => {
    setSearch({})
    navigate(`/timeline/${k}`)
  }

  useRegisterCommands([
    { id: "timeline:prev", label: "Previous day", group: "Timeline", icon: ChevronLeft, run: () => go(shiftDay(key, -1)) },
    ...(isToday ? [] : [{ id: "timeline:next", label: "Next day", group: "Timeline", icon: ChevronRight, run: () => go(shiftDay(key, 1)) }]),
    { id: "timeline:in", label: "Zoom in", group: "Timeline", icon: ZoomIn, run: () => setZoom(zooms[Math.min(zooms.length - 1, zoomIndex + 1)].value) },
    { id: "timeline:out", label: "Zoom out", group: "Timeline", icon: ZoomOut, run: () => setZoom(zooms[Math.max(0, zoomIndex - 1)].value) },
    { id: "timeline:day", label: `Open the day page for ${dayName(key) === "Today" ? "today" : dayName(key)}`, group: "Timeline", icon: CalendarDays, run: () => navigate(isToday ? "/today" : `/today/${key}`) },
  ])

  const hasRecords = !!data && (data.lanes.some((l) => l.spans.length) || data.points.length > 0)
  const ticks = tickList(view)
  const shownMoments = moments.slice(0, 400)

  return (
    <>
      <PageHeader
        title={`Timeline · ${dayName(key)}`}
        actions={
          <>
            <DayPicker current={key} base="/timeline" />
            <Button size="sm" variant="ghost" render={<Link to={isToday ? "/today" : `/today/${key}`} />}>
              <CalendarDays size={14} aria-hidden /> Day page
            </Button>
            {isToday && (
              <Button size="sm" variant="ghost" render={<Link to="/today/live" />}>
                <Radio size={14} aria-hidden /> Live log
              </Button>
            )}
          </>
        }
      />
      <DaysSidebar current={key} base="/timeline" />
      <Page width="dashboard" className="timeline-page">
        <header className="day-intro">
          <h2 className="day-title">{fullDayName(key)}</h2>
          <p className="day-subtitle">The raw record: what was in front, when, and what was on screen. Gaps are unknown, not idle.</p>
        </header>
        {day.error && !data ? (
          <Callout tone="danger" title="This day couldn't be loaded">
            {day.error.message}
          </Callout>
        ) : !data ? (
          <Skeleton width="100%" height={240} />
        ) : !hasRecords ? (
          <>
            <Card className="day-coverage">
              <CoverageBar day={data} window={window} now={now} compact />
            </Card>
            <Card>
              <Empty title="Nothing was recorded on this day" description="There is no record to show. Pick another day from the list." />
            </Card>
          </>
        ) : (
          <>
            <Card className="day-coverage">
              <CoverageBar
                day={data}
                window={window}
                now={now}
                compact
                onPick={(t) => {
                  setZoom(zoom === "day" ? "2h" : zoom)
                  setCenter(t)
                }}
              />
            </Card>

            <div className="timeline-controls">
              <Segmented value={zoom} onValueChange={(v) => setZoom(v as Zoom)} items={zooms.map((z) => ({ value: z.value, label: z.label }))} />
              <div className="timeline-pan">
                <Button size="sm" variant="ghost" disabled={view.start <= window.start} onClick={() => pan(-1)} aria-label="Earlier">
                  <ChevronLeft size={14} aria-hidden />
                </Button>
                <span className="timeline-range">{zoom === "day" ? "Whole day" : timeRange(view.start, view.end)}</span>
                <Button size="sm" variant="ghost" disabled={view.end >= window.end} onClick={() => pan(1)} aria-label="Later">
                  <ChevronRight size={14} aria-hidden />
                </Button>
              </div>
            </div>

            <Card className="lanes">
              <div className="lanes-axis" aria-hidden>
                <span className="lane-label" />
                <div className="lane-track">
                  {ticks.map((t) => (
                    <span key={t} style={{ left: `${pct(t, view)}%` }}>
                      {view.end - view.start > 6 * 3600_000 ? hourLabel(t) : clock(t)}
                    </span>
                  ))}
                </div>
              </div>
              <SpanLane label="Apps" spans={lanes.focus} view={view} tone={(s) => String(appTone(appOf(s)))} text={appOf} describe={(s) => appOf(s)} onPick={(s) => openSpan(s, s.label)} selected={selected} />
              <SpanLane
                label="Windows"
                spans={lanes.focus}
                view={view}
                tone={() => "neutral"}
                text={(s) => s.window_title || appOf(s)}
                describe={(s) => s.window_title || s.label}
                onPick={(s) => openSpan(s, s.label)}
                selected={selected}
              />
              <TickLane label="Screen text" points={texts} view={view} tone="text" describe={textLabel} onPick={(p) => openPoint(p, textLabel(p))} />
              <TickLane label="Clicks" points={clicks} view={view} tone="click" describe={(p) => `Click${p.label && p.label !== "ui.click" ? ` “${p.label}”` : ""}${p.app_bundle_id ? ` · ${p.app_bundle_id}` : ""}`} onPick={(p) => openPoint(p, "Click")} />
              <ActivityLane activity={data.activity} view={view} />
              <SpanLane
                label="Presence"
                spans={lanes.presence}
                view={view}
                tone={(s) => (s.state === "active" || s.label === "active" ? "on" : "off")}
                describe={(s) => presenceWords[s.state ?? s.label] ?? s.label}
                onPick={(s) => openSpan(s, presenceWords[s.state ?? s.label] ?? s.label)}
              />
              <SpanLane
                label="Attention"
                spans={lanes.attention}
                view={view}
                tone={(s) => (s.label === "engaged" ? "on" : s.label === "reading" || s.label === "watching" ? "mid" : "off")}
                describe={(s) => `${attentionWords[s.label] ?? s.label} (inferred from input${s.evidence_count ? `, ${s.evidence_count} records` : ""})`}
                onPick={(s) => openSpan(s, `${attentionWords[s.label] ?? s.label} (inferred)`)}
              />
              {lanes.others.map((lane) => (
                <SpanLane key={`${lane.device}-${lane.kind}`} label={lane.kind.charAt(0).toUpperCase() + lane.kind.slice(1)} spans={lane.spans} view={view} tone={() => "neutral"} text={(s) => s.label} describe={(s) => s.label} onPick={(s) => openSpan(s, s.label)} />
              ))}
              {otherPoints.length > 0 && <TickLane label="Other" points={otherPoints} view={view} tone="click" describe={(p) => p.label} onPick={(p) => openPoint(p, p.label)} />}
            </Card>

            <div className="timeline-columns">
              <section className="day-section">
                <h3 className="day-section-title">
                  Moments <span className="day-section-count">{moments.length}</span>
                </h3>
                <p className="line-meta">Each time a window came to the front. Open one to see the screen text recorded while it was there.</p>
                {moments.length === 0 ? (
                  <p className="line-meta">Nothing was in front in this stretch.</p>
                ) : (
                  <div className="moments" ref={listRef}>
                    {shownMoments.map(({ span: s, count }) => (
                      <button
                        key={`${s.start}-${s.evidence[0]}`}
                        type="button"
                        className="moment"
                        data-moment={s.evidence[0]}
                        data-selected={s.evidence[0] === selected ? "" : undefined}
                        onClick={() => openSpan(s, s.label)}
                      >
                        <span className="line-time">{clockSeconds(s.start)}</span>
                        <span className="moment-app" data-tone={appTone(appOf(s))}>
                          {appOf(s)}
                        </span>
                        <span className="moment-title">{s.window_title || "No window title"}</span>
                        <span className="moment-meta">
                          {duration(Date.parse(s.end) - Date.parse(s.start))}
                          {count > 0 ? ` · ${count} text` : ""}
                        </span>
                      </button>
                    ))}
                    {moments.length > shownMoments.length && <p className="line-meta">Showing the first {shownMoments.length}. Zoom in to see the rest.</p>}
                  </div>
                )}
              </section>
              <section className="day-section">
                <div className="exposure-head">
                  <h3 className="day-section-title">Time in front</h3>
                  <Segmented
                    value={windowExposure ? "window" : "app"}
                    onValueChange={(v) => setWindowExposure(v === "window")}
                    items={[
                      { value: "app", label: "Apps" },
                      { value: "window", label: "Windows" },
                    ]}
                  />
                </div>
                <p className="line-meta">For {view.end - view.start >= window.end - window.start ? "the whole day" : timeRange(view.start, view.end)}. Time in front, not time paying attention.</p>
                <ol className="exposure">
                  {(windowExposure ? titleExposure : exposure).slice(0, 25).map(([name, ms], _i, list) => (
                    <li key={name}>
                      <span className="exposure-name" title={name}>
                        {name}
                      </span>
                      <span className="exposure-bar" aria-hidden>
                        <span style={{ width: `${(ms / list[0][1]) * 100}%` }} data-tone={windowExposure ? undefined : appTone(name)} />
                      </span>
                      <span className="exposure-time">{duration(ms)}</span>
                    </li>
                  ))}
                </ol>
                {data.aggregates.length > 0 && (
                  <>
                    <h4 className="record-section-title">Day totals from other devices</h4>
                    <ol className="exposure">
                      {data.aggregates.map((a) => (
                        <li key={`${a.day}-${a.label}`}>
                          <span className="exposure-name">{a.label}</span>
                          <span className="exposure-time">about {duration(a.seconds_estimated * 1000)}</span>
                        </li>
                      ))}
                    </ol>
                  </>
                )}
              </section>
            </div>
          </>
        )}
      </Page>
      {sheet}
    </>
  )
}

export { TimelinePage }
