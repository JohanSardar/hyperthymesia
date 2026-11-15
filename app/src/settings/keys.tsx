import { Copy, Ellipsis, KeyRound, Plus, Trash2 } from "lucide-react"
import { useEffect, useState } from "react"
import { api, type ApiKey } from "../server/api"
import { useSettings } from "../server/settings"
import {
  Badge,
  Button,
  Card,
  CardHeader,
  Checkbox,
  Confirm,
  Dialog,
  Field,
  IconButton,
  Input,
  Menu,
  MenuContent,
  MenuItem,
  MenuTrigger,
  Row,
  SecretInput,
  Select,
  buttonAttrs,
  toast,
} from "../ui"
import "../pages/settings.css"

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
    <>
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
    </>
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

export { ApiKeysSection }
