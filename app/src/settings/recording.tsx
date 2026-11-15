import { Check, Eye, MonitorSmartphone, Plus, X } from "lucide-react"
import { useState } from "react"
import { usePolled } from "../server/connections"
import { admin, type NeverRecord, type Recording } from "../server/admin"
import { api } from "../server/api"
import { useSettings } from "../server/settings"
import { Badge, Button, Card, Input, Menu, MenuContent, MenuItem, MenuTrigger, Segmented, Spinner, StatusDot, Switch, Tile, buttonAttrs, toast } from "../ui"
import { errorText, RestoreDefaults, Setting, useSave } from "./parts"
import "../first-run/first-run.css"

function pauseWords(until: string) {
  const end = new Date(until)
  const sameDay = end.toDateString() === new Date().toDateString()
  return `Paused until ${end.toLocaleString(undefined, sameDay ? { hour: "numeric", minute: "2-digit" } : { weekday: "short", hour: "numeric", minute: "2-digit" })}`
}

function tomorrowMorning() {
  const d = new Date()
  d.setDate(d.getDate() + 1)
  d.setHours(8, 0, 0, 0)
  return d
}

function PauseRow({ recording, set }: { recording: Recording | undefined; set: (r: Recording) => void }) {
  const until = recording?.paused_until ?? null
  const pause = async (end: Date | null) => {
    try {
      set(await admin.pause(end ? end.toISOString() : null))
      toast.success(end ? "Recording paused" : "Recording resumed", { description: end ? "Paired Macs stop within a few seconds" : undefined })
    } catch (cause) {
      toast.error("Couldn't change the pause", { description: errorText(cause) })
    }
  }
  return (
    <Setting
      id="recording.pause"
      description={until ? `${pauseWords(until)}. It starts again by itself.` : undefined}
      control={
        until ? (
          <Button onClick={() => pause(null)}>Resume now</Button>
        ) : (
          <Menu
            onAction={(value) => {
              const now = Date.now()
              const end = value === "tomorrow" ? tomorrowMorning() : new Date(now + Number(value) * 60_000)
              void pause(end)
            }}
          >
            <MenuTrigger {...buttonAttrs("default", "sm")} disabled={!recording}>
              Pause…
            </MenuTrigger>
            <MenuContent align="end">
              <MenuItem value="15">For 15 minutes</MenuItem>
              <MenuItem value="60">For an hour</MenuItem>
              <MenuItem value="240">For 4 hours</MenuItem>
              <MenuItem value="tomorrow">Until tomorrow morning</MenuItem>
            </MenuContent>
          </Menu>
        )
      }
    />
  )
}

/** Apps are matched by bundle ID; a name alone can't pick out an app. */
function AppList({ rules, save }: { rules: NeverRecord; save: (next: NeverRecord) => Promise<void> }) {
  const [id, setId] = useState("")
  const [name, setName] = useState("")
  const add = async () => {
    const bundle = id.trim()
    if (!bundle) return
    await save({ ...rules, apps: [...rules.apps, { bundle_id: bundle, name: name.trim() || bundle }] })
    setId("")
    setName("")
  }
  return (
    <div className="never-list">
      {rules.apps.length === 0 && <p className="row-description">No apps yet. You can also add the app in front from the menu bar.</p>}
      {rules.apps.map((app) => (
        <span className="never-item" key={app.bundle_id}>
          <span className="never-item-text">
            <span className="row-label">{app.name}</span>
            <span className="mono-line">{app.bundle_id}</span>
          </span>
          <Button variant="ghost" size="xs" aria-label={`Record ${app.name} again`} onClick={() => save({ ...rules, apps: rules.apps.filter((a) => a.bundle_id !== app.bundle_id) })}>
            <X size={12} aria-hidden /> Remove
          </Button>
        </span>
      ))}
      <form
        className="never-add"
        onSubmit={(e) => {
          e.preventDefault()
          void add()
        }}
      >
        <Input placeholder="App name, e.g. 1Password" value={name} onChange={(e) => setName(e.currentTarget.value)} aria-label="App name" />
        <Input mono placeholder="Bundle ID, e.g. com.1password.1password" value={id} onChange={(e) => setId(e.currentTarget.value)} aria-label="App bundle ID" />
        <Button type="submit" disabled={!id.trim()}>
          <Plus size={13} aria-hidden /> Add
        </Button>
      </form>
    </div>
  )
}

function SiteList({ rules, save }: { rules: NeverRecord; save: (next: NeverRecord) => Promise<void> }) {
  const [site, setSite] = useState("")
  return (
    <div className="never-list">
      {rules.sites.length === 0 && <p className="row-description">No sites yet. Paste an address; only its site is kept.</p>}
      {rules.sites.map((s) => (
        <span className="never-item" key={s}>
          <span className="never-item-text">
            <span className="row-label mono">{s}</span>
            <span className="row-description">and every *.{s} page</span>
          </span>
          <Button variant="ghost" size="xs" aria-label={`Record ${s} again`} onClick={() => save({ ...rules, sites: rules.sites.filter((x) => x !== s) })}>
            <X size={12} aria-hidden /> Remove
          </Button>
        </span>
      ))}
      <form
        className="never-add"
        onSubmit={async (e) => {
          e.preventDefault()
          if (!site.trim()) return
          await save({ ...rules, sites: [...rules.sites, site.trim()] })
          setSite("")
        }}
      >
        <Input placeholder="bank.example or a full address" value={site} onChange={(e) => setSite(e.currentTarget.value)} aria-label="Site" />
        <Button type="submit" disabled={!site.trim()}>
          <Plus size={13} aria-hidden /> Add
        </Button>
      </form>
    </div>
  )
}

