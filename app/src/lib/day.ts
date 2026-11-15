import type { Coverage, DayResponse, Episode, Interval, Revision, Span } from "../server/day"

/* Local days. A day is local midnight to the next local midnight, so DST days are 23 or 25 hours. */

const pad = (n: number) => String(n).padStart(2, "0")

function dayKey(date: Date) {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}
function todayKey() {
  return dayKey(new Date())
}
function isDayKey(value: string | undefined): value is string {
  return !!value && /^\d{4}-\d{2}-\d{2}$/.test(value) && !Number.isNaN(parseDay(value).getTime())
}
function parseDay(key: string) {
  const [y, m, d] = key.split("-").map(Number)
  return new Date(y, m - 1, d)
}
function dayRange(key: string) {
  const from = parseDay(key)
  const to = new Date(from.getFullYear(), from.getMonth(), from.getDate() + 1)
  return { from, to }
}
function shiftDay(key: string, days: number) {
  const d = parseDay(key)
  return dayKey(new Date(d.getFullYear(), d.getMonth(), d.getDate() + days))
}

const longDay = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric" })
const longDayYear = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric", year: "numeric" })
const shortDay = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" })
const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" })
const secondsFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", second: "2-digit" })
const hourFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric" })

/** "Today", "Yesterday", or "Monday, November 9" (with the year when it isn't this year). */
function dayName(key: string, style: "long" | "short" = "long") {
  if (key === todayKey()) return "Today"
  if (key === shiftDay(todayKey(), -1)) return "Yesterday"
  const d = parseDay(key)
  if (style === "short") return shortDay.format(d)
  return (d.getFullYear() === new Date().getFullYear() ? longDay : longDayYear).format(d)
}
function fullDayName(key: string) {
  const d = parseDay(key)
  return (d.getFullYear() === new Date().getFullYear() ? longDay : longDayYear).format(d)
}

const t = (iso: string | number | Date) => (typeof iso === "string" ? Date.parse(iso) : typeof iso === "number" ? iso : iso.getTime())

function clock(iso: string | number | Date) {
  return timeFormat.format(new Date(t(iso)))
}
function clockSeconds(iso: string | number | Date) {
  return secondsFormat.format(new Date(t(iso)))
}
function hourLabel(ms: number) {
  return hourFormat.format(new Date(ms))
}
function timeRange(start: string | number, end: string | number) {
  return `${clock(start)} – ${clock(end)}`
}
/** "45 s", "12 min", "1 h 5 min". Under a minute stays in seconds: no false rounding up. */
function duration(ms: number) {
  const s = Math.round(ms / 1000)
  if (s < 60) return `${s} s`
  const m = Math.round(s / 60)
  if (m < 60) return `${m} min`
  const h = Math.floor(m / 60)
  return m % 60 ? `${h} h ${m % 60} min` : `${h} h`
}

/* Interval arithmetic in epoch milliseconds. */

type Ms = { start: number; end: number }

function toMs(i: Interval): Ms {
  return { start: t(i.start), end: t(i.end) }
}
function clip(i: Ms, window: Ms): Ms | null {
  const start = Math.max(i.start, window.start)
  const end = Math.min(i.end, window.end)
  return end > start ? { start, end } : null
}
function union(list: Ms[]): Ms[] {
  const sorted = [...list].sort((a, b) => a.start - b.start)
  const out: Ms[] = []
  for (const i of sorted) {
    const last = out[out.length - 1]
    if (last && i.start <= last.end) last.end = Math.max(last.end, i.end)
    else out.push({ ...i })
  }
  return out
}
function total(list: Ms[]) {
  return union(list).reduce((sum, i) => sum + i.end - i.start, 0)
}

/* Coverage: what the Mac recorded, where it recorded nothing, and where it was paused. */

type BarState = "recorded" | "away" | "paused" | "gap" | "future" | "withheld"
type BarSegment = { start: number; end: number; state: BarState }

const awayStates = new Set(["locked", "device_asleep", "display_off", "idle"])

/**
 * Collapses one collector's coverage across every device into one row. At any
 * moment, a device recording beats a pause, and a pause beats no data.
 */
