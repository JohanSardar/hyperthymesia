import { ArrowRight, Check, ShieldCheck } from "lucide-react"
import { ConnectionsSidebar } from "./sidebar"
import { useMemo, useState } from "react"
import { Link, useNavigate, useParams } from "react-router"
import { ApiError } from "../server/http"
import { api, usePolled, type CatalogEntry, type ConnectBody } from "../server/connections"
import { PageHeader } from "../shell"
import { Button, Card, CardHeader, Dialog, DialogClose, buttonAttrs, Empty, Field, Input, Page, Row, SecretInput, Select, Skeleton, toast } from "../ui"
import { authLabel, reach, useCatalog } from "./catalog"
import { blurb } from "./blurbs"
import { Logo } from "./logo"
import { HISTORY, kindLabel, useDefaultHistory } from "./parts"
import "./connections.css"

/** Sends one connect request and takes the person to where the agent's work shows. */
function useConnect() {
  const navigate = useNavigate()
  return async (body: ConnectBody, name: string) => {
    const created = await api.connect(body)
    if (created.status === "needs_reconnect") {
      toast.warning(`${name} is saved but couldn't be reached`, { description: "Check the token or sign-in, then reconnect" })
      navigate(`/connections/${created.id}`)
    } else {
      toast.success(`${name} connected`, { description: "The agent is setting it up now" })
      navigate(`/agent?focus=${created.id}`)
    }
  }
}

function ConnectDialog({ entry, open, onOpenChange }: { entry: CatalogEntry; open: boolean; onOpenChange: (open: boolean) => void }) {
  const connect = useConnect()
  const [key, setKey] = useState("")
  const [variables, setVariables] = useState<Record<string, string>>({})
  const defaultHistory = useDefaultHistory()
  const [history, setHistory] = useState(defaultHistory)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const needsKey = entry.auth === "key"
  const ready = (!needsKey || key.trim()) && entry.variables.every((v) => variables[v]?.trim())

  async function submit() {
    if (!ready || busy) return
    setBusy(true)
    setError(null)
    const base = { key: needsKey ? key.trim() : undefined, history_days: Number(history) }
    const body: ConnectBody = entry.id.startsWith("x-")
      ? { ...base, name: entry.name, description: entry.description, connection: entry.connection }
      : { ...base, catalog_id: entry.id, variables: entry.variables.length ? variables : undefined }
    try {
      await connect(body, entry.name)
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "The server couldn't be reached")
      setBusy(false)
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      size="sm"
      title={`Connect ${entry.name}`}
      description={entry.requirements}
      footer={
        <>
          <DialogClose {...buttonAttrs("default", "sm")}>Cancel</DialogClose>
          <Button variant="primary" disabled={!ready || busy} onClick={submit}>
            {busy ? "Checking access…" : "Connect"}
          </Button>
        </>
      }
    >
      <form
        className="connect-card-body"
        style={{ padding: 0 }}
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        {needsKey && (
          <Field label="Token" description="Stored privately on this Mac and sent only in request headers. The agent never sees it.">
            <SecretInput value={key} onValueChange={setKey} placeholder="Paste a read-only token" autoFocus />
          </Field>
        )}
        {entry.variables.map((v) => (
          <Field key={v} label={v.charAt(0).toUpperCase() + v.slice(1)} description={v === "site" ? "The part before .atlassian.net" : undefined}>
            <Input value={variables[v] ?? ""} onChange={(e) => setVariables((cur) => ({ ...cur, [v]: e.currentTarget.value }))} />
          </Field>
        ))}
        {entry.auth === "existing_login" && (
          <p className="row-description">
            Uses <span className="mono-line">{reach(entry).detail}</span> as you're already signed in on this Mac. Nothing is installed and your sign-in isn't changed.
          </p>
        )}
        <Field label="Start from" description="How far back the first pull reaches">
          <Select items={HISTORY} value={history} onValueChange={(v) => setHistory(v ?? defaultHistory)} width="full" aria-label="Start from" />
        </Field>
        {error && <div className="form-error">{error}</div>}
        <button type="submit" hidden />
      </form>
    </Dialog>
  )
}

