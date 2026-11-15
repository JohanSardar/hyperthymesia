import { ArrowRight, CalendarClock, History, KeyRound, LogIn, RotateCw } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import { Link, NavLink, useLocation } from "react-router"
import { ConnectionSetups } from "../agent/connection-setups"
import { ago, at, clock, dayWord, money, runTitle, windowHours } from "../agent/format"
import { describeProblem, type Fix } from "../agent/problems"
import { SigninSheet } from "../agent/signin"
import { TestPanel } from "../agent/test"
import { effortLabel, ModelSheet } from "../model/model-sheet"
import { provider, providers, ProviderTile } from "../model/providers"
import { usePolled } from "../server/connections"
import { api, type AgentRun, type LastResult, type ProviderId, type ProviderStatus } from "../server/api"
import { useSettings } from "../server/settings"
import { PageHeader, PageSidebar, SidebarEmpty, SidebarGroup, useRegisterCommands } from "../shell"
import { Badge, Button, Card, Input, Page, PageIntro, Row, Section, Skeleton, StatusDot, Switch, toast } from "../ui"
import "./agent.css"

/** Whether a provider account works, from its latest Test or run. */
type Health = { tone: "idle" | "ok" | "warning" | "error"; text: string; login?: boolean }

function health(p: ProviderStatus | undefined): Health {
  if (!p) return { tone: "idle", text: "Checking…" }
  const keyed = p.method === "key"
  if (!p.signed_in) return { tone: "idle", text: keyed ? "No key saved" : "Not signed in" }
  const latest = [p.last_check, p.last_run].filter((r): r is NonNullable<LastResult> => Boolean(r)).sort((a, b) => b.at - a.at)[0]
  const base = keyed ? "Key saved" : "Signed in"
  if (!latest) return { tone: "ok", text: `${base} · not tested yet` }
  if (latest.state === "done") return { tone: "ok", text: `${base} · worked ${ago(latest.at)}` }
  const problem = describeProblem(latest.problem, p.id)
  const login = latest.problem === "signin" || latest.problem === "key"
  return { tone: login ? "error" : "warning", text: login ? `${problem?.title} ${ago(latest.at)}` : `${base} · last attempt failed ${ago(latest.at)}`, login }
}

function AgentPage() {
  const { data, save, reload } = useSettings()
  const model = data?.settings.model ?? null
  const accounts = usePolled(api.providers, 5000)
  const runs = usePolled(() => api.agentRuns(), 15000)
  const [signin, setSignin] = useState<ProviderId | null>(null)
  const [sheet, setSheet] = useState<{ open: boolean; provider: ProviderId | null }>({ open: false, provider: null })
  const location = useLocation()

  useEffect(() => {
    if (location.hash && data) document.getElementById(location.hash.slice(1))?.scrollIntoView({ behavior: "smooth", block: "start" })
  }, [location.hash, data])

  const fix = (what: Fix, id: ProviderId | null = model?.provider ?? null) => {
    if (what === "signin" && id) setSignin(id)
    else if (what === "key") setSheet({ open: true, provider: "vercel" })
    else if (what === "model") setSheet({ open: true, provider: model?.provider ?? null })
  }

  useRegisterCommands([
    { id: "agent.model", label: "Change the model", group: "Agent", run: () => setSheet({ open: true, provider: model?.provider ?? null }) },
    ...(model?.provider === "codex" || model?.provider === "grok"
      ? [{ id: "agent.signin", label: `Sign in to ${provider(model.provider).name} again`, group: "Agent", icon: LogIn, run: () => setSignin(model.provider) }]
      : []),
  ])

  const list = accounts.data?.providers ?? []
  const recent = (runs.data?.runs ?? []).slice(0, 6)
  return (
    <>
      <PageHeader title="Agent" />
      <PageSidebar>
        <SidebarGroup label="Accounts">
          {providers.map((p) => {
            const h = health(list.find((s) => s.id === p.id))
            return (
              <a key={p.id} href={`#account-${p.id}`} className="sidebar-row" onClick={(e) => { e.preventDefault(); document.getElementById(`account-${p.id}`)?.scrollIntoView({ behavior: "smooth", block: "center" }) }}>
                <StatusDot tone={h.tone} />
                <span className="sidebar-row-label">{p.name}</span>
                {model?.provider === p.id && <span className="sidebar-row-meta">in use</span>}
              </a>
            )
          })}
        </SidebarGroup>
        <SidebarGroup label="Recent runs" actions={<Link to="/runs" className="sidebar-group-link">All</Link>}>
          {!runs.data ? (
            [0, 1, 2].map((i) => (
              <div key={i} className="sidebar-row">
                <Skeleton width="70%" />
              </div>
            ))
          ) : recent.length === 0 ? (
            <SidebarEmpty>No runs yet</SidebarEmpty>
          ) : (
            recent.map((run) => <RunLink key={run.id} run={run} />)
          )}
        </SidebarGroup>
      </PageSidebar>
      <Page width="settings">
        <PageIntro title="Agent" description="Your AI reads what was recorded and writes your day pages. It runs on your own provider login, through fx." />

        <Section title="Provider accounts" description="Each provider keeps its own login. The one in use writes your day pages">
          <div className="account-list">
            {providers.map((p) => (
              <AccountCard
                key={p.id}
                status={list.find((s) => s.id === p.id)}
                inUse={model?.provider === p.id}
                onSignin={() => setSignin(p.id)}
                onKey={() => setSheet({ open: true, provider: "vercel" })}
                onUse={() => setSheet({ open: true, provider: p.id })}
              />
            ))}
          </div>
        </Section>

        <Section title="Default model" description="Used for every pass and connection setup. Changes apply from the next run">
          <Card>
            {model ? (
              <Row
                leading={<ProviderTile id={model.provider} />}
                label={model.model}
                description={[provider(model.provider).name, model.effort ? `${effortLabel(model.effort)} effort` : "Model's default effort"].join(" · ")}
                control={<Button onClick={() => setSheet({ open: true, provider: model.provider })}>Change</Button>}
              />
            ) : (
              <Row
                leading={<span className="tile-empty" aria-hidden />}
                label="No model chosen"
                description="Nothing runs until you choose one. Nothing is picked for you"
                control={
                  <Button variant="primary" onClick={() => setSheet({ open: true, provider: null })}>
                    Choose a model
                  </Button>
                }
              />
            )}
            {model && <TestPanel model={model} onResult={() => void accounts.refresh()} onFix={(what) => fix(what)} />}
          </Card>
        </Section>

        <Schedule upcoming={runs.data?.upcoming ?? []} loaded={Boolean(runs.data)} />

        <SpendingLimit
          spent={runs.data?.month_spend_usd}
          limit={data?.settings.spend_limit_usd ?? null}
          onSave={async (limit) => {
            await save({ spend_limit_usd: limit })
            void runs.refresh()
          }}
        />

        <ConnectionSetups />
      </Page>
      <SigninSheet
        id={signin}
        open={signin !== null}
        onOpenChange={(open) => !open && setSignin(null)}
        onDone={() => {
          toast.success("Signed in", { description: "Press Test to check it with your model" })
          void accounts.refresh()
        }}
      />
      <ModelSheet
        open={sheet.open}
        initialProvider={sheet.provider}
        onOpenChange={(open) => setSheet((s) => ({ ...s, open }))}
        onSaved={() => {
          void reload()
          void accounts.refresh()
        }}
      />
    </>
  )
}