function collectorRow(coverage: Coverage[], collector: string, window: Ms, now: number): BarSegment[] {
  const spans = coverage.filter((c) => c.collector === collector && c.connector.startsWith("macos-agent"))
  const cuts = new Set([window.start, window.end, Math.min(Math.max(now, window.start), window.end)])
  for (const c of spans) {
    cuts.add(Math.min(Math.max(t(c.start), window.start), window.end))
    cuts.add(Math.min(Math.max(t(c.end), window.start), window.end))
  }
  const points = [...cuts].sort((a, b) => a - b)
  const out: BarSegment[] = []
  for (let k = 0; k < points.length - 1; k++) {
    const start = points[k]
    const end = points[k + 1]
    if (end <= start) continue
    const mid = (start + end) / 2
    let state: BarState = mid >= now ? "future" : "gap"
    if (state !== "future") {
      const here = spans.filter((c) => t(c.start) <= mid && t(c.end) > mid).map((c) => c.state)
      if (here.includes("observed")) state = "recorded"
      else if (here.includes("suppressed")) state = "paused"
    }
    const last = out[out.length - 1]
    if (last && last.state === state && last.end === start) last.end = end
    else out.push({ start, end, state })
  }
  return out
}

/** The Mac row: recorded where the focus collector observed; away where presence says locked, asleep or idle. */
function macRow(day: DayResponse, window: Ms, now: number): BarSegment[] {
  const base = collectorRow(day.coverage, "focus", window, now)
  const away = day.lanes
    .filter((lane) => lane.kind === "presence")
    .flatMap((lane) => lane.spans)
    .filter((s) => awayStates.has(s.state ?? s.label))
    .map((s) => clip(toMs(s), window))
    .filter((s): s is Ms => !!s)
  const out: BarSegment[] = []
  const push = (seg: BarSegment) => {
    const last = out[out.length - 1]
    if (last && last.state === seg.state && last.end === seg.start) last.end = seg.end
    else if (seg.end > seg.start) out.push(seg)
  }
  for (const seg of base) {
    if (seg.state !== "recorded") {
      push(seg)
      continue
    }
    let cursor = seg.start
    for (const a of union(away)) {
      if (a.end <= cursor || a.start >= seg.end) continue
      push({ start: cursor, end: Math.max(cursor, a.start), state: "recorded" })
      push({ start: Math.max(cursor, a.start), end: Math.min(seg.end, a.end), state: "away" })
      cursor = Math.min(seg.end, a.end)
    }
    push({ start: cursor, end: seg.end, state: "recorded" })
  }
  return out
}

function totals(row: BarSegment[]) {
  const sums: Record<BarState, number> = { recorded: 0, away: 0, paused: 0, gap: 0, future: 0, withheld: 0 }
  for (const s of row) sums[s.state] += s.end - s.start
  return sums
}

/* The account: day → goals → tasks → episodes, merged across the day's 4-hour windows. */

type Cite = { id: number; revision: number }
type DayEpisode = Episode & { key: string; revision: number; spans: Ms[]; start: number; end: number }
type DayTask = {
  key: string
  title: string
  description: string
  goal: string | null
  open: boolean
  spans: Ms[]
  start: number
  end: number
  episodes: DayEpisode[]
  cites: Cite[]
}
type DayGoal = { key: string; title: string | null; tasks: DayTask[]; spans: Ms[]; start: number; cites: Cite[] }
type WindowNote = { start: number; end: number; status: "written" | "hidden" | "not-written" | "nothing-recorded" | "future"; summary?: string; revision?: Revision; cites: Cite[] }

function citesOf(episodes: DayEpisode[]): Cite[] {
  const seen = new Set<string>()
  const out: Cite[] = []
  for (const e of episodes) {
    for (const id of e.evidence) {
      const k = `${e.revision}:${id}`
      if (!seen.has(k)) {
        seen.add(k)
        out.push({ id, revision: e.revision })
      }
    }
  }
  return out
}

