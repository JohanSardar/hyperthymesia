import { RefreshCw } from "lucide-react"
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react"
import { api, type Pass, type Pull, type Source } from "../server/api"
import { PageHeader } from "../shell"
import { Badge, Button, Empty, IconButton, Page, PageIntro, SearchInput, Segmented, Skeleton, StatusDot } from "../ui"
import "./runs.css"

type Tone = "neutral" | "primary" | "success" | "warning" | "danger"

const at = (us: number) => new Date(us / 1000)

function ago(us: number, now: number) {
  const seconds = Math.round((now - us / 1000) / 1000)
  if (seconds < 0) {
    const ahead = -seconds
    if (ahead < 60) return "in <1m"
    if (ahead < 3600) return `in ${Math.round(ahead / 60)}m`
    return `in ${Math.round(ahead / 3600)}h`
  }
  if (seconds < 45) return "now"
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))}m`
  if (seconds < 86400) return `${Math.round(seconds / 3600)}h`
  if (seconds < 86400 * 30) return `${Math.round(seconds / 86400)}d`
  return at(us).toLocaleDateString(undefined, { month: "short", day: "numeric" })
}

const time = (date: Date) => date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })

function windowLabel(us: number) {
  const start = at(us)
  const end = new Date(start.getTime() + 4 * 3600 * 1000)
  const today = new Date()
  const yesterday = new Date(today.getTime() - 86400 * 1000)
  const day =
    start.toDateString() === today.toDateString()
      ? "Today"
      : start.toDateString() === yesterday.toDateString()
        ? "Yesterday"
        : start.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })
  return `${day}, ${time(start)}–${time(end)}`
}

function duration(from: number, to: number) {
  const seconds = Math.max(0, Math.round((to - from) / 1e6))
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`
}

function passState(pass: Pass, now: number): { label: string; tone: Tone; live?: boolean } {
  switch (pass.state) {
    case "running":
      return { label: "Running", tone: "primary", live: true }
    case "done":
      return pass.has_error ? { label: "Done with errors", tone: "warning" } : { label: "Done", tone: "success" }
    case "failed":
      return { label: "Failed", tone: "danger" }
    case "ready":
      if (pass.attempts > 0) return { label: "Retrying", tone: "warning" }
      return pass.available_at && pass.available_at / 1000 > now ? { label: "Scheduled", tone: "neutral" } : { label: "Queued", tone: "neutral" }
    default:
      return { label: pass.state, tone: "neutral" }
  }
}

function pullState(pull: Pull): { label: string; tone: Tone; live?: boolean } {
  switch (pull.status) {
    case "running":
      return { label: "Running", tone: "primary", live: true }
    case "succeeded":
      return { label: "Done", tone: "success" }
    case "failed":
      return { label: "Failed", tone: "danger" }
    case "stopped":
      return { label: "Stopped", tone: "warning" }
    default:
      return { label: pull.status, tone: "neutral" }
  }
}

function State({ label, tone, live }: { label: string; tone: Tone; live?: boolean }) {
  return (
    <Badge tone={tone}>
      {live && <StatusDot tone="live" />}
      {label}
    </Badge>
  )
}

