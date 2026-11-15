import { Copy, Ellipsis, KeyRound, Plus, Trash2 } from "lucide-react"
import { useEffect, useState } from "react"
import { useNavigate } from "react-router"
import { ModelCheckButton } from "../model/model-check"
import { effortLabel, ModelSheet } from "../model/model-sheet"
import { provider, ProviderTile } from "../model/providers"
import { api, type ApiKey, type Appearance, type Source } from "../server/api"
import { useSettings } from "../server/settings"
import { PageHeader } from "../shell"
import { useReplayTour } from "../tour/tour"
import {
  Badge,
  Button,
  Card,
  CardHeader,
  Checkbox,
  Confirm,
  ConfirmTrigger,
  Dialog,
  Empty,
  Field,
  IconButton,
  Input,
  Menu,
  MenuContent,
  MenuItem,
  MenuTrigger,
  Page,
  PageIntro,
  Row,
  SecretInput,
  Section,
  Segmented,
  Select,
  Skeleton,
  StatusDot,
  Switch,
  buttonAttrs,
  toast,
} from "../ui"
import "./settings.css"

function SettingsPage() {
  const { data, error } = useSettings()
  return (
    <>
      <PageHeader title="Settings" />
      <Page width="settings">
        <PageIntro title="Settings" description="Your model, what gets recorded, where it's kept and who can read it" />
        {!data ? (
          error ? (
            <Empty title="Settings didn't load" description={error} />
          ) : (
            <Card>
              {[0, 1, 2].map((i) => (
                <Row key={i} label={<Skeleton width={140} />} description={<Skeleton width={260} />} />
              ))}
            </Card>
          )
        ) : (
          <>
            <ModelSection />
            <ConnectionsSection />
            <RecordingSection />
            <StorageSection />
            <ApiKeysSection />
            <AppearanceSection />
            <GettingStartedSection />
          </>
        )}
      </Page>
    </>
  )
}

function GettingStartedSection() {
  const replay = useReplayTour()
  return (
    <Section title="Getting started">
      <Card>
        <Row label="Tour" description="A short walk through Today, Connections, Agent and Settings" control={<Button onClick={() => void replay()}>Replay the tour</Button>} />
      </Card>
    </Section>
  )
}

function ModelSection() {
  const { data, reload } = useSettings()
  const [open, setOpen] = useState(false)
  const model = data!.settings.model
  const keySaved = data!.gateway_key_configured
  const removeKey = async () => {
    try {
      await api.removeGatewayKey()
      await reload()
      toast.success("Key removed")
    } catch (cause) {
      toast.error("Couldn't remove the key", { description: cause instanceof Error ? cause.message : undefined })
    }
  }
  return (
    <Section title="Model" description="The agent uses this for every pass and setup job. Changes apply to its next run">
      <Card>
        {model ? (
          <Row
            leading={<ProviderTile id={model.provider} />}
            label={model.model}
            description={`${provider(model.provider).name} · ${model.effort ? `${effortLabel(model.effort)} effort` : "Model's default effort"}`}
            control={<Button onClick={() => setOpen(true)}>Change</Button>}
          />
        ) : (
          <Row
            leading={<span className="tile-empty" aria-hidden />}
            label="No model chosen"
            description="The agent can't run until you pick one"
            control={
              <Button variant="primary" onClick={() => setOpen(true)}>
                Choose a model
              </Button>
            }
          />
        )}
        {model && <Row label="Test the model" description="Sends one short request. Your provider may charge a little for it" control={<ModelCheckButton />} />}
        {(keySaved || model?.provider === "vercel") && (
          <Row
            label="Vercel AI Gateway key"
            description={keySaved ? "Saved on this server. It's never shown again" : "Not saved. Add it from Change › Vercel AI Gateway"}
            control={
              keySaved ? (
                <Confirm
                  title="Remove the gateway key?"
                  description="Vercel AI Gateway models stop working until you add a key again. A key set in the server's environment stays."
                  confirmLabel="Remove key"
                  destructive
                  trigger={<ConfirmTrigger {...buttonAttrs("ghost", "sm")}>Remove</ConfirmTrigger>}
                  onConfirm={removeKey}
                />
              ) : (
                <Badge tone="warning">Missing</Badge>
              )
            }
          />
        )}
      </Card>
      <ModelSheet open={open} onOpenChange={setOpen} />
    </Section>
  )
}