function account(day: DayResponse, window: Ms, now: number) {
  const episodes = new Map<string, DayEpisode>()
  const taskMap = new Map<string, DayTask & { latest: number }>()
  for (const r of day.revisions) {
    const rev = r.revision_id
    for (const e of r.interpretation.episodes ?? []) {
      const spans = (e.intervals ?? []).map(toMs).map((i) => clip(i, window)).filter((i): i is Ms => !!i)
      if (!spans.length) continue
      const key = `${rev}:${e.id}`
      episodes.set(key, { ...e, key, revision: rev, spans, start: Math.min(...spans.map((s) => s.start)), end: Math.max(...spans.map((s) => s.end)) })
    }
  }
  const claimed = new Set<string>()
  for (const r of day.revisions) {
    const rev = r.revision_id
    for (const task of r.interpretation.tasks ?? []) {
      const own = (task.episode_ids ?? []).map((id) => episodes.get(`${rev}:${id}`)).filter((e): e is DayEpisode => !!e)
      const spans = (task.intervals ?? []).map(toMs).map((i) => clip(i, window)).filter((i): i is Ms => !!i)
      if (!own.length && !spans.length) continue
      own.forEach((e) => claimed.add(e.key))
      // Task ids are stable across windows, so one task can span several.
      const existing = taskMap.get(task.id)
      if (existing) {
        existing.episodes.push(...own)
        existing.spans.push(...spans)
        if (rev >= existing.latest) {
          existing.latest = rev
          existing.title = task.title
          existing.description = task.description
          existing.open = task.open
          if (task.goal) existing.goal = task.goal.trim()
        }
      } else {
        taskMap.set(task.id, {
          key: task.id,
          latest: rev,
          title: task.title,
          description: task.description,
          goal: task.goal?.trim() || null,
          open: task.open,
          spans,
          start: 0,
          end: 0,
          episodes: own,
          cites: [],
        })
      }
    }
  }
  const tasks: DayTask[] = [...taskMap.values()].map(({ latest: _latest, ...task }) => {
    const spans = union(task.spans.length ? task.spans : task.episodes.flatMap((e) => e.spans))
    const sorted = [...task.episodes].sort((a, b) => a.start - b.start)
    return { ...task, spans, episodes: sorted, start: spans[0]?.start ?? 0, end: spans[spans.length - 1]?.end ?? 0, cites: citesOf(sorted) }
  })
  const loose = [...episodes.values()].filter((e) => !claimed.has(e.key)).sort((a, b) => a.start - b.start)

  const goalMap = new Map<string, DayTask[]>()
  for (const task of tasks) {
    const k = task.goal ?? ""
    goalMap.set(k, [...(goalMap.get(k) ?? []), task])
  }
  const goals: DayGoal[] = [...goalMap.entries()].map(([k, list]) => {
    const sorted = list.sort((a, b) => a.start - b.start)
    const spans = union(sorted.flatMap((x) => x.spans))
    return { key: k || "(none)", title: k || null, tasks: sorted, spans, start: spans[0]?.start ?? 0, cites: dedupe(sorted.flatMap((x) => x.cites)) }
  })
  // Stated goals first, in the order the day reached them; tasks with no stated goal after.
  goals.sort((a, b) => (a.title === null ? 1 : 0) - (b.title === null ? 1 : 0) || a.start - b.start)

  const recordedSpans = macRow(day, window, now).filter((s) => s.state === "recorded" || s.state === "away")
  const notes: WindowNote[] = day.windows
    .map((w): WindowNote | null => {
      const span = clip(toMs(w), window)
      if (!span) return null
      const revision = day.revisions.find((r) => r.revision_id === w.canonical_revision)
      const inWindow = [...episodes.values()].filter((e) => e.revision === revision?.revision_id)
      const recorded = recordedSpans.some((s) => s.start < span.end && s.end > span.start && s.state === "recorded")
      // A canonical account this login may not read: it used data from sources outside its namespaces.
      const status: WindowNote["status"] = revision
        ? "written"
        : w.canonical_revision !== null
          ? "hidden"
          : span.start >= now
          ? "future"
          : recorded
            ? "not-written"
            : "nothing-recorded"
      const note: WindowNote = { ...span, status, cites: citesOf(inWindow) }
      if (revision) note.revision = revision
      if (revision?.interpretation.summary) note.summary = revision.interpretation.summary
      return note
    })
    .filter((n): n is WindowNote => !!n)
  return { goals, tasks, loose, notes, episodeCount: episodes.size }
}

function dedupe(cites: Cite[]) {
  const seen = new Set<string>()
  return cites.filter((c) => {
    const k = `${c.revision}:${c.id}`
    if (seen.has(k)) return false
    seen.add(k)
    return true
  })
}

/* App names and colours for the timeline. */

function appOf(span: Span) {
  return span.app_name || span.label.split(" — ")[0] || "App"
}
/** A stable hue per app from its name, drawn from the chart tokens. */
function appTone(name: string) {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return (h % 5) + 1
}

export {
  account,
  appOf,
  appTone,
  clip,
  clock,
  clockSeconds,
  collectorRow,
  dayKey,
  dayName,
  dayRange,
  duration,
  fullDayName,
  hourLabel,
  isDayKey,
  macRow,
  parseDay,
  shiftDay,
  timeRange,
  toMs,
  todayKey,
  total,
  totals,
  union,
}
export type { BarSegment, BarState, Cite, DayEpisode, DayGoal, DayTask, Ms, WindowNote }
