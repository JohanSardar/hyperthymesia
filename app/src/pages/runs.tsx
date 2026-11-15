import { ArrowUpRight, CalendarClock, FileText, History, RefreshCw, RotateCw, TriangleAlert } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import { Link, NavLink, useNavigate, useParams, useSearchParams } from "react-router"
import { at, clock, costLabel, dayKey, dayWord, duration, runTitle, seconds, tokens, windowHours, windowLabel } from "../agent/format"
import { describeProblem, type Fix } from "../agent/problems"
import { SigninSheet } from "../agent/signin"
import { effortLabel, ModelSheet } from "../model/model-sheet"
import { provider } from "../model/providers"
import { api as connectionsApi, usePolled } from "../server/connections"
import { api, type AgentRun, type ProviderId, type RunState, type Upcoming } from "../server/api"
import { PageHeader, PageSidebar, SidebarEmpty, SidebarGroup, useRegisterCommands } from "../shell"
import { Badge, Button, Callout, Empty, IconButton, Page, Segmented, Skeleton, Spinner, Stat, StatusDot, toast } from "../ui"
import { PullsTable } from "./pulls"
import "./runs.css"

type Tone = "neutral" | "primary" | "success" | "warning" | "danger"

const stateWords: Record<RunState, { label: string; tone: Tone; dot: "live" | "ok" | "warning" | "error" | "idle" }> = {
  running: { label: "Running", tone: "primary", dot: "live" },
  done: { label: "Done", tone: "success", dot: "ok" },
  skipped: { label: "Nothing new", tone: "neutral", dot: "ok" },
  failed: { label: "Failed", tone: "danger", dot: "error" },
  waiting: { label: "Waiting", tone: "warning", dot: "warning" },
  stopped: { label: "Started over", tone: "neutral", dot: "idle" },
}

/** Re-renders every second while something on screen is live. */
function useTick(live: boolean) {
  const [, tick] = useState(0)
  useEffect(() => {
    if (!live) return
    const timer = window.setInterval(() => tick((n) => n + 1), 1000)
    return () => window.clearInterval(timer)
  }, [live])
}

function RunsPage() {
  const { id } = useParams()
  const [params, setParams] = useSearchParams()
  const view = params.get("view") === "pulls" ? "pulls" : "runs"
  const navigate = useNavigate()
  const [live, setLive] = useState(false)
  const runs = usePolled(() => api.agentRuns(), live ? 3000 : 15000)
  const list = runs.data?.runs ?? []
  useEffect(() => setLive(list.some((r) => r.state === "running")), [list])
  const selectedId = id ? Number(id) : list[0]?.id

  useRegisterCommands([{ id: "runs.refresh", label: "Refresh runs", group: "Runs", icon: RefreshCw, run: () => void runs.refresh() }])

  return (
    <>
      <PageHeader
        title="Runs"
        actions={
          <>
            <Segmented
              aria-label="Show"
              value={view}
              onValueChange={(v) => setParams(v === "pulls" ? { view: "pulls" } : {})}
              items={[
                { value: "runs", label: "AI runs" },
                { value: "pulls", label: "Pulls" },
              ]}
            />
            <IconButton label="Refresh" onClick={() => void runs.refresh()}>
              <RefreshCw size={14} aria-hidden />
            </IconButton>
          </>
        }
      />
      <RunList runs={list} upcoming={runs.data?.upcoming ?? []} loading={!runs.data && !runs.error} selected={view === "runs" ? selectedId : undefined} />
      {view === "pulls" ? (
        <Page width="dashboard">
          <div className="runs-intro">
            <h2 className="h1">Pulls</h2>
            <p className="page-intro-description">Each connection's scheduled fetch, newest first. Pulls run without the AI and cost nothing.</p>
          </div>
          <PullsTable />
        </Page>
      ) : runs.error && !runs.data ? (
        <Page width="detail">
          <Empty fill title="Runs didn't load" description={runs.error.message}>
            <Button onClick={() => void runs.refresh()}>Try again</Button>
          </Empty>
        </Page>
      ) : runs.data && list.length === 0 ? (
        <Page width="detail">
          <Empty
            fill
            icon={<History size={20} aria-hidden />}
            title="No runs yet"
            description="The first runs once a 4-hour window of recording is in. You'll see it here live, then what it cost and what it wrote."
          >
            <Button render={<Link to="/agent" />}>See the schedule</Button>
          </Empty>
        </Page>
      ) : selectedId ? (
        <RunDetail key={selectedId} id={selectedId} onChanged={() => void runs.refresh()} onOpen={(next) => navigate(`/runs/${next}`)} />
      ) : (
        <Page width="detail">
          <Skeleton width="40%" height={24} />
        </Page>
      )}
    </>
  )
}