function ConnectionsSection() {
  const navigate = useNavigate()
  const [sources, setSources] = useState<Source[] | null>(null)
  useEffect(() => {
    api.sources().then(setSources, () => setSources([]))
  }, [])
  const on = sources?.filter((s) => s.enabled).length ?? 0
  return (
    <Section title="Connections" description="Everything besides this Mac that feeds your history">
      <Card>
        <Row
          label="Your connections"
          description={sources === null ? "Loading…" : sources.length ? `${sources.length} connected, ${on} on` : "Nothing connected yet"}
          onClick={() => navigate("/connections")}
        />
        <Row label="What the agent set up" description="Pulls it scheduled for each connection, last run and retries" onClick={() => navigate("/agent")} />
      </Card>
    </Section>
  )
}

function RecordingSection() {
  const { data, save } = useSettings()
  const [pending, setPending] = useState(false)
  const on = data!.settings.mac_recording
  const toggle = async (next: boolean) => {
    setPending(true)
    try {
      await save({ mac_recording: next })
      toast(next ? "Recording on" : "Recording paused", { description: next ? "Paired Macs start capturing within a few seconds" : "Paired Macs stop capturing within a few seconds" })
    } catch (cause) {
      toast.error("Couldn't change recording", { description: cause instanceof Error ? cause.message : undefined })
    } finally {
      setPending(false)
    }
  }
  const copy = async () => {
    try {
      await navigator.clipboard.writeText("hx-server setup")
      toast.success("Copied", { description: "Paste it into Terminal on the Mac" })
    } catch {
      toast.error("Couldn't copy")
    }
  }
  return (
    <Section title="Mac recording">
      <Card>
        <Row
          label="Record on paired Macs"
          description="Screen text, apps and windows. Pausing keeps your permissions; queued entries still upload"
          control={
            <>
              <StatusDot tone={on ? "live" : "idle"} />
              <Switch aria-label="Record on paired Macs" checked={on} disabled={pending} onCheckedChange={toggle} />
            </>
          }
        />
        <Row
          label="Permissions"
          description={
            <>
              To turn on or re-check Accessibility and Screen Recording, run <code className="inline-code">hx-server setup</code> on the Mac
            </>
          }
          control={
            <IconButton label="Copy command" onClick={copy}>
              <Copy size={14} aria-hidden />
            </IconButton>
          }
        />
      </Card>
    </Section>
  )
}

const retention = [7, 30, 90, 180, 365, 3650]

function days(n: number) {
  if (n === 3650) return "10 years"
  if (n === 365) return "1 year"
  return `${n} days`
}

function bytes(n: number) {
  const units = ["bytes", "KB", "MB", "GB", "TB"]
  let value = n
  let unit = 0
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000
    unit++
  }
  return `${unit ? value.toFixed(value < 10 ? 1 : 0) : value} ${units[unit]}`
}

