import { useEffect, useState } from "react"
import { request } from "./http"

/** Calls to the owner's day API (`crates/hx-server/src/day_api.rs`). All read-only. */

type Interval = { start: string; end: string }

type DayCount = { day: string; records: number; screen_text: number; focus: number }
type WindowState = { start: string; end: string; canonical_revision: number | null }
type DaysResponse = { days: DayCount[]; windows: WindowState[] }
type Bucket = { at: number; records: number; screen_text: number; focus: number }

/**
 * The server counts records per 15-minute UTC bucket; this groups them into
 * local days with the browser's own time zone rules, so a summer day stays
 * itself after the clocks change. Newest day first.
 */
async function loadDays(): Promise<DaysResponse> {
  const raw = await request<{ buckets: Bucket[]; windows: WindowState[] }>("/v1/days")
  const byDay = new Map<string, DayCount>()
  for (const b of raw.buckets) {
    const d = new Date(b.at * 1000)
    const day = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`
    const row = byDay.get(day) ?? { day, records: 0, screen_text: 0, focus: 0 }
    row.records += b.records
    row.screen_text += b.screen_text
    row.focus += b.focus
    byDay.set(day, row)
  }
  return { days: [...byDay.values()].sort((a, b) => (a.day < b.day ? 1 : -1)), windows: raw.windows }
}

type CoverageState = "observed" | "unknown" | "suppressed"
type Coverage = { device: string; connector: string; collector: string; state: CoverageState; start: string; end: string }

type Span = {
  start: string
  end: string
  label: string
  evidence: number[]
  evidence_count?: number
  uncertain: boolean
  app_name?: string
  window_title?: string
  app_bundle_id?: string
  url?: string
  state?: string
  basis?: string
}
type Lane = { device: string; kind: string; spans: Span[] }

type Point = {
  at: string
  kind: string
  label: string
  evidence: number
  device: string
  app_name?: string
  window_title?: string
  app_bundle_id?: string
  surface?: string
  source?: string
  role?: string
  chars?: number
  preview?: string
}

type Claim = { actor: string; action: string; object: string; outcome: string; basis: string; evidence: number[]; outcome_evidence?: number[] }
type Episode = { id: string; intervals: Interval[]; title: string; description: string; claims: Claim[]; evidence: number[]; confidence: number }
type Task = {
  id: string
  title: string
  description: string
  intervals: Interval[]
  episode_ids: string[]
  goal?: string | null
  aliases?: string[]
  confidence: number
  open: boolean
}
type Interpretation = {
  window: Interval
  episodes: Episode[]
  tasks: Task[]
  summary: string
  provenance?: { model?: string; prompt_version?: string; created_at?: string }
  check_failures?: { item: string; reason: string }[]
}
type Revision = { revision_id: number; has_agent_run: boolean; model: string | null; interpretation: Interpretation }

type DayResponse = {
  window: Interval
  observed_through: string | null
  coverage: Coverage[]
  lanes: Lane[]
  points: Point[]
  /** [minute start, keys, clicks, scrolls]: counts only, never which keys. */
  activity: [string, number, number, number][]
  aggregates: { day: string; label: string; seconds_estimated: number; evidence: number[] }[]
  revisions: Revision[]
  windows: WindowState[]
}

type StoredEnvelope = {
  kind: string
  connector_instance: string
  device?: string | null
  sensitivity?: string
  time: { source_start?: string; source_end?: string | null; observed_at: string; utc_offset_min?: number }
  payload: Record<string, unknown>
}
type StoredRecord = {
  id: number
  envelope: StoredEnvelope
  sensitivity: string
  blobs?: { sha256: string; evidence_expired: boolean }[]
  derived_evidence?: { id: number; envelope: StoredEnvelope; sensitivity: string }[]
  source_event_id?: number
}
type Seen =
  | { revision: number; unavailable: true }
  | { revision: number; digest_lines: string[]; snapshot: string | null; has_digest: boolean; unavailable?: undefined }
type CiteResponse = {
  record: StoredRecord
  seen: Seen | null
  screen_text: { id: number; envelope: StoredEnvelope }[]
  screen_text_total: number
}

type EntryHit = {
  kind: "task" | "episode" | "summary"
  revision_id: number
  id?: string
  title: string
  description?: string
  goal?: string | null
  start: string
  end: string
  evidence?: number[] | null
}
type RecordHit = { id: number; at: string; kind: string | null; connector_instance: string | null; app: string | null; title: string | null; snippet: string }
type FindResponse = { entries: EntryHit[]; records: RecordHit[] }

const dayApi = {
  days: loadDays,
  day: (from: Date, to: Date) => request<DayResponse>(`/v1/day?from=${encodeURIComponent(from.toISOString())}&to=${encodeURIComponent(to.toISOString())}`),
  cite: (id: number, revision?: number) => request<CiteResponse>(`/v1/cite/${id}${revision ? `?revision=${revision}` : ""}`),
  find: (q: string) => request<FindResponse>(`/v1/find?q=${encodeURIComponent(q)}`),
}

// Days already loaded this visit. A past day doesn't change; today is reloaded.
const dayCache = new Map<string, DayResponse>()

type Load<T> = { data?: T; error?: Error; loading: boolean }

/** Loads `key` once per visit, keeping the last result while a new key loads. */
function useLoad<T>(key: string | null, load: () => Promise<T>, cache?: Map<string, T>, refreshMs = 0): Load<T> {
  const [state, setState] = useState<Load<T>>(() => ({ data: key ? cache?.get(key) : undefined, loading: !!key }))
  useEffect(() => {
    if (!key) return
    let alive = true
    const cached = cache?.get(key)
    setState({ data: cached, loading: !cached })
    const run = () =>
      load().then(
        (data) => {
          cache?.set(key, data)
          if (alive) setState({ data, loading: false })
        },
        (error: Error) => alive && setState((s) => ({ data: s.data, error, loading: false })),
      )
    void run()
    const timer = refreshMs ? window.setInterval(() => document.visibilityState === "visible" && void run(), refreshMs) : 0
    return () => {
      alive = false
      window.clearInterval(timer)
    }
    // `load` is derived from `key`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, refreshMs])
  return state
}

export { dayApi, dayCache, useLoad }
export type {
  Claim,
  CiteResponse,
  Coverage,
  CoverageState,
  DayCount,
  DayResponse,
  DaysResponse,
  EntryHit,
  Episode,
  FindResponse,
  Interpretation,
  Interval,
  Lane,
  Point,
  RecordHit,
  Revision,
  Seen,
  Span,
  StoredEnvelope,
  StoredRecord,
  Task,
  WindowState,
}