function RunLink({ run }: { run: AgentRun }) {
  return (
    <NavLink to={`/runs/${run.id}`} className="sidebar-row">
      <StatusDot tone={run.state === "running" ? "live" : run.state === "done" || run.state === "skipped" ? "ok" : run.state === "failed" ? "error" : "warning"} />
      <span className="sidebar-row-label">{run.kind === "pass" && run.window_start ? windowHours(run.window_start) : runTitle(run)}</span>
      <span className="sidebar-row-meta">{run.state === "running" ? "running" : dayWord(at(run.kind === "pass" && run.window_start ? run.window_start : run.started_at)).replace("Yesterday", "yest.")}</span>
    </NavLink>
  )
}

function AccountCard({ status, inUse, onSignin, onKey, onUse }: { status: ProviderStatus | undefined; inUse: boolean; onSignin: () => void; onKey: () => void; onUse: () => void }) {
  if (!status) {
    return (
      <Card className="account-card">
        <Skeleton width={36} height={36} />
        <div className="row-text">
          <Skeleton width={120} />
          <Skeleton width={220} />
        </div>
      </Card>
    )
  }
  const p = provider(status.id)
  const h = health(status)
  const keyed = status.method === "key"
  return (
    <Card className="account-card" data-in-use={inUse ? "" : undefined}>
      <span id={`account-${p.id}`} className="account-anchor" />
      <ProviderTile id={p.id} />
      <div className="row-text">
        <span className="account-name">
          <span className="row-label">{p.name}</span>
          {inUse && <Badge tone="primary">In use</Badge>}
        </span>
        <span className="account-status">
          <StatusDot tone={h.tone} /> {h.text}
        </span>
        <span className="row-description">{keyed ? "One key for models from almost every lab. Billed per request by Vercel." : `Your ${p.account} plan. Included in it; no charge per run.`}</span>
      </div>
      <div className="row-control">
        {keyed ? (
          <Button variant={status.signed_in ? "ghost" : "default"} onClick={onKey}>
            <KeyRound size={13} aria-hidden /> {status.signed_in ? "Replace key" : "Add key"}
          </Button>
        ) : (
          <Button variant={h.login ? "primary" : status.signed_in ? "ghost" : "default"} onClick={onSignin}>
            {h.login ? <RotateCw size={13} aria-hidden /> : <LogIn size={13} aria-hidden />} {status.signed_in ? "Sign in again" : "Sign in"}
          </Button>
        )}
        {!inUse && status.signed_in && (
          <Button variant="ghost" onClick={onUse}>
            Use
          </Button>
        )}
      </div>
    </Card>
  )
}