function StorageSection() {
  const { data, save } = useSettings()
  const { storage, settings } = data!
  const options = [...new Set([...retention, settings.chunk_retention_days])].sort((a, b) => a - b)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(storage.state_dir)
      toast.success("Copied path")
    } catch {
      toast.error("Couldn't copy")
    }
  }
  return (
    <Section title="Data and storage" description="Everything stays on your server. Nothing is deleted automatically yet">
      <Card>
        <Row
          label="Data folder"
          description={<span className="mono path">{storage.state_dir}</span>}
          control={
            <IconButton label="Copy path" onClick={copy}>
              <Copy size={14} aria-hidden />
            </IconButton>
          }
        />
        <Row label="Screenshots and media" description="Stored on the server for your entries" control={<span className="row-value">{bytes(storage.published_blob_bytes)}</span>} />
        <Row
          label="Keep detailed chunks for"
          description="Sets the clean-up policy. Clean-up is off for now, so changing this deletes nothing"
          control={
            <Select
              aria-label="Keep detailed chunks for"
              width={144}
              value={String(settings.chunk_retention_days)}
              onValueChange={(value) => {
                if (!value) return
                save({ chunk_retention_days: Number(value) }).then(
                  () => toast.success("Saved"),
                  (cause) => toast.error("Couldn't save", { description: cause instanceof Error ? cause.message : undefined }),
                )
              }}
              items={options.map((n) => ({ value: String(n), label: days(n) }))}
            />
          }
        />
      </Card>
    </Section>
  )
}

const scopes = [
  { value: "read_l3", label: "Read your history", description: "The written-up history, not raw captures" },
  { value: "read_raw", label: "Read everything", description: "Raw entries and screenshots too" },
  { value: "ingest", label: "Send entries", description: "For a device or script that adds entries" },
]

const prefixes = [
  { value: "source:", label: "Connections" },
  { value: "macos-agent:", label: "Mac activity" },
  { value: "macos-media:", label: "Mac screenshots" },
  { value: "companion:", label: "Companions" },
]

function describeKey(key: ApiKey) {
  const access = key.scopes.map((s) => scopes.find((x) => x.value === s)?.label ?? s).join(", ")
  const reach = prefixes.every((p) => key.prefixes.includes(p.value)) ? "everything" : key.prefixes.map((p) => prefixes.find((x) => x.value === p)?.label ?? p).join(", ")
  return `${access} · ${reach}`
}

function ApiKeysSection() {
  const { data, reload } = useSettings()
  const [creating, setCreating] = useState(false)
  const [revoking, setRevoking] = useState<ApiKey | null>(null)
  const [showRevoked, setShowRevoked] = useState(false)
  const keys = data!.api_keys
  const live = keys.filter((k) => !k.revoked)
  const revoked = keys.length - live.length
  const shown = showRevoked ? keys : live

  const revoke = async (key: ApiKey) => {
    try {
      await api.revokeKey(key.id)
      await reload()
      toast.success(`Revoked ${key.name}`)
    } catch (cause) {
      toast.error("Couldn't revoke", { description: cause instanceof Error ? cause.message : undefined })
    }
  }

  return (
    <Section title="API keys" description="Programs use these to read your history through /v1. Paired Macs and this app have keys here too">
      <Card>
        <CardHeader
          title={live.length === 1 ? "1 key" : `${live.length} keys`}
          actions={
            <>
              {revoked > 0 && (
                <Button variant="ghost" size="xs" onClick={() => setShowRevoked((v) => !v)}>
                  {showRevoked ? "Hide revoked" : `Show ${revoked} revoked`}
                </Button>
              )}
              <IconButton label="Create API key" variant="default" onClick={() => setCreating(true)}>
                <Plus size={14} aria-hidden />
              </IconButton>
            </>
          }
        />
        {shown.map((key) => (
          <div key={key.id} className="key-row" data-revoked={key.revoked ? "" : undefined}>
            <Row
              leading={
                <span className="key-glyph" aria-hidden>
                  <KeyRound size={14} strokeWidth={1.75} />
                </span>
              }
              label={key.name}
              description={describeKey(key)}
              control={
                key.revoked ? (
                  <Badge>Revoked</Badge>
                ) : (
                  <Menu>
                    <MenuTrigger aria-label={`More for ${key.name}`} {...buttonAttrs("ghost", "sm", undefined, true)}>
                      <Ellipsis size={14} aria-hidden />
                    </MenuTrigger>
                    <MenuContent align="end">
                      <MenuItem value="revoke" icon={<Trash2 size={14} />} onClick={() => setRevoking(key)}>
                        Revoke
                      </MenuItem>
                    </MenuContent>
                  </Menu>
                )
              }
            />
          </div>
        ))}
      </Card>
      <CreateKeyDialog open={creating} onOpenChange={setCreating} onCreated={reload} />
      <Confirm
        open={revoking !== null}
        onOpenChange={(open) => !open && setRevoking(null)}
        title={`Revoke ${revoking?.name ?? "key"}?`}
        description="Anything using it stops working on its next request. If this is the key this browser signed in with, you'll be signed out."
        confirmLabel="Revoke"
        destructive
        onConfirm={() => revoking && revoke(revoking)}
      />
    </Section>
  )
}

