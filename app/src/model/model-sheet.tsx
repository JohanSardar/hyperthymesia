import { ChevronLeft, Plus } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import { api, type Catalog, type CatalogModel, type ModelChoice, type ProviderId } from "../server/api"
import { useSettings } from "../server/settings"
import { Badge, Button, Card, CardHeader, DialogClose, Field, IconButton, Input, Radio, RadioGroup, SearchInput, SecretInput, Segmented, Sheet, Skeleton, buttonAttrs, toast } from "../ui"
import { SigninPanel } from "../agent/signin"
import { provider, providers, ProviderTile, type Provider } from "./providers"
import "./model.css"

type Props = {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved?: (choice: ModelChoice) => void
  /** Opens straight on this provider's models. */
  initialProvider?: ProviderId | null
}

function ModelSheet({ open, onOpenChange, onSaved, initialProvider }: Props) {
  const { data, save, reload } = useSettings()
  const current = data?.settings.model ?? null
  const [view, setView] = useState<ProviderId | null>(null)
  const [direction, setDirection] = useState<"forward" | "back">("forward")

  useEffect(() => {
    if (open) setView(initialProvider ?? null)
  }, [open, initialProvider])

  const go = (next: ProviderId | null) => {
    setDirection(next ? "forward" : "back")
    setView(next)
  }

  const form = view ? provider(view) : null
  return (
    <Sheet
      open={open}
      onOpenChange={onOpenChange}
      className="model-sheet"
      initialFocus={view ? "auto" : 'input[type="search"]'}
      title={
        form ? (
          <span className="model-sheet-title">
            <IconButton label="All providers" size="xs" onClick={() => go(null)}>
              <ChevronLeft size={14} aria-hidden />
            </IconButton>
            <ProviderTile id={form.id} size={24} />
            {form.kind === "key" ? `Connect ${form.name}` : `Use ${form.name}`}
          </span>
        ) : (
          "Choose a model"
        )
      }
      description={form ? undefined : "Pick the provider your agent runs on, then any model it offers"}
    >
      <div key={view ?? "list"} className="sheet-view" data-direction={direction}>
        {form ? (
          <ProviderForm
            provider={form}
            current={current?.provider === form.id ? current : null}
            keySaved={data?.gateway_key_configured ?? false}
            onCancel={() => onOpenChange(false)}
            onSave={async (choice) => {
              await save({ model: choice })
              await reload()
              toast.success("Model saved", { description: "The agent uses it from its next run" })
              onSaved?.(choice)
              onOpenChange(false)
            }}
          />
        ) : (
          <ProviderList current={current?.provider ?? null} onPick={go} />
        )}
      </div>
    </Sheet>
  )
}

function ProviderList({ current, onPick }: { current: ProviderId | null; onPick: (id: ProviderId) => void }) {
  const [query, setQuery] = useState("")
  const match = (p: Provider) => `${p.name} ${p.host} ${p.blurb}`.toLowerCase().includes(query.trim().toLowerCase())
  const groups = [
    { title: "Subscriptions", items: providers.filter((p) => p.kind === "signin" && match(p)) },
    { title: "API keys", items: providers.filter((p) => p.kind === "key" && match(p)) },
  ].filter((g) => g.items.length)
  return (
    <>
      <SearchInput placeholder="Search providers" value={query} onChange={(e) => setQuery(e.currentTarget.value)} aria-label="Search providers" />
      {groups.map((group) => (
        <section key={group.title} className="provider-group">
          <h3 className="h2">{group.title}</h3>
          <div className="provider-grid">
            {group.items.map((p) => (
              <button key={p.id} type="button" className="provider-card" onClick={() => onPick(p.id)}>
                <ProviderTile id={p.id} />
                <span className="provider-card-text">
                  <span className="row-label">{p.name}</span>
                  <span className="provider-card-host">{p.host}</span>
                </span>
                {current === p.id && <span className="provider-card-aside">In use</span>}
              </button>
            ))}
          </div>
        </section>
      ))}
      {!groups.length && <p className="provider-none">No provider matches “{query}”</p>}
      <p className="provider-note">These are the providers fx can run. Vercel AI Gateway reaches models from Anthropic, OpenAI, Google, xAI and many more with one key.</p>
    </>
  )
}