function RunsPage() {
  const [tab, setTab] = useState<"passes" | "pulls">("passes")
  const [passes, setPasses] = useState<Pass[] | null>(null)
  const [pulls, setPulls] = useState<Pull[] | null>(null)
  const [cursor, setCursor] = useState<{ pass: number | null; pull: number | null }>({ pass: null, pull: null })
  const [sources, setSources] = useState<Map<string, string>>(new Map())
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState("")
  const [now, setNow] = useState(() => Date.now())
  const [loadingMore, setLoadingMore] = useState(false)

  // Refresh the newest page; rows and cursors from "Load older" stay.
  const load = useCallback(async () => {
    try {
      const page = await api.runs()
      const merge = <T extends { id: number }>(fresh: T[], current: T[] | null) => {
        const oldest = fresh.at(-1)?.id
        return oldest === undefined ? fresh : [...fresh, ...(current ?? []).filter((row) => row.id < oldest)]
      }
      setPasses((current) => merge(page.passes, current))
      setPulls((current) => merge(page.pulls, current))
      setCursor((current) => ({
        pass: current.pass !== null && page.next_pass_before !== null && current.pass < page.next_pass_before ? current.pass : page.next_pass_before,
        pull: current.pull !== null && page.next_pull_before !== null && current.pull < page.next_pull_before ? current.pull : page.next_pull_before,
      }))
      setError(null)
      setNow(Date.now())
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Runs didn't load")
    }
  }, [])

  useEffect(() => {
    void load()
    api.sources().then(
      (list: Source[]) => setSources(new Map(list.map((s) => [s.id, s.name]))),
      () => {},
    )
    const timer = window.setInterval(load, 30_000)
    return () => window.clearInterval(timer)
  }, [load])

  const more = async () => {
    setLoadingMore(true)
    try {
      if (tab === "passes" && cursor.pass) {
        const page = await api.runs({ pass_before: cursor.pass })
        setPasses((current) => [...(current ?? []), ...page.passes])
        setCursor((c) => ({ ...c, pass: page.passes.length ? page.next_pass_before : null }))
      } else if (tab === "pulls" && cursor.pull) {
        const page = await api.runs({ pull_before: cursor.pull })
        setPulls((current) => [...(current ?? []), ...page.pulls])
        setCursor((c) => ({ ...c, pull: page.pulls.length ? page.next_pull_before : null }))
      }
    } finally {
      setLoadingMore(false)
    }
  }

  const q = query.trim().toLowerCase()
  const shownPasses = useMemo(
    () => (passes ?? []).filter((p) => !q || `${windowLabel(p.window_start)} ${passState(p, now).label}`.toLowerCase().includes(q)),
    [passes, q, now],
  )
  const shownPulls = useMemo(
    () => (pulls ?? []).filter((p) => !q || `${sources.get(p.source_id) ?? p.source_id} ${p.recipe_id} ${pullState(p).label}`.toLowerCase().includes(q)),
    [pulls, q, sources],
  )
  const hasMore = tab === "passes" ? Boolean(cursor.pass) && (passes?.length ?? 0) >= 50 : Boolean(cursor.pull) && (pulls?.length ?? 0) >= 50

  return (
    <>
      <PageHeader
        title="Runs"
        actions={
          <IconButton label="Refresh" onClick={load}>
            <RefreshCw size={14} aria-hidden />
          </IconButton>
        }
      />
      <Page width="dashboard">
        <PageIntro title="Runs" description="Every 4-hour pass and every pull, newest first" />
        <div className="runs-toolbar">
          <Segmented
            aria-label="Show"
            value={tab}
            onValueChange={(v) => v && setTab(v as "passes" | "pulls")}
            items={[
              { value: "passes", label: <TabLabel count={passes?.length}>Passes</TabLabel> },
              { value: "pulls", label: <TabLabel count={pulls?.length}>Pulls</TabLabel> },
            ]}
          />
          <SearchInput
            className="runs-search"
            placeholder={tab === "passes" ? "Filter by day or state" : "Filter by connection or state"}
            value={query}
            onChange={(e) => setQuery(e.currentTarget.value)}
            aria-label="Filter runs"
          />
        </div>

        {error && !passes ? (
          <Empty title="Runs didn't load" description={error}>
            <Button onClick={load}>Try again</Button>
          </Empty>
        ) : tab === "passes" ? (
          <Table
            loading={passes === null}
            columns={["Window", "State", "Attempts", "Next try", "Written"]}
            empty={q ? "No pass matches" : "No passes yet"}
            emptyHint={q ? undefined : "The first pass runs once 4 hours of entries are in"}
            rows={shownPasses.map((p) => {
              const state = passState(p, now)
              return {
                key: `pass-${p.id}`,
                cells: [
                  <span className="cell-strong">{windowLabel(p.window_start)}</span>,
                  <State {...state} />,
                  <span className="cell-num">{p.attempts}</span>,
                  <span className="cell-muted">{p.state === "ready" && p.available_at ? ago(p.available_at, now) : "—"}</span>,
                  p.revision_id ? <span className="cell-mono" title={p.revision_id}>{p.revision_id.slice(0, 10)}</span> : <span className="cell-muted">Not yet</span>,
                ],
              }
            })}
          />
        ) : (
          <Table
            loading={pulls === null}
            columns={["When", "Connection", "State", "New", "Duplicates", "Rejected", "Took"]}
            empty={q ? "No pull matches" : "No pulls yet"}
            emptyHint={q ? undefined : "Pulls show up here once a connection is set up. History starts with this version"}
            rows={shownPulls.map((p) => ({
              key: `pull-${p.id}`,
              cells: [
                <span className="cell-muted" title={at(p.started_at).toLocaleString()}>{ago(p.started_at, now)}</span>,
                <span className="cell-strong">{sources.get(p.source_id) ?? p.source_id}</span>,
                <State {...pullState(p)} />,
                <span className="cell-num">{p.accepted ?? "—"}</span>,
                <span className="cell-num cell-muted">{p.duplicates ?? "—"}</span>,
                <span className="cell-num" data-alert={p.rejected ? "" : undefined}>{p.rejected ?? "—"}</span>,
                <span className="cell-muted">{p.finished_at ? duration(p.started_at, p.finished_at) : "—"}</span>,
              ],
            }))}
          />
        )}
        {hasMore && !q && (
          <div className="runs-more">
            <Button onClick={more} disabled={loadingMore}>
              {loadingMore ? "Loading…" : "Load older"}
            </Button>
          </div>
        )}
      </Page>
    </>
  )
}

function TabLabel({ count, children }: { count?: number; children: ReactNode }) {
  return (
    <span className="tab-label">
      {children}
      {count !== undefined && <span className="tab-count">{count >= 50 ? "50+" : count}</span>}
    </span>
  )
}

type TableProps = {
  columns: string[]
  rows: { key: string; cells: ReactNode[] }[]
  loading: boolean
  empty: string
  emptyHint?: string
}

function Table({ columns, rows, loading, empty, emptyHint }: TableProps) {
  return (
    <div className="runs-table-wrap">
      <table className="runs-table" style={{ ["--columns" as string]: columns.length }}>
        <thead>
          <tr>
            {columns.map((c) => (
              <th key={c} scope="col">
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {loading
            ? [0, 1, 2, 3].map((i) => (
                <tr key={i}>
                  {columns.map((c) => (
                    <td key={c}>
                      <Skeleton width={c === columns[0] ? 140 : 56} />
                    </td>
                  ))}
                </tr>
              ))
            : rows.map((row, index) => (
                <tr key={row.key} style={{ ["--i" as string]: Math.min(index, 12) }}>
                  {row.cells.map((cell, i) => (
                    <td key={i}>{cell}</td>
                  ))}
                </tr>
              ))}
        </tbody>
      </table>
      {!loading && !rows.length && <Empty title={empty} description={emptyHint} />}
    </div>
  )
}

export { RunsPage }