function CreateKeyDialog({ open, onOpenChange, onCreated }: { open: boolean; onOpenChange: (open: boolean) => void; onCreated: () => Promise<void> }) {
  const [name, setName] = useState("")
  const [scope, setScope] = useState("read_l3")
  const [reach, setReach] = useState<string[]>(prefixes.map((p) => p.value))
  const [token, setToken] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (open) {
      setName("")
      setScope("read_l3")
      setReach(prefixes.map((p) => p.value))
      setToken(null)
      setError(null)
    }
  }, [open])

  const create = async () => {
    setError(null)
    try {
      const created = await api.createKey({ name: name.trim(), prefixes: reach, scopes: [scope] })
      setToken(created.token)
      await onCreated()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Couldn't create the key")
    }
  }

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(token!)
      toast.success("Copied key")
    } catch {
      toast.error("Couldn't copy", { description: "Show the key and copy it yourself" })
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      size="sm"
      title={token ? "Save your key" : "Create API key"}
      description={token ? "This is the only time it's shown. Keep it somewhere private." : "Programs use this key to read your history through /v1"}
      footer={
        token ? (
          <Button variant="primary" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        ) : (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button variant="primary" disabled={!name.trim() || !reach.length} onClick={create}>
              Create API key
            </Button>
          </>
        )
      }
    >
      {token ? (
        <div className="token-reveal">
          <SecretInput value={token} readOnly aria-label="New API key" />
          <IconButton label="Copy key" variant="default" onClick={copy}>
            <Copy size={14} aria-hidden />
          </IconButton>
        </div>
      ) : (
        <>
          <Field label="Name" error={error ?? undefined}>
            <Input autoFocus placeholder="Notes script" value={name} onChange={(e) => setName(e.currentTarget.value)} maxLength={128} />
          </Field>
          <Field label="Access">
            <Select aria-label="Access" width="full" value={scope} onValueChange={(v) => v && setScope(v)} items={scopes} />
          </Field>
          <fieldset className="reach">
            <legend className="field-label">Can see</legend>
            {prefixes.map((p) => (
              <label key={p.value} className="reach-option">
                <Checkbox
                  checked={reach.includes(p.value)}
                  onCheckedChange={(checked) => setReach((current) => (checked ? [...current, p.value] : current.filter((v) => v !== p.value)))}
                />
                {p.label}
              </label>
            ))}
          </fieldset>
        </>
      )}
    </Dialog>
  )
}

function AppearanceSection() {
  const { data, save } = useSettings()
  return (
    <Section title="Appearance">
      <Card>
        <Row
          label="Theme"
          description="Follow the system, or pick one"
          control={
            <Segmented
              aria-label="Theme"
              value={data!.settings.appearance}
              onValueChange={(value) => {
                if (!value) return
                save({ appearance: value as Appearance }).catch((cause) =>
                  toast.error("Couldn't save", { description: cause instanceof Error ? cause.message : undefined }),
                )
              }}
              items={[
                { value: "system", label: "System" },
                { value: "light", label: "Light" },
                { value: "dark", label: "Dark" },
              ]}
            />
          }
        />
      </Card>
    </Section>
  )
}

export { SettingsPage }
