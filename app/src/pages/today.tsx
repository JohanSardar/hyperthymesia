import { useEffect, useMemo, useRef, useState } from "react"
import { Link } from "react-router"
import { Logo } from "../connections/logo"
import { describe, happenedAt, sourceLabel } from "../lib/entries"
import { ago, clock } from "../lib/time"
import { api, useEntryStream, usePolled, type LogEntry } from "../server/connections"
import { PageHeader } from "../shell"
import { Button, Card, Empty, Page, PageIntro, Segmented, Skeleton, StatusDot } from "../ui"
import "../connections/connections.css"
import "./today.css"

const KEEP = 400
const WINDOW_MIN = 30

type Row = LogEntry & { fresh: boolean }
type Filter = "all" | "mac" | "connections"

function isMac(instance: string) {
  return instance.startsWith("macos-") || instance.startsWith("companion:")
}

/** Ticks every `ms`, so relative times and the arrival bars stay current. */
function useNow(ms: number) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), ms)
    return () => window.clearInterval(timer)
  }, [ms])
  return now
}

/** A number that rolls to its new value instead of jumping. */
function Count({ value }: { value: number }) {
  const [shown, setShown] = useState(value)
  const from = useRef(value)
  useEffect(() => {
    const start = performance.now()
    const begin = from.current
    if (begin === value) return
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches
    let frame = 0
    const step = (t: number) => {
      const k = reduce ? 1 : Math.min(1, (t - start) / 500)
      const eased = 1 - Math.pow(1 - k, 5)
      const next = Math.round(begin + (value - begin) * eased)
      setShown(next)
      from.current = next
      if (k < 1) frame = requestAnimationFrame(step)
    }
    frame = requestAnimationFrame(step)
    return () => cancelAnimationFrame(frame)
  }, [value])
  return <>{shown.toLocaleString()}</>
}