function RunList({ runs, upcoming, loading, selected }: { runs: AgentRun[]; upcoming: Upcoming[]; loading: boolean; selected?: number }) {
  useTick(runs.some((r) => r.state === "running"))
  const groups = useMemo(() => {
    // A pass belongs to the day of its window: the 6–10 PM pass often starts after midnight.
    const out = new Map<string, AgentRun[]>()
    for (const [i, run] of runs.entries()) {
      // Retries of one job that keep failing fold into the newest attempt.
      const newer = runs[i - 1]
      if (newer && run.state === "failed" && newer.state === "failed" && run.job_id !== null && run.job_id === newer.job_id) continue
      const day = dayWord(at(run.kind === "pass" && run.window_start ? run.window_start : run.started_at))
      out.set(day, [...(out.get(day) ?? []), run])
    }
    return [...out].map(([day, list]) => ({ day, runs: list }))
  }, [runs])
  const next = upcoming.filter((u) => !u.held).slice(0, 3)
  return (
    <PageSidebar>
      {next.length > 0 && (
        <SidebarGroup label="Coming up">
          {next.map((u) => (
            <div key={u.job_id} className="sidebar-row run-row" data-upcoming="">
              <CalendarClock size={13} aria-hidden />
              <span className="sidebar-row-label">{u.rerun ? "Re-run" : u.catch_up ? "Catch-up" : "Pass"}, {windowHours(u.window_start)}</span>
              <span className="sidebar-row-meta">{at(u.available_at).getTime() <= Date.now() ? "soon" : dueLabel(u.available_at)}</span>
            </div>
          ))}
        </SidebarGroup>
      )}
      {loading ? (
        <SidebarGroup label="Runs">
          {[0, 1, 2, 3, 4].map((i) => (
            <div key={i} className="sidebar-row">
              <Skeleton width={`${55 + ((i * 11) % 30)}%`} />
            </div>
          ))}
        </SidebarGroup>
      ) : runs.length === 0 ? (
        <SidebarGroup label="Runs">
          <SidebarEmpty>No runs yet</SidebarEmpty>
        </SidebarGroup>
      ) : (
        groups.map((group) => (
          <SidebarGroup key={group.day} label={group.day}>
            {group.runs.map((run) => {
              const words = stateWords[run.state]
              return (
                <NavLink key={run.id} to={`/runs/${run.id}`} className="sidebar-row run-row" aria-current={run.id === selected ? "page" : undefined}>
                  <StatusDot tone={words.dot} />
                  <span className="sidebar-row-label">{run.kind === "pass" && run.window_start ? windowHours(run.window_start) : runTitle(run)}</span>
                  <span className="sidebar-row-meta">
                    {run.state === "running"
                      ? duration(seconds(run.started_at, Date.now() * 1000))
                      : run.state === "failed" && tries(runs, run) > 1
                        ? `failed ×${tries(runs, run)}`
                        : run.state === "failed" || run.state === "waiting"
                          ? words.label.toLowerCase()
                          : run.finished_at
                            ? duration(seconds(run.started_at, run.finished_at))
                            : ""}
                  </span>
                </NavLink>
              )
            })}
          </SidebarGroup>
        ))
      )}
    </PageSidebar>
  )
}