const permissionCopy = {
  accessibility: { name: "Accessibility", icon: Eye, purpose: "Which app and window are in front, and the text apps make available. Counts keys and clicks; never records which." },
  screen: { name: "Screen Recording", icon: MonitorSmartphone, purpose: "Reads the text on screen. The picture is dropped as soon as its text is read." },
} as const

function Permissions() {
  const status = usePolled(api.setupStatus, 2000)
  const permissions = status.data?.permissions
  const guiding = status.data?.guide?.status === "running"
  const ask = async () => {
    try {
      await api.requestPermissions()
      toast("The guide is opening on the Mac", { description: "It shows one switch at a time beside System Settings" })
    } catch (cause) {
      toast.error("The guide didn't open", { description: errorText(cause) })
    }
  }
  return (
    <Setting
      id="recording.permissions"
      description={
        status.data && !permissions
          ? status.data.app.installed
            ? "The Mac app isn't running, so permissions can't be read. Open Hyperthymesia from Applications."
            : "The Mac app isn't installed on this computer."
          : undefined
      }
    >
      <div className="permission-grid settings-permissions">
        {(["accessibility", "screen"] as const).map((id) => {
          const copy = permissionCopy[id]
          const Icon = copy.icon
          const on = Boolean(permissions?.[id])
          return (
            <div key={id} className="permission-card card" data-on={on ? "" : undefined}>
              <div className="permission-card-top">
                <Tile size={36}>
                  <span className="tile-glyph">
                    <Icon size={16} strokeWidth={1.75} />
                  </span>
                </Tile>
                {!status.data ? (
                  <Spinner size={12} />
                ) : !permissions ? (
                  <Badge>Unknown</Badge>
                ) : on ? (
                  <Badge tone="success">
                    <Check size={11} strokeWidth={3} aria-hidden /> On
                  </Badge>
                ) : (
                  <Badge tone="warning">Off</Badge>
                )}
              </div>
              <span className="row-label">{copy.name}</span>
              <span className="row-description">{copy.purpose}</span>
              {permissions && !on && (
                <div className="permission-foot">
                  {guiding ? (
                    <span className="permission-live">
                      <StatusDot tone="live" /> Watching for the change
                    </span>
                  ) : (
                    <Button onClick={ask}>Turn on</Button>
                  )}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </Setting>
  )
}

function RecordingPage({ actionsSlot }: { actionsSlot: (node: React.ReactNode) => React.ReactNode }) {
  const { data } = useSettings()
  const save = useSave()
  const recording = usePolled(admin.recording, 5000)
  const on = data!.settings.mac_recording
  const profile = data!.settings.recording_profile ?? "balanced"
  const rules = recording.data?.never_record ?? { apps: [], sites: [] }
  const saveRules = async (next: NeverRecord) => {
    try {
      recording.setData(await admin.neverRecord(next))
      toast.success("Saved", { description: "Paired Macs pick it up within a few seconds" })
    } catch (cause) {
      toast.error("Couldn't save the list", { description: errorText(cause) })
    }
  }
  return (
    <>
      {actionsSlot(
        <RestoreDefaults
          changes={[on ? "" : "recording on", profile === "balanced" ? "" : "pace Balanced"].filter(Boolean)}
          keeps="Your never-record apps and sites are kept; remove entries one by one if you want them recorded again."
          onRestore={() => save({ mac_recording: true, recording_profile: "balanced" }, "Defaults restored")}
        />,
      )}
      <Card>
        <Setting
          id="recording.on"
          control={
            <>
              <StatusDot tone={on ? (recording.data?.paused_until ? "warning" : "live") : "idle"} />
              <Switch aria-label="Record on this Mac" checked={on} onCheckedChange={(next) => save({ mac_recording: next }, next ? "Recording on" : "Recording off")} />
            </>
          }
        />
        <PauseRow recording={recording.data} set={(r) => recording.setData(r)} />
        <Setting
          id="recording.pace"
          control={
            <Segmented
              aria-label="Recording pace"
              value={profile}
              onValueChange={(value) => value && save({ recording_profile: value as "balanced" | "saver" }, value === "saver" ? "Saver on" : "Balanced on")}
              items={[
                { value: "balanced", label: "Balanced" },
                { value: "saver", label: "Saver" },
              ]}
            />
          }
        />
      </Card>
      <Card>
        <Setting id="recording.apps">
          <AppList rules={rules} save={saveRules} />
        </Setting>
        <Setting id="recording.sites">
          <SiteList rules={rules} save={saveRules} />
        </Setting>
        <Setting id="recording.private" />
      </Card>
      <Card>
        <Permissions />
      </Card>
    </>
  )
}

export { RecordingPage }