function TodayPage() {
  const [rows, setRows] = useState<Row[]>([])
  const [loaded, setLoaded] = useState(false)
  const [arrivedToday, setArrivedToday] = useState(0)
  const [filter, setFilter] = useState<Filter>("all")
  const now = useNow(15_000)

  const today = usePolled(api.today, 60_000)
  const connections = usePolled(api.connections, 30_000)

  // Reset the live increment whenever the server's own figure refreshes.
  useEffect(() => setArrivedToday(0), [today.data])

  const stream = useEntryStream((entries, initial) => {
    setLoaded(true)
    if (!entries.length) return
    const incoming = entries.map((e) => ({ ...e, fresh: !initial })).reverse()
    setRows((current) => {
      const seen = new Set(current.map((r) => r.id))
      const added = incoming.filter((r) => !seen.has(r.id))
      return [...added, ...current.map((r) => (r.fresh ? { ...r, fresh: false } : r))].slice(0, KEEP)
    })
    if (!initial && today.data) {
      const start = new Date(today.data.start).getTime()
      setArrivedToday((n) => n + entries.filter((e) => new Date(e.received_at).getTime() >= start).length)
    }
  })

  // The stream's first event can be a bare keepalive when nothing is recorded yet.
  useEffect(() => {
    if (stream === "live") {
      const timer = window.setTimeout(() => setLoaded(true), 1500)
      return () => window.clearTimeout(timer)
    }
  }, [stream])

  const names = useMemo(() => new Map((connections.data ?? []).map((s) => [s.connector_instance, s.name])), [connections.data])

  const visible = useMemo(
    () => rows.filter((r) => filter === "all" || (filter === "mac" ? isMac(r.envelope.connector_instance) : !isMac(r.envelope.connector_instance))),
    [rows, filter],
  )

  // Per-minute arrivals over the last half hour, from what this page has seen.
  const bars = useMemo(() => {
    const buckets = new Array<number>(WINDOW_MIN).fill(0)
    const t = Math.max(now, Date.now())
    for (const r of rows) {
      const age = Math.max(0, Math.floor((t - new Date(r.received_at).getTime()) / 60_000))
      if (age >= 0 && age < WINDOW_MIN) buckets[WINDOW_MIN - 1 - age] += 1
    }
    return buckets
  }, [rows, now])
  const peak = Math.max(1, ...bars)
  const lastTen = bars.slice(-10).reduce((a, b) => a + b, 0)

  const sources = connections.data ?? []
  const ready = sources.filter((s) => s.enabled && s.status === "ready").length
  const settingUp = sources.filter((s) => s.enabled && s.status === "setting_up").length

  const groups = useMemo(() => {
    const out: { hour: string; rows: Row[] }[] = []
    for (const r of visible) {
      const d = new Date(r.received_at)
      d.setMinutes(0, 0, 0)
      const hour = d.toLocaleString(undefined, { hour: "numeric", ...(d.toDateString() === new Date().toDateString() ? {} : { month: "short", day: "numeric" }) })
      const last = out[out.length - 1]
      if (last && last.hour === hour) last.rows.push(r)
      else out.push({ hour, rows: [r] })
    }
    return out
  }, [visible])

  const live = stream === "live"

  return (
    <>
      <PageHeader
        title="Today"
        actions={
          <span className="live-pill" data-state={stream}>
            <StatusDot tone={live ? "live" : stream === "connecting" ? "warning" : "error"} />
            {live ? "Live" : stream === "connecting" ? "Connecting" : "Offline"}
          </span>
        }
      />
      <Page width="dashboard">
        <PageIntro title="Today" description="Everything recorded today, the moment it arrives" />

        <Card className="stat-strip">
          <div className="strip-stat">
            <span className="stat-label">Entries today</span>
            <span className="stat-value">{today.data ? <Count value={today.data.entries_today + arrivedToday} /> : <Skeleton width={56} height={22} />}</span>
            <span className="stat-hint">since midnight</span>
          </div>
          <div className="strip-stat">
            <span className="stat-label">Arriving</span>
            <span className="stat-value">
              <Count value={lastTen} />
            </span>
            <div className="spark" aria-label={`${lastTen} entries in the last 10 minutes`}>
              {bars.map((n, i) => (
                <span key={i} style={{ height: `${Math.max(n ? 12 : 4, (n / peak) * 100)}%` }} data-on={n > 0 ? "" : undefined} />
              ))}
            </div>
            <span className="stat-hint">last 10 min</span>
          </div>
          <div className="strip-stat">
            <span className="stat-label">Sources today</span>
            <span className="stat-value">{today.data ? <Count value={today.data.active_sources} /> : <Skeleton width={32} height={22} />}</span>
            <span className="stat-hint">sent something today</span>
          </div>
          <div className="strip-stat">
            <span className="stat-label">Connections</span>
            <span className="stat-value">
              {connections.data ? (
                <>
                  <Count value={ready} />
                  <span className="stat-of"> / {sources.length}</span>
                </>
              ) : (
                <Skeleton width={40} height={22} />
              )}
            </span>
            <span className="stat-hint">{settingUp ? `${settingUp} setting up` : "ready"}</span>
          </div>
          <div className="strip-stat">
            <span className="stat-label">Last pull</span>
            <span className="stat-value stat-value-text">{today.data ? ago(today.data.last_pull_at, now) : <Skeleton width={72} height={22} />}</span>
            <span className="stat-hint">every 4 hours</span>
          </div>
        </Card>

        <section className="section">
          <div className="section-head">
            <h3 className="h2">Log</h3>
            <Segmented
              value={filter}
              onValueChange={(v) => setFilter(v as Filter)}
              items={[
                { value: "all", label: "All" },
                { value: "mac", label: "This Mac" },
                { value: "connections", label: "Connections" },
              ]}
            />
          </div>

          {!loaded ? (
            <Card className="log">
              {Array.from({ length: 6 }, (_, i) => (
                <div className="log-row log-row-skeleton" key={i}>
                  <Skeleton width={48} />
                  <Skeleton width={72} />
                  <Skeleton width={`${40 + ((i * 17) % 40)}%`} />
                </div>
              ))}
            </Card>
          ) : visible.length === 0 ? (
            <Card>
              <Empty
                title={filter === "connections" ? "Nothing pulled yet" : "No entries yet"}
                description={filter === "connections" ? "Entries from your connections show up here after each pull" : "Entries show up here the moment they're recorded"}
              >
                {filter === "connections" && (
                  <Button render={<Link to="/connections" />} variant="default">
                    Connect something
                  </Button>
                )}
              </Empty>
            </Card>
          ) : (
            <div className="log-groups">
              {groups.map((group) => (
                <div className="log-group" key={group.hour}>
                  <div className="log-hour">{group.hour}</div>
                  <Card className="log">
                    {group.rows.map((row) => (
                      <LogRow key={row.id} row={row} names={names} />
                    ))}
                  </Card>
                </div>
              ))}
            </div>
          )}
        </section>
      </Page>
    </>
  )
}

function LogRow({ row, names }: { row: Row; names: Map<string, string> }) {
  const line = describe(row.envelope)
  const Icon = line.icon
  const instance = row.envelope.connector_instance
  const label = sourceLabel(instance, names)
  const at = happenedAt(row.envelope)
  return (
    <div className="log-row" data-fresh={row.fresh ? "" : undefined}>
      <div className="log-row-inner">
        <time className="log-time" dateTime={at} title={new Date(at).toLocaleString()}>
          {clock(at)}
        </time>
        <span className="log-source" title={instance}>
          {instance.startsWith("source:") ? <Logo name={label} size={24} className="log-logo" /> : <Icon size={13} strokeWidth={1.75} aria-hidden />}
          <span>{label}</span>
        </span>
        <span className="log-text">
          {instance.startsWith("source:") && <Icon size={13} strokeWidth={1.75} aria-hidden className="log-kind" />}
          <span className="log-line">{line.text}</span>
        </span>
        {line.detail && <span className="log-detail">{line.detail}</span>}
      </div>
    </div>
  )
}

export { TodayPage }