type FormProps = {
  provider: Provider
  current: ModelChoice | null
  keySaved: boolean
  onCancel: () => void
  onSave: (choice: ModelChoice) => Promise<void>
}

function ProviderForm({ provider: p, current, keySaved, onCancel, onSave }: FormProps) {
  const [key, setKey] = useState("")
  const [hasKey, setHasKey] = useState(keySaved)
  const [catalog, setCatalog] = useState<Catalog | null>(null)
  const [loading, setLoading] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)
  const [query, setQuery] = useState("")
  const [model, setModel] = useState<string | null>(current?.model ?? null)
  const [effort, setEffort] = useState<string | null>(current?.effort ?? null)
  const [custom, setCustom] = useState(false)

  const check = async () => {
    setLoading(true)
    setFailure(null)
    try {
      if (p.kind === "key" && key) {
        await api.saveGatewayKey(key.trim())
        setHasKey(true)
        setKey("")
      }
      const next = await api.catalog(p.id)
      setCatalog(next)
      if (next.error) setFailure(p.kind === "signin" ? "Not signed in yet" : "Couldn't read the model list")
    } catch (cause) {
      setFailure(cause instanceof Error ? cause.message : "That didn't work")
    } finally {
      setLoading(false)
    }
  }

  // Reading the list is free and needs no key, so do it straight away.
  useEffect(() => {
    void check()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.id])

  useEffect(() => {
    if (catalog && current && !catalog.models.some((m) => m.id === current.model)) setCustom(true)
  }, [catalog, current])

  const models = catalog?.models ?? []
  const chosen = models.find((m) => m.id === model)
  const efforts = custom ? [] : (chosen?.efforts ?? [])
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    return q ? models.filter((m) => `${m.id} ${m.name ?? ""}`.toLowerCase().includes(q)) : models
  }, [models, query])
  const groups = useMemo(() => group(filtered), [filtered])

  const pick = (id: string | null) => {
    setModel(id)
    const next = models.find((m) => m.id === id)
    if (!next?.efforts?.includes(effort ?? "")) setEffort(null)
  }

  const ready = Boolean(model && model.trim()) && (p.kind === "signin" || hasKey || Boolean(key.trim()))

  return (
    <>
      <section className="provider-group">
        <h3 className="h2">{p.kind === "key" ? "Endpoint" : "Account"}</h3>
        <Card className="form-card">
          {p.kind === "key" ? (
            <Field
              label="API key"
              description={
                hasKey
                  ? "A key is saved on your server. Paste a new one to replace it."
                  : "Saved on your Hyperthymesia server when you check access. It's never shown again."
              }
            >
              <SecretInput placeholder={hasKey ? "Key saved" : "API key"} value={key} onValueChange={setKey} aria-label="API key" />
            </Field>
          ) : (
            <div className="signin-note">
              <span className="row-label">Sign in with your {p.account} account</span>
              <span className="row-description">fx keeps the login on this computer. Hyperthymesia never sees your password.</span>
              {failure && !loading && <SigninPanel id={p.id} compact onDone={check} />}
            </div>
          )}
          <div className="form-card-foot">
            <AccessState loading={loading} catalog={catalog} failure={failure} needsKey={p.kind === "key" && !hasKey} ok={p.kind === "key" ? "Key saved" : "Signed in"} />
            <Button variant="soft" onClick={check} disabled={loading || (p.kind === "key" && !key && !hasKey)}>
              Check access
            </Button>
          </div>
        </Card>
      </section>

      <section className="provider-group">
        <h3 className="h2">Model</h3>
        <Card className="models-card">
          <CardHeader
            title={catalog?.models.length ? `${p.name} offers ${catalog.models.length} models` : "No models listed yet"}
            actions={catalog?.models.length ? <SearchInput size="sm" placeholder="Find models" value={query} onChange={(e) => setQuery(e.currentTarget.value)} aria-label="Find models" /> : undefined}
          />
          {loading && !catalog ? (
            <div className="models-loading">
              <Skeleton width="40%" />
              <Skeleton width="62%" />
              <Skeleton width="48%" />
            </div>
          ) : custom ? (
            <div className="custom-model">
              <Field label="Model ID" description={`Exactly as ${p.name} names it`}>
                <Input mono autoFocus placeholder={p.id === "vercel" ? "provider/model" : "model-id"} value={model ?? ""} onChange={(e) => setModel(e.currentTarget.value)} />
              </Field>
              {models.length > 0 && (
                <Button variant="ghost" size="xs" onClick={() => { setCustom(false); setModel(null) }}>
                  Back to the list
                </Button>
              )}
            </div>
          ) : models.length ? (
            <RadioGroup aria-label="Model" value={model} onValueChange={(v) => pick(v as string)} className="model-list">
              {groups.map(([vendor, items]) => (
                <div key={vendor} className="model-group">
                  {vendor && <div className="model-group-label">{vendorName(vendor)}</div>}
                  {items.map((m) => (
                    <label key={m.id} className="model-row">
                      <Radio value={m.id} />
                      <span className="model-row-name">{m.name || m.id}</span>
                      <span className="model-row-id">{m.id}</span>
                    </label>
                  ))}
                </div>
              ))}
              {!filtered.length && <p className="provider-none">No model matches “{query}”</p>}
            </RadioGroup>
          ) : (
            <p className="models-empty">
              {failure && p.kind === "signin"
                ? `Sign in to ${p.name} above to see its models, or enter a model ID now.`
                : "Check access to list the models"}
            </p>
          )}
          {!custom && catalog?.allows_custom_model !== false && (
            <div className="models-foot">
              <Button variant="ghost" size="xs" onClick={() => { setCustom(true); setModel(null) }}>
                <Plus size={12} aria-hidden /> Use a model ID
              </Button>
            </div>
          )}
        </Card>
        {efforts.length > 0 && (
          <Card className="effort-card">
            <div className="row">
              <div className="row-text">
                <span className="row-label">Effort</span>
                <span className="row-description">How hard {chosen?.name ?? "the model"} thinks before answering</span>
              </div>
              <div className="row-control">
                <Segmented
                  aria-label="Effort"
                  value={effort ?? "default"}
                  onValueChange={(v) => setEffort(!v || v === "default" ? null : (v as string))}
                  items={[{ value: "default", label: "Default" }, ...efforts.map((e) => ({ value: e, label: effortLabel(e) }))]}
                />
              </div>
            </div>
          </Card>
        )}
      </section>

      <footer className="sheet-footer model-sheet-footer">
        <a className="btn" data-variant="link" data-size="sm" href={p.help} target="_blank" rel="noreferrer">
          {p.name} help ↗
        </a>
        <span className="model-sheet-footer-end">
          <DialogClose {...buttonAttrs("ghost", "sm")} onClick={onCancel}>
            Cancel
          </DialogClose>
          <Button variant="primary" disabled={!ready} onClick={async () => {
              try {
                if (p.kind === "key" && key.trim()) await api.saveGatewayKey(key.trim())
                await onSave({ provider: p.id, model: model!.trim(), effort })
              } catch (cause) {
                toast.error("Couldn't save", { description: cause instanceof Error ? cause.message : undefined })
              }
            }}>
            Use this model
          </Button>
        </span>
      </footer>
    </>
  )
}