function Schedule({ upcoming, loaded }: { upcoming: { available_at: number; catch_up: boolean; held: boolean; window_start: number }[]; loaded: boolean }) {
  const next = upcoming.find((u) => !u.catch_up && !u.held)
  const nextCatchUp = upcoming.find((u) => u.catch_up && !u.held)
  const held = upcoming.some((u) => u.held)
  const when = (us: number) => {
    const d = at(us)
    return d.getTime() < Date.now() ? "now" : `${dayWord(d).toLowerCase() === "today" ? "today" : dayWord(d)} at ${clock(d)}`
  }
  return (
    <Section title="Schedule" description="It runs by itself. You never need to start a run" actions={<Button variant="ghost" size="xs" render={<Link to="/runs" />}><History size={12} aria-hidden /> Runs <ArrowRight size={12} aria-hidden /></Button>}>
      <Card>
        <Row
          leading={<CalendarClock size={16} strokeWidth={1.75} className="row-icon" aria-hidden />}
          label="Every 4 hours"
          description="When a 4-hour window ends and its last minutes are in, the AI reads what was recorded in it and writes that part of your day page."
          control={<span className="schedule-next">{!loaded ? <Skeleton width={90} /> : next ? `Next ${when(next.available_at)}` : "Waiting for recording"}</span>}
        />
        <Row
          leading={<RotateCw size={16} strokeWidth={1.75} className="row-icon" aria-hidden />}
          label="Next-day catch-up"
          description="A day later it reads each window again with anything that arrived late, such as synced calendar events, and updates the page only if something changed."
          control={<span className="schedule-next">{!loaded ? <Skeleton width={90} /> : nextCatchUp ? `Next ${when(nextCatchUp.available_at)}` : "None due"}</span>}
        />
        {held && <Row label="Held by the spending limit" description="Some runs wait for next month, or for a higher limit below." />}
      </Card>
    </Section>
  )
}

function SpendingLimit({ spent, limit, onSave }: { spent: number | undefined; limit: number | null; onSave: (limit: number | null) => Promise<void> }) {
  const [on, setOn] = useState(limit !== null)
  const [value, setValue] = useState(limit !== null ? String(limit) : "")
  useEffect(() => {
    setOn(limit !== null)
    setValue(limit !== null ? String(limit) : "")
  }, [limit])
  const parsed = Number(value)
  const valid = value.trim() !== "" && Number.isFinite(parsed) && parsed >= 0 && parsed <= 100000
  const changed = valid && parsed !== limit
  const used = useMemo(() => (limit && spent !== undefined ? Math.min(1, spent / limit) : null), [limit, spent])

  const apply = async (next: number | null) => {
    try {
      await onSave(next)
      toast.success(next === null ? "No spending limit" : `Limit set to ${money(next)} a month`)
    } catch (cause) {
      toast.error("Couldn't save the limit", { description: cause instanceof Error ? cause.message : undefined })
    }
  }

  return (
    <Section title="Spending limit" description="Optional. There's no limit unless you set one">
      <Card className="limit-card">
        <span id="limit" className="account-anchor" />
        <Row
          label="Monthly limit"
          description="When this month's spending reaches it, no new runs start until next month or until you raise it. A run already going finishes."
          control={
            <Switch
              checked={on}
              aria-label={on ? "Remove the spending limit" : "Set a spending limit"}
              onCheckedChange={(checked) => {
                setOn(checked)
                if (!checked && limit !== null) void apply(null)
              }}
            />
          }
        />
        {on && (
          <div className="limit-edit">
            <span className="limit-input">
              <span className="limit-currency" aria-hidden>
                $
              </span>
              <Input inputMode="decimal" placeholder="10" value={value} onChange={(e) => setValue(e.currentTarget.value)} aria-label="Monthly limit in US dollars" />
              <span className="row-description">a month</span>
            </span>
            <Button variant="primary" disabled={!changed} onClick={() => void apply(parsed)}>
              Save
            </Button>
          </div>
        )}
        <Row
          label="This month"
          description="Codex and Grok runs are part of your plan and count as $0. Vercel AI Gateway reports each run's cost."
          control={
            <span className="limit-spent">
              {spent === undefined ? <Skeleton width={60} /> : money(spent)}
              {limit !== null && spent !== undefined && <span className="row-description"> of {money(limit)}</span>}
            </span>
          }
        />
        {used !== null && (
          <div className="limit-meter" aria-hidden>
            <span style={{ width: `${used * 100}%` }} data-full={used >= 1 ? "" : undefined} />
          </div>
        )}
      </Card>
    </Section>
  )
}

export { AgentPage }
