import { ArrowRight } from "lucide-react"
import { useEffect, useState } from "react"
import { Link, useParams } from "react-router"
import { describe, happenedAt } from "../lib/entries"
import { ago, clock } from "../lib/time"
import { useLogoIds } from "../pages/connections"
import { api, usePolled, type Envelope } from "../server/connections"
import { PageHeader } from "../shell"
import { Button, Card, CardHeader, Dialog, DialogClose, buttonAttrs, Empty, Field, Page, Row, SecretInput, Skeleton, Switch, toast } from "../ui"
import { useCatalog } from "./catalog"
import { Logo } from "./logo"
import { StatusLabel } from "./parts"
import "./connections.css"
import "../pages/today.css"

type Item = { id: number; envelope: Envelope }

function ReconnectDialog({ id, name, hasKey, open, onOpenChange, done }: { id: string; name: string; hasKey: boolean; open: boolean; onOpenChange: (o: boolean) => void; done: () => unknown }) {
  const [key, setKey] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  async function submit() {
    setBusy(true)
    setError(null)
    try {
      await api.reconnect(id, key.trim() || undefined)
      toast.success(`Rechecking ${name}`, { description: "Setup resumes if it can be reached" })
      onOpenChange(false)
      setKey("")
      await done()
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't reconnect")
    } finally {
      setBusy(false)
    }
  }
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      size="sm"
      title={`Reconnect ${name}`}
      description={hasKey ? "Paste a new token, or leave it empty to recheck the saved one." : "Rechecks the saved sign-in and resumes setup."}
      footer={
        <>
          <DialogClose {...buttonAttrs("default", "sm")}>Cancel</DialogClose>
          <Button variant="primary" disabled={busy} onClick={submit}>
            {busy ? "Checking…" : "Reconnect"}
          </Button>
        </>
      }
    >
      <div className="connect-card-body" style={{ padding: 0 }}>
        {hasKey && (
          <Field label="New token" description="Optional. Headers and settings stay as they are.">
            <SecretInput value={key} onValueChange={setKey} placeholder="Paste a read-only token" autoFocus />
          </Field>
        )}
        {error && <div className="form-error">{error}</div>}
      </div>
    </Dialog>
  )
}