function CatalogDetailPage() {
  const { id = "" } = useParams()
  const { catalog, error } = useCatalog()
  const sources = usePolled(api.connections)
  const [open, setOpen] = useState(false)
  const connect = useConnect()
  const navigate = useNavigate()
  const [busy, setBusy] = useState(false)
  const entry = catalog?.byId.get(id)
  const existing = useMemo(() => (entry ? (sources.data ?? []).filter((s) => s.name.toLowerCase() === entry.name.toLowerCase()) : []), [sources.data, entry])

  if (!catalog) {
    return (
      <>
        <ConnectionsSidebar />
        <PageHeader crumbs={[{ label: "Connections", to: "/connections" }, { label: "…" }]} />
        <Page width="detail">{error ? <Empty title="Couldn't load the catalog" description={error.message} /> : <Skeleton width="100%" height={320} />}</Page>
      </>
    )
  }
  if (!entry) {
    return (
      <>
        <ConnectionsSidebar />
        <PageHeader crumbs={[{ label: "Connections", to: "/connections" }, { label: "Not found" }]} />
        <Page width="detail">
          <Empty title="That source isn't in the catalog">
            <Button render={<Link to="/connections" />}>Back to connections</Button>
          </Empty>
        </Page>
      </>
    )
  }

  const how = reach(entry)
  const oneClick = entry.auth === "none" && entry.variables.length === 0

  async function connectNow() {
    if (!entry) return
    if (!oneClick) return setOpen(true)
    setBusy(true)
    try {
      await connect(entry.id.startsWith("x-") ? { name: entry.name, description: entry.description, connection: entry.connection } : { catalog_id: entry.id }, entry.name)
    } catch (e) {
      toast.error(`Couldn't connect ${entry.name}`, { description: e instanceof Error ? e.message : undefined })
      setBusy(false)
    }
  }

  return (
    <>
      <ConnectionsSidebar />
      <PageHeader
        crumbs={[
          { label: "Connections", to: "/connections" },
          { label: entry.category, to: `/connections/browse?category=${encodeURIComponent(entry.category)}` },
          { label: entry.name },
        ]}
      />
      <Page width="detail">
        <div className="detail-grid">
          <div className="detail-main">
            <div>
              <Logo id={entry.id} name={entry.name} size={44} />
              <div className="detail-title">
                <div>
                  <h2 className="h1">{entry.name}</h2>
                  <p className="detail-subtitle">{blurb(entry)}</p>
                </div>
                <Button size="md" variant={existing.length ? "default" : "primary"} onClick={connectNow} disabled={busy}>
                  {busy ? "Connecting…" : existing.length ? "Connect another" : "Connect"}
                </Button>
              </div>
            </div>

            {existing.length > 0 && (
              <Card>
                {existing.map((s) => (
                  <Row
                    key={s.id}
                    onClick={() => navigate(`/connections/${s.id}`)}
                    leading={<Check size={14} aria-hidden />}
                    label={`${s.name} is connected`}
                    description="Open it to see its status, entries and what the agent set up"
                  />
                ))}
              </Card>
            )}

            <Card>
              <CardHeader title="How it connects" />
              <Row label="What the agent is told" description={entry.description} />
              <Row label={how.label} description={<span className="mono-line">{how.detail}</span>} control={<span className="kind-chip">{kindLabel(entry)}</span>} />
              <Row label={authLabel(entry)} description={entry.requirements} />
            </Card>

            <Card>
              <CardHeader title="What happens when you connect" />
              <Row leading={<span className="step">1</span>} label="Access is checked" description="One read-only request confirms it can reach your data. Nothing is changed." />
              <Row leading={<span className="step">2</span>} label="The agent sets it up" description="It works out what to read, tests a pull and saves it. You can see and stop everything it set up on the Agent page." />
              <Row leading={<span className="step">3</span>} label="Pulls run every four hours" description="New entries arrive in Today as they're pulled." />
            </Card>
          </div>

          <aside className="detail-aside">
            <div className="aside-item">
              <span className="aside-label">Category</span>
              <span>{entry.category}</span>
            </div>
            <div className="aside-item">
              <span className="aside-label">Connects through</span>
              <span>{how.label}</span>
            </div>
            <div className="aside-item">
              <span className="aside-label">Sign-in</span>
              <span>{authLabel(entry)}</span>
            </div>
            <div className="aside-item">
              <span className="aside-label">Listed by</span>
              <span>{entry.id.startsWith("x-") ? "Ready-made template" : "Built in"}</span>
            </div>
            <div className="aside-rule" />
            <div className="aside-item">
              <span className="aside-label">Permissions</span>
              <ul className="aside-list">
                <li>Reads only; never writes or deletes</li>
                <li>{entry.auth === "key" ? "Token kept private on this Mac" : entry.auth === "existing_login" ? "Uses your existing sign-in as is" : "Needs no sign-in"}</li>
                <li>Pulls every four hours</li>
                <li>Off switch on the Agent page</li>
              </ul>
            </div>
            <div className="aside-rule" />
            <Link to="/connections/new?kind=api" className="view-all">
              <ShieldCheck size={14} aria-hidden /> Add your own instead <ArrowRight size={14} aria-hidden />
            </Link>
          </aside>
        </div>
        <ConnectDialog key={entry.id} entry={entry} open={open} onOpenChange={setOpen} />
      </Page>
    </>
  )
}

export { CatalogDetailPage, useConnect }
