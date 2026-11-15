import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { api, type Pull, type Source } from "../server/api"
import { Badge, Button, Empty, Skeleton, StatusDot } from "../ui"
import { ago as agoText, duration, seconds } from "../agent/format"
import type { ReactNode } from "react"

type Tone = "neutral" | "primary" | "success" | "warning" | "danger"

const at = (us: number) => new Date(us / 1000)

function State({ label, tone, live }: { label: string; tone: Tone; live?: boolean }) {
  return (
    <Badge tone={tone}>
      {live && <StatusDot tone="live" />}
      {label}
    </Badge>
  )
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

/** Every pull, newest first. Pulls fetch from connections on a schedule, without the AI. */
function PullsTable() {
  const [pulls, setPulls] = useState<Pull[] | null>(null)
  const [cursor, setCursor] = useState<number | null>(null)
  const [sources, setSources] = useState<Map<string, string>>(new Map())
  const [error, setError] = useState<string | null>(null)
  const [loadingMore, setLoadingMore] = useState(false)
  const paged = useRef(false)

  const load = useCallback(async () => {
    try {
      const page = await api.runs()
      setPulls((current) => {
        const oldest = page.pulls.at(-1)?.id
        return oldest === undefined ? page.pulls : [...page.pulls, ...(current ?? []).filter((row) => row.id < oldest)]
      })
      if (!paged.current) setCursor(page.next_pull_before)
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Pulls didn't load")
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
    if (!cursor) return
    setLoadingMore(true)
    try {
      paged.current = true
      const page = await api.runs({ pull_before: cursor })
      setPulls((current) => [...(current ?? []), ...page.pulls])
      setCursor(page.pulls.length ? page.next_pull_before : null)
    } finally {
      setLoadingMore(false)
    }
  }

  const rows = useMemo(
    () =>
      (pulls ?? []).map((p) => ({
        key: `pull-${p.id}`,
        cells: [
          <span className="cell-muted" title={at(p.started_at).toLocaleString()}>{agoText(p.started_at)}</span>,
          <span className="cell-strong">{sources.get(p.source_id) ?? p.source_id}</span>,
          <State {...pullState(p)} />,
          <span className="cell-num">{p.accepted ?? "—"}</span>,
          <span className="cell-num cell-muted">{p.duplicates ?? "—"}</span>,
          <span className="cell-num" data-alert={p.rejected ? "" : undefined}>{p.rejected ?? "—"}</span>,
          <span className="cell-muted">{p.finished_at ? duration(seconds(p.started_at, p.finished_at)) : "—"}</span>,
        ],
      })),
    [pulls, sources],
  )

  if (error && !pulls)
    return (
      <Empty title="Pulls didn't load" description={error}>
        <Button onClick={load}>Try again</Button>
      </Empty>
    )
  return (
    <>
      <Table
        loading={pulls === null}
        columns={["When", "Connection", "State", "New", "Duplicates", "Rejected", "Took"]}
        empty="No pulls yet"
        emptyHint="Pulls show up here once a connection is set up"
        rows={rows}
      />
      {cursor && (pulls?.length ?? 0) >= 50 && (
        <div className="runs-more">
          <Button onClick={more} disabled={loadingMore}>
            {loadingMore ? "Loading…" : "Load older"}
          </Button>
        </div>
      )}
    </>
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

export { PullsTable }