function AccessState({ loading, catalog, failure, needsKey, ok }: { loading: boolean; catalog: Catalog | null; failure: string | null; needsKey: boolean; ok: string }) {
  if (loading) return <span className="access-state">Checking…</span>
  if (failure) return <Badge tone="warning">{failure}</Badge>
  if (needsKey) return <span className="access-state">Add a key to use these models</span>
  if (catalog) return <Badge tone="success">{ok}</Badge>
  return <span />
}

function group(models: CatalogModel[]): [string, CatalogModel[]][] {
  const out = new Map<string, CatalogModel[]>()
  for (const m of models) {
    const vendor = m.id.includes("/") ? m.id.split("/")[0] : ""
    out.set(vendor, [...(out.get(vendor) ?? []), m])
  }
  return [...out]
}

const vendors: Record<string, string> = { openai: "OpenAI", xai: "xAI", deepseek: "DeepSeek", moonshotai: "Moonshot AI", zai: "Z.ai", meta: "Meta", mistral: "Mistral", google: "Google", anthropic: "Anthropic", alibaba: "Alibaba", amazon: "Amazon", minimax: "MiniMax" }

function vendorName(id: string) {
  return vendors[id] ?? id.charAt(0).toUpperCase() + id.slice(1)
}

function effortLabel(effort: string) {
  return effort === "xhigh" ? "Extra high" : effort.charAt(0).toUpperCase() + effort.slice(1)
}

export { effortLabel, ModelSheet }
