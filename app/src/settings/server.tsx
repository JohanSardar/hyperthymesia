import { ArrowRight, Copy, Link2 } from "lucide-react"
import { useEffect, useState, type ReactNode } from "react"
import { Link } from "react-router"
import { admin } from "../server/admin"
import { usePolled } from "../server/connections"
import { useSettings } from "../server/settings"
import { Button, Card, Field, IconButton, Input, Segmented, Skeleton, StatusDot, toast } from "../ui"
import { ApiKeysSection } from "./keys"
import { dateTime, errorText, RestoreDefaults, Setting, useSave } from "./parts"

async function copy(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text)
    toast.success(`Copied ${what}`)
  } catch {
    toast.error("Couldn't copy")
  }
}

function uptime(since: string | null) {
  if (!since) return "unknown"
  const minutes = Math.round((Date.now() - new Date(since).getTime()) / 60_000)
  if (minutes < 60) return `${minutes} min`
  const hours = Math.round(minutes / 60)
  return hours < 48 ? `${hours} h` : `${Math.round(hours / 24)} days`
}

function PairDevice() {
  const { data } = useSettings()
  const remote = data!.settings.remote_url ?? null
  const [name, setName] = useState("")
  const [reach, setReach] = useState<"local" | "remote">("local")
  const [link, setLink] = useState<{ link: string; expires_at: string } | null>(null)
  const [busy, setBusy] = useState(false)
  const make = async () => {
    setBusy(true)
    try {
      setLink(await admin.pair(name.trim(), reach))
    } catch (cause) {
      toast.error("Couldn't make a link", { description: errorText(cause) })
    } finally {
      setBusy(false)
    }
  }
  return (
    <Setting id="server.pair">
      <form
        className="pair-form"
        onSubmit={(e) => {
          e.preventDefault()
          void make()
        }}
      >
        <Field label="Device name">
          <Input placeholder="Work MacBook" value={name} onChange={(e) => setName(e.currentTarget.value)} maxLength={120} />
        </Field>
        <Field label="It reaches this server" description={reach === "remote" ? remote ?? "Set a remote address below first" : "On this same computer"}>
          <Segmented
            aria-label="How the device reaches this server"
            value={reach}
            onValueChange={(v) => v && setReach(v as "local" | "remote")}
            items={[
              { value: "local", label: "This computer" },
              { value: "remote", label: "Remote address" },
            ]}
          />
        </Field>
        <div>
          <Button type="submit" variant="primary" disabled={!name.trim() || busy || (reach === "remote" && !remote)}>
            <Link2 size={13} aria-hidden /> Make a one-time link
          </Button>
        </div>
      </form>
      {link && (
        <div className="pair-link">
          <span className="mono path">{link.link}</span>
          <IconButton label="Copy link" variant="default" onClick={() => copy(link.link, "link")}>
            <Copy size={14} aria-hidden />
          </IconButton>
          <span className="row-description">
            Open it on the Mac to pair, or paste it into that Mac's Hyperthymesia setup. It works once, until {new Date(link.expires_at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}.
          </span>
        </div>
      )}
    </Setting>
  )
}

function RemoteAccess({ tailscale, localUrl }: { tailscale: string | null | undefined; localUrl: string | undefined }) {
  const port = localUrl ? new URL(localUrl).port || "80" : "7377"
  const { data } = useSettings()
  const save = useSave()
  const saved = data!.settings.remote_url ?? ""
  const [value, setValue] = useState(saved)
  useEffect(() => setValue(saved), [saved])
  return (
    <Setting
      id="server.remote"
      description={
        <>
          This server only listens on this computer. To reach it from elsewhere, put a private HTTPS address in front of it, such as{" "}
          <code className="inline-code">tailscale serve --bg {port}</code>
          {tailscale ? " (Tailscale is installed here)" : ""}, and enter that address here. Pairing links can then use it.
        </>
      }
    >
      <form
        className="never-add"
        onSubmit={(e) => {
          e.preventDefault()
          void save({ remote_url: value.trim() || null }, value.trim() ? "Remote address saved" : "Remote address removed")
        }}
      >
        <Input mono placeholder="https://my-mac.tailnet.ts.net" value={value} onChange={(e) => setValue(e.currentTarget.value)} aria-label="Remote address" />
        <Button type="submit" disabled={value.trim() === saved}>
          Save
        </Button>
      </form>
    </Setting>
  )
}

function ServerPage({ actionsSlot }: { actionsSlot: (node: ReactNode) => ReactNode }) {
  const { data } = useSettings()
  const save = useSave()
  const server = usePolled(admin.server, 10_000)
  const devices = usePolled(admin.devices, 30_000)
  const s = server.data
  const pending = s?.jobs.filter((j) => j.state !== "failed").reduce((n, j) => n + j.count, 0) ?? 0
  return (
    <>
      {actionsSlot(
        <RestoreDefaults
          changes={data!.settings.remote_url ? ["no remote address"] : []}
          keeps="Paired devices and API keys are not touched."
          onRestore={() => save({ remote_url: null }, "Defaults restored")}
        />,
      )}
      <Card>
        <Setting
          id="server.status"
          leading={<StatusDot tone={server.error ? "error" : s ? "ok" : "idle"} />}
          description={
            server.error
              ? "Not answering"
              : s
                ? `Running for ${uptime(s.started_at)} · version ${s.version} · ${pending ? `${pending} processing steps waiting` : "nothing waiting"}${s.running_passes ? " · writing up now" : ""}${s.last_record_at ? ` · last record ${dateTime(s.last_record_at)}` : ""}`
                : "Checking…"
          }
        />
        <Setting
          id="server.address"
          description={s ? <span className="mono">{s.local_url}</span> : <Skeleton width={160} />}
          control={
            s && (
              <IconButton label="Copy address" onClick={() => copy(s.local_url, "address")}>
                <Copy size={14} aria-hidden />
              </IconButton>
            )
          }
        />
        <RemoteAccess tailscale={s?.remote.tailscale} localUrl={s?.local_url} />
      </Card>
      <Card>
        <Setting
          id="server.devices"
          description={devices.data ? `${devices.data.devices.length} ${devices.data.devices.length === 1 ? "device" : "devices"}; rename one or merge duplicates on the Devices page` : undefined}
          control={
            <Button render={<Link to="/devices" />}>
              Open <ArrowRight size={12} aria-hidden />
            </Button>
          }
        />
        <PairDevice />
      </Card>
      <Setting id="server.keys" />
      <ApiKeysSection />
    </>
  )
}

export { ServerPage }
