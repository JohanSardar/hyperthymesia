import { AlertTriangle, ArrowRight, Play, RotateCw, Sparkles } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import { Link, useSearchParams } from "react-router"
import { useCatalog } from "../connections/catalog"
import { Logo } from "../connections/logo"
import { StatusLabel } from "../connections/parts"
import { ago, until } from "../lib/time"
import { api, usePolled, type AgentConnection, type Pull } from "../server/connections"
import { Button, Card, Confirm, ConfirmTrigger, buttonAttrs, Empty, IconButton, Section, Skeleton, Switch, toast } from "../ui"
import { useLogoIds } from "../pages/connections"
import { PlainError } from "../connections/errors"
import "../connections/connections.css"
import "../pages/agent.css"

function humanize(name: string) {
  const words = name.replace(/[_-]+/g, " ").trim()
  return words.charAt(0).toUpperCase() + words.slice(1)
}

function describePull(pull: Pull) {
  const r = pull.recipe?.request ?? {}
  if (Array.isArray(r.argv)) return r.argv.join(" ")
  if (typeof r.tool === "string") return `MCP tool ${r.tool}`
  const method = typeof r.method === "string" ? r.method : "GET"
  const path = typeof r.path === "string" ? r.path : typeof r.url === "string" ? r.url : ""
  return path ? `${method} ${path}` : (pull.recipe?.kind ?? "")
}

async function act(run: () => Promise<unknown>, done: string, failed: string, after: () => unknown) {
  try {
    await run()
    toast.success(done)
  } catch (e) {
    toast.error(failed, { description: e instanceof Error ? e.message : undefined })
  } finally {
    await after()
  }
}

function PullRow({ pull, sourceOn, refresh }: { pull: Pull; sourceOn: boolean; refresh: () => unknown }) {
  const on = pull.enabled && !pull.owner_off
  const failing = Boolean(pull.last_error)
  return (
    <div className="pull-row" data-off={on && sourceOn ? undefined : ""}>
      <span className="pull-dot" data-state={!on || !sourceOn ? "off" : failing ? "error" : pull.last_ok_at ? "ok" : "waiting"} aria-hidden />
      <div className="pull-text">
        <span className="row-label">{humanize(pull.recipe?.name ?? pull.name)}</span>
        <span className="mono-line pull-request">{describePull(pull)}</span>
        <span className="pull-meta">
          {failing ? (
            <span className="pull-error">
              <AlertTriangle size={12} aria-hidden /> <PlainError text={pull.last_error!} />
            </span>
          ) : (
            <>
              <span>{pull.last_ok_at ? `Last pulled ${ago(pull.last_ok_at)}` : "Not pulled yet"}</span>
              {on && sourceOn && (
                <>
                  <span className="sep">·</span>
                  <span>Next {until(pull.next_run_at)}</span>
                </>
              )}
            </>
          )}
        </span>
      </div>
      <div className="row-control">
        <IconButton
          label="Pull now"
          disabled={!on || !sourceOn}
          onClick={() => act(() => api.retryPull(pull.id), "Pull queued", "Couldn't start the pull", refresh)}
        >
          <Play size={13} aria-hidden />
        </IconButton>
        <Switch
          checked={on}
          aria-label={on ? "Switch this pull off" : "Switch this pull on"}
          onCheckedChange={(checked) => act(() => api.setPullEnabled(pull.id, checked), checked ? "Pull switched on" : "Pull switched off", "Couldn't change the pull", refresh)}
        />
      </div>
    </div>
  )
}