function ConnectionPage() {
  const { id = "" } = useParams()
  const sources = usePolled(api.connections, 5000)
  const { catalog } = useCatalog()
  const logoIds = useLogoIds(catalog)
  const source = sources.data?.find((s) => s.id === id)
  const [items, setItems] = useState<Item[] | null>(null)
  const [before, setBefore] = useState<number | null>(null)
  const [reconnect, setReconnect] = useState(false)

  useEffect(() => {
    api.entries(id).then((page) => {
      setItems(page.entries)
      setBefore(page.entries.length ? page.next_before : null)
    }, () => setItems([]))
  }, [id, source?.last_pull_at])

  async function more() {
    if (!before) return
    const page = await api.entries(id, before)
    setItems((cur) => [...(cur ?? []), ...page.entries])
    setBefore(page.entries.length ? page.next_before : null)
  }

  if (sources.data && !source) {
    return (
      <>
        <PageHeader crumbs={[{ label: "Connections", to: "/connections" }, { label: "Not found" }]} />
        <Page width="settings">
          <Empty title="That connection doesn't exist">
            <Button render={<Link to="/connections" />}>Back to connections</Button>
          </Empty>
        </Page>
      </>
    )
  }

  return (
    <>
      <PageHeader crumbs={[{ label: "Connections", to: "/connections" }, { label: source?.name ?? "…" }]} />
      <Page width="settings">
        {!source ? (
          <Skeleton width="100%" height={200} />
        ) : (
          <>
            <div className="detail-main">
              <Logo id={logoIds.get(source.name.toLowerCase())} name={source.name} size={44} />
              <div className="detail-title" style={{ marginTop: 0 }}>
                <div>
                  <h2 className="h1">{source.name}</h2>
                  <p className="detail-subtitle yours-meta" style={{ marginTop: 8 }}>
                    <StatusLabel source={source} />
                    <span className="sep">·</span>
                    <span>{source.last_pull_at ? `Pulled ${ago(source.last_pull_at)}` : "Not pulled yet"}</span>
                  </p>
                </div>
                {source.status === "needs_reconnect" && (
                  <Button variant="primary" size="md" onClick={() => setReconnect(true)}>
                    Reconnect
                  </Button>
                )}
              </div>
            </div>

            <Card>
              <Row
                label="On"
                description={source.enabled ? "The agent may read it and pulls run on schedule" : "No reads and no pulls. Its token and entries are kept."}
                control={
                  <Switch
                    checked={source.enabled}
                    aria-label="On"
                    onCheckedChange={async (checked) => {
                      try {
                        await api.setEnabled(source.id, checked)
                        toast.success(checked ? `${source.name} is back on` : `${source.name} is off`)
                      } catch (e) {
                        toast.error("Couldn't change it", { description: (e as Error).message })
                      }
                      await sources.refresh()
                    }}
                  />
                }
              />
              <Row label="What the agent set up" description="What it understood, its scheduled pulls, retry and off switches" control={<Button variant="ghost" render={<Link to={`/agent?focus=${source.id}`} />}>Open <ArrowRight size={12} aria-hidden /></Button>} />
              {!source.oauth && source.status !== "needs_reconnect" && (
                <Row label="Reconnect" description={source.has_key ? "Replace the token or recheck it" : "Recheck access and resume setup"} control={<Button onClick={() => setReconnect(true)}>Reconnect</Button>} />
              )}
              {source.last_error && <Row label="Last error" description={<span className="pull-error">{source.last_error}</span>} />}
            </Card>

            <Card>
              <CardHeader title="Details" />
              <Row label="Connected" description={new Date(source.connected_at).toLocaleString()} />
              <Row label="First pull reached back" description={source.history_days ? `${source.history_days} ${source.history_days === 1 ? "day" : "days"}` : "Only new entries"} />
              <Row label="Sign-in" description={source.oauth ? "OAuth, refreshed automatically" : source.has_key ? "Token, kept private on this Mac" : "None, or your existing sign-in"} />
            </Card>

            <section className="section">
              <h3 className="h2">Entries</h3>
              {items === null ? (
                <Skeleton width="100%" height={120} />
              ) : items.length === 0 ? (
                <Card>
                  <Empty title="Nothing pulled yet" description="Entries appear here after the first pull" />
                </Card>
              ) : (
                <>
                  <Card className="log">
                    {items.map((item) => {
                      const line = describe(item.envelope)
                      const Icon = line.icon
                      const at = happenedAt(item.envelope)
                      return (
                        <div className="log-row" key={item.id}>
                          <div className="log-row-inner" style={{ gridTemplateColumns: "88px minmax(0,1fr) auto" }}>
                            <time className="log-time" dateTime={at}>
                              {new Date(at).toDateString() === new Date().toDateString() ? clock(at) : new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric" })}
                            </time>
                            <span className="log-text">
                              <Icon size={13} strokeWidth={1.75} aria-hidden className="log-kind" />
                              <span className="log-line">{line.text}</span>
                            </span>
                            {line.detail && <span className="log-detail">{line.detail}</span>}
                          </div>
                        </div>
                      )
                    })}
                  </Card>
                  {before && (
                    <div>
                      <Button variant="ghost" onClick={more}>
                        Show more
                      </Button>
                    </div>
                  )}
                </>
              )}
            </section>
            <ReconnectDialog id={source.id} name={source.name} hasKey={source.has_key} open={reconnect} onOpenChange={setReconnect} done={sources.refresh} />
          </>
        )}
      </Page>
    </>
  )
}

export { ConnectionPage }