function RunDetail({ id, onChanged, onOpen }: { id: number; onChanged: () => void; onOpen: (id: number) => void }) {
  const [fast, setFast] = useState(true)
  const detail = usePolled(() => api.agentRun(id), fast ? 1500 : 0)
  const run = detail.data
  useEffect(() => {
    if (run) setFast(run.state === "running")
  }, [run])
  useTick(run?.state === "running")
  const [signin, setSignin] = useState<ProviderId | null>(null)
  const [sheet, setSheet] = useState<{ open: boolean; provider: ProviderId | null }>({ open: false, provider: null })
  const [busy, setBusy] = useState(false)
  const navigate = useNavigate()

  const rerun = async (quiet = false) => {
    if (!run) return
    setBusy(true)
    try {
      if (run.kind === "setup" && run.job?.source_id) {
        await connectionsApi.retrySetup(run.job.source_id)
        if (!quiet) toast.success("Setup started again")
      } else {
        await api.rerun(run.id)
        if (!quiet) toast.success("Queued", { description: "It starts within a few seconds and shows up at the top" })
      }
      onChanged()
      // Follow the new run as soon as it starts.
      const before = run.id
      for (let i = 0; i < 20; i++) {
        await new Promise((resolve) => window.setTimeout(resolve, 1500))
        const latest = (await api.agentRuns()).runs.find((r) => r.id > before && r.kind === run.kind && r.window_start === run.window_start)
        if (latest) {
          onChanged()
          onOpen(latest.id)
          break
        }
      }
    } catch (cause) {
      toast.error("Couldn't run it again", { description: cause instanceof Error ? cause.message : undefined })
    } finally {
      setBusy(false)
    }
  }

  const fix = (what: Fix) => {
    if (!run) return
    if (what === "signin" && run.provider) setSignin(run.provider)
    else if (what === "key") setSheet({ open: true, provider: "vercel" })
    else if (what === "model") setSheet({ open: true, provider: run.provider })
    else if (what === "limit") navigate("/agent#limit")
    else if (what === "rerun") void rerun()
  }

  useRegisterCommands(
    run && run.state !== "running" && ((run.kind === "pass" && run.window_start !== null) || (run.kind === "setup" && run.job?.source_id))
      ? [{ id: "runs.rerun", label: "Re-run this", group: "Runs", icon: RotateCw, run: () => void rerun() }]
      : [],
  )

  if (!run) {
    return (
      <Page width="detail">
        {detail.error ? (
          <Empty fill title="This run didn't load" description={detail.error.message} />
        ) : (
          <div className="run-head">
            <Skeleton width="45%" height={28} />
            <Skeleton width="30%" />
          </div>
        )}
      </Page>
    )
  }

  const words = stateWords[run.state]
  const problem = describeProblem(run.problem, run.provider, run.kind)
  const took = seconds(run.started_at, run.finished_at ?? Date.now() * 1000)
  const canRerun = (run.kind === "pass" && run.window_start !== null) || (run.kind === "setup" && Boolean(run.job?.source_id))
  const day = run.window_start ? dayKey(run.window_start) : null
  const wrote = run.kind === "pass" && run.state === "done" && day
  const retryAt = run.state === "waiting" && run.job?.state === "ready" ? run.job.available_at : null

  return (
    <Page width="detail" className="run-page">
      <div className="run-head">
        <span className="run-kind">{run.kind === "pass" ? "4-hour pass" : run.kind === "setup" ? "Connection setup" : "Test"}</span>
        <div className="run-title-row">
          <h2 className="h1">{runTitle(run)}</h2>
          <Badge tone={words.tone}>
            {run.state === "running" && <StatusDot tone="live" />}
            {words.label}
          </Badge>
          <span className="run-actions">
            {wrote && (
              <Button render={<Link to={`/today/${day}`} />}>
                <FileText size={13} aria-hidden /> Open what it wrote
              </Button>
            )}
            {canRerun && run.state !== "running" && (
              <Button variant={run.state === "failed" && problem?.fix === "rerun" ? "primary" : "default"} disabled={busy} onClick={() => void rerun()}>
                {busy ? <Spinner size={12} /> : <RotateCw size={13} aria-hidden />} {run.kind === "setup" ? "Retry setup" : "Re-run"}
              </Button>
            )}
          </span>
        </div>
        <p className="run-sub">
          Started {dayWord(at(run.started_at)).toLowerCase() === "today" ? "today" : dayWord(at(run.started_at))} at {clock(at(run.started_at))}
          {run.model && (
            <>
              {" "}
              · {run.provider ? provider(run.provider).name : ""} {run.model}
              {run.effort ? `, ${effortLabel(run.effort).toLowerCase()} effort` : ""}
            </>
          )}
        </p>
      </div>

      {problem && run.state !== "done" && (
        <Callout
          tone={run.state === "failed" ? "danger" : "neutral"}
          icon={<TriangleAlert size={14} />}
          title={problem.title}
          actions={
            problem.fix &&
            !(problem.fix === "rerun" && run.state === "failed") && (
              <Button variant="primary" size="sm" disabled={busy} onClick={() => fix(problem.fix)}>
                {problem.fixLabel}
              </Button>
            )
          }
        >
          {problem.body}
          {retryAt && ` It tries again at ${clock(at(retryAt))}.`}
        </Callout>
      )}

      <div className="run-stats">
        <Stat label="Status" value={words.label} hint={run.state === "running" ? `${run.steps} steps so far` : run.finished_at ? `Finished at ${clock(at(run.finished_at))}` : undefined} />
        <Stat label="Duration" value={duration(took)} hint={run.state === "running" ? "Still going" : undefined} />
        <Stat
          label="Cost"
          value={costLabel(run)}
          hint={run.input_tokens !== null ? `${tokens(run.input_tokens)} in · ${tokens(run.output_tokens)} out` : run.provider === "vercel" ? "Reported by Vercel" : undefined}
        />
        <Stat
          label="Wrote"
          value={
            run.state === "running"
              ? "Not yet"
              : run.kind === "setup"
                ? run.state === "done" ? "Pull plan" : "Nothing"
                : wrote ? "Day page" : run.state === "skipped" ? "Nothing new" : "Nothing"
          }
          hint={
            wrote ? (
              <Link to={`/today/${day}`} className="run-link">{run.window_start ? windowLabel(run.window_start) : ""} <ArrowUpRight size={11} aria-hidden /></Link>
            ) : run.kind === "setup" && run.state === "done" ? (
              <Link to="/connections" className="run-link">{run.label ?? "Connections"} <ArrowUpRight size={11} aria-hidden /></Link>
            ) : undefined
          }
        />
      </div>

      <section className="run-log" aria-label="Log">
        <div className="run-log-head">
          <span className="h2">Log</span>
          {run.state === "running" && (
            <span className="run-live">
              <StatusDot tone="live" /> Live
            </span>
          )}
        </div>
        <ol className="run-log-list">
          {run.log.map((line, i) => (
            <li key={i} style={{ ["--i" as string]: Math.min(i, 12) }}>
              <span className="run-log-time">+{duration(seconds(run.started_at, line.at))}</span>
              <span>{line.text}</span>
            </li>
          ))}
          {run.state === "running" && (
            <li className="run-log-working">
              <span className="run-log-time">
                <Spinner size={11} />
              </span>
              <span>Working…</span>
            </li>
          )}
        </ol>
        <p className="run-log-note">The log names each step and the kind of record it read. It never shows your screen text or the AI's words; the day page holds what it wrote.</p>
      </section>

      <SigninSheet
        id={signin}
        open={signin !== null}
        onOpenChange={(open) => !open && setSignin(null)}
        onDone={() => {
          toast.success("Signed in", { description: canRerun ? "Running it again now" : undefined })
          if (canRerun) void rerun(true)
        }}
      />
      <ModelSheet
        open={sheet.open}
        initialProvider={sheet.provider}
        onOpenChange={(open) => setSheet((s) => ({ ...s, open }))}
        onSaved={() => {
          if (canRerun) void rerun()
        }}
      />
    </Page>
  )
}

export { RunsPage }

/** "08:10 AM" today, "Mon 08:00 AM" on another day. */
function dueLabel(us: number) {
  const d = at(us)
  return dayWord(d) === "Today" ? clock(d) : `${d.toLocaleDateString(undefined, { weekday: "short" })} ${clock(d)}`
}

/** How many failed attempts in a row the run's job made, counting this one. */
function tries(runs: AgentRun[], run: AgentRun) {
  let n = 0
  for (const r of runs.slice(runs.indexOf(run))) {
    if (r.state !== "failed" || r.job_id === null || r.job_id !== run.job_id) break
    n++
  }
  return n
}