function AgentCard({ item, logoId, focused, refresh }: { item: AgentConnection; logoId?: string; focused: boolean; refresh: () => unknown }) {
  const { source, setup, pulls, addons } = item
  const pending = source.status === "setting_up" || setup?.status === "pending"
  const failed = setup?.status === "failed" || source.status === "failed" || source.status === "needs_attention"
  const summary = setup?.result?.summary
  const noPull = setup?.result?.no_pull

  return (
    <Card className="agent-card" data-focused={focused ? "" : undefined} data-off={source.enabled ? undefined : ""}>
      <div className="agent-card-head" id={`c-${source.id}`}>
        <Logo id={logoId} name={source.name} size={36} />
        <div className="row-text">
          <Link to={`/connections/${source.id}`} className="row-label agent-name">
            {source.name}
          </Link>
          <span className="yours-meta">
            <StatusLabel source={source} setup={setup} />
            <span className="sep">·</span>
            <span>{source.last_pull_at ? `Pulled ${ago(source.last_pull_at)}` : "Not pulled yet"}</span>
          </span>
        </div>
        <div className="row-control">
          <Switch
            checked={source.enabled}
            aria-label={source.enabled ? `Switch ${source.name} off` : `Switch ${source.name} on`}
            onCheckedChange={(checked) =>
              act(() => api.setEnabled(source.id, checked), checked ? `${source.name} is back on` : `${source.name} is off`, "Couldn't change it", refresh)
            }
          />
        </div>
      </div>

      <div className="agent-block">
        <span className="agent-block-label">
          {failed ? <AlertTriangle size={12} aria-hidden /> : <Sparkles size={12} aria-hidden />} {failed ? "What went wrong" : "What it understood"}
        </span>
        {pending ? (
          <div className="thinking">
            <span className="thinking-text">Working out what to read and testing a pull…</span>
            <div className="thinking-lines" aria-hidden>
              <span />
              <span />
            </div>
          </div>
        ) : failed ? (
          <div className="agent-failed">
            {setup?.last_error ?? source.last_error ? <PlainError text={(setup?.last_error ?? source.last_error)!} /> : <p>Setup didn't finish.</p>}
          </div>
        ) : source.status === "needs_reconnect" ? (
          <p className="agent-summary muted">It couldn't reach {source.name} with the saved sign-in. Reconnect it to let the agent finish.</p>
        ) : summary ? (
          <p className="agent-summary">{summary}</p>
        ) : (
          <p className="agent-summary muted">Nothing to show yet.</p>
        )}
        {noPull && !pending && <p className="agent-nopull">No automatic pull: {noPull}</p>}
      </div>

      {(pulls.length > 0 || addons.length > 0) && (
        <div className="agent-block">
          <span className="agent-block-label">
            {pulls.length} {pulls.length === 1 ? "pull" : "pulls"} scheduled · every {Math.round(item.cadence_seconds / 3600)} hours
          </span>
          <div className="pull-list">
            {pulls.map((pull) => (
              <PullRow key={pull.id} pull={pull} sourceOn={source.enabled} refresh={refresh} />
            ))}
            {addons.map((addon) => (
              <div className="pull-row" key={addon.id} data-off={addon.enabled && !addon.owner_off ? undefined : ""}>
                <span className="pull-dot" data-state={addon.enabled && !addon.owner_off ? "ok" : "off"} aria-hidden />
                <div className="pull-text">
                  <span className="row-label">{addon.name}</span>
                  <span className="pull-meta">{addon.text}</span>
                </div>
                <div className="row-control">
                  <Switch
                    checked={addon.enabled && !addon.owner_off}
                    aria-label="Switch this add-on"
                    onCheckedChange={(checked) => act(() => api.setAddonEnabled(addon.id, checked), checked ? "Add-on on" : "Add-on off", "Couldn't change it", refresh)}
                  />
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="agent-foot">
        <span>
          {setup ? `Setup ${setup.generation > 1 ? `attempt ${setup.generation} ` : ""}${pending ? "started" : "ran"} ${ago(setup.finished_at ?? setup.created_at)}` : "No setup yet"}
        </span>
        {source.status === "needs_reconnect" ? (
          <Button size="xs" variant="ghost" render={<Link to={`/connections/${source.id}`} />}>
            Reconnect <ArrowRight size={12} aria-hidden />
          </Button>
        ) : (
          <Confirm
            title={`Set up ${source.name} again?`}
            description="Starts a new agent run with your model. Pulls you switched off stay off."
            confirmLabel="Retry setup"
            onConfirm={() => act(() => api.retrySetup(source.id), "Setup started again", "Couldn't retry setup", refresh)}
            trigger={
              <ConfirmTrigger {...buttonAttrs(failed ? "default" : "ghost", "xs")} disabled={pending || !source.enabled}>
                <RotateCw size={12} aria-hidden /> Retry setup
              </ConfirmTrigger>
            }
          />
        )}
      </div>
    </Card>
  )
}

/** What the agent set up for each connection: pulls, add-ons and retries. */
function ConnectionSetups() {
  const [params] = useSearchParams()
  const focus = params.get("focus")
  const [fast, setFast] = useState(true)
  const agent = usePolled(api.agent, fast ? 3000 : 15000)
  const { catalog } = useCatalog()
  const logoIds = useLogoIds(catalog)
  const items = agent.data?.connections ?? []

  useEffect(() => {
    if (agent.data) setFast(items.some((c) => c.source.status === "setting_up" || c.setup?.status === "pending"))
  }, [agent.data, items])

  useEffect(() => {
    if (focus && agent.data) document.getElementById(`c-${focus}`)?.scrollIntoView({ behavior: "smooth", block: "center" })
  }, [focus, agent.data])

  const totals = useMemo(() => {
    const pulls = items.flatMap((c) => (c.source.enabled ? c.pulls.filter((p) => p.enabled && !p.owner_off) : []))
    const next = pulls.map((p) => p.next_run_at).filter((t): t is string => Boolean(t)).sort()[0]
    return { pulls: pulls.length, next, working: items.filter((c) => c.setup?.status === "pending").length }
  }, [items])

  return (
    <Section
      title="Connection setups"
      description={
        agent.data && items.length
          ? `${totals.pulls} ${totals.pulls === 1 ? "pull" : "pulls"} scheduled${totals.next ? `, next ${until(totals.next)}` : ""}${totals.working ? ` · setting up ${totals.working}` : ""}. Pulls run without the AI`
          : "When you connect something, the agent works out what to read, tests it and schedules its pulls"
      }
    >
      {!agent.data && !agent.error ? (
        <Card>
          <div className="agent-card-head">
            <Skeleton width={36} height={36} />
            <div className="row-text">
              <Skeleton width={120} />
              <Skeleton width={200} />
            </div>
          </div>
        </Card>
      ) : agent.error && !agent.data ? (
        <Empty title="Couldn't reach the server" description={agent.error.message} />
      ) : items.length === 0 ? (
        <Card>
          <Empty title="Nothing connected yet" description="Your Mac's screen text is enough on its own. Connections add calendar, mail, notes and more">
            <Button render={<Link to="/connections" />}>Connect something</Button>
          </Empty>
        </Card>
      ) : (
        <div className="agent-list">
          {items.map((item) => (
            <AgentCard key={item.source.id} item={item} logoId={logoIds.get(item.source.name.toLowerCase())} focused={item.source.id === focus} refresh={agent.refresh} />
          ))}
        </div>
      )}
    </Section>
  )
}

export { AgentCard, ConnectionSetups }
