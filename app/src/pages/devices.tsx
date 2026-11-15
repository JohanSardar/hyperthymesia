import { Laptop, Link2, Merge, Pencil, Split } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import { Link, useNavigate, useParams } from "react-router"
import { ago } from "../lib/time"
import { admin, type Device } from "../server/admin"
import { usePolled } from "../server/connections"
import { PageHeader, PageSidebar, SidebarEmpty, SidebarGroup } from "../shell"
import { Badge, Button, Card, CardHeader, Checkbox, Dialog, Empty, Field, Input, Page, PageIntro, Row, Skeleton, StatusDot, Tile, toast } from "../ui"
import "./devices.css"

const stateWords = {
  recording: { tone: "live", text: "Recording" },
  paused: { tone: "warning", text: "Paused" },
  off: { tone: "idle", text: "Recording off" },
  quiet: { tone: "idle", text: "Not sending" },
} as const

function label(d: Device) {
  return d.name ?? d.device
}

function day(iso: string | null) {
  return iso ? new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" }) : "—"
}

function RenameDialog({ device, open, onOpenChange, done }: { device: Device; open: boolean; onOpenChange: (o: boolean) => void; done: () => unknown }) {
  const [name, setName] = useState(device.name ?? "")
  useEffect(() => {
    if (open) setName(device.name ?? "")
  }, [open, device.name])
  const save = async () => {
    try {
      await admin.rename(device.device, name.trim() || null)
      toast.success(name.trim() ? `Now called ${name.trim()}` : "Name removed")
      onOpenChange(false)
      await done()
    } catch (cause) {
      toast.error("Couldn't rename", { description: cause instanceof Error ? cause.message : undefined })
    }
  }
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      size="sm"
      title="Rename device"
      description={`It reports itself as ${device.device}. The new name shows everywhere in the app; recorded history keeps the name it was recorded under.`}
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="primary" onClick={save}>
            Save
          </Button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault()
          void save()
        }}
      >
        <Field label="Name" description="Leave empty to use the name it reports">
          <Input autoFocus value={name} onChange={(e) => setName(e.currentTarget.value)} maxLength={120} placeholder={device.device} />
        </Field>
      </form>
    </Dialog>
  )
}

function MergeDialog({ device, all, open, onOpenChange, done }: { device: Device; all: Device[]; open: boolean; onOpenChange: (o: boolean) => void; done: () => unknown }) {
  const others = all.filter((d) => d.device !== device.device)
  const [picked, setPicked] = useState<string[]>([])
  useEffect(() => {
    if (open) setPicked([])
  }, [open])
  const merge = async () => {
    try {
      await admin.merge(picked, device.device)
      toast.success(`Merged into ${label(device)}`, { description: "Its history now shows as one device everywhere" })
      onOpenChange(false)
      await done()
    } catch (cause) {
      toast.error("Couldn't merge", { description: cause instanceof Error ? cause.message : undefined })
    }
  }
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      size="sm"
      title={`Merge into ${label(device)}`}
      description="Pick the names that are really this same Mac, for example after its network name changed. Nothing recorded changes; you can split them again later."
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!picked.length} onClick={merge}>
            Merge {picked.length || ""}
          </Button>
        </>
      }
    >
      <div className="merge-list">
        {others.length === 0 && <p className="row-description">No other devices to merge.</p>}
        {others.map((d) => (
          <label className="merge-option" key={d.device}>
            <Checkbox checked={picked.includes(d.device)} onCheckedChange={(c) => setPicked((cur) => (c ? [...cur, d.device] : cur.filter((x) => x !== d.device)))} />
            <span className="merge-option-text">
              <span className="row-label">{label(d)}</span>
              <span className="row-description">
                {d.records.toLocaleString()} records · {day(d.first)} – {day(d.last)}
              </span>
            </span>
          </label>
        ))}
      </div>
    </Dialog>
  )
}

function DeviceDetail({ device, all, refresh }: { device: Device; all: Device[]; refresh: () => unknown }) {
  const [renaming, setRenaming] = useState(false)
  const [merging, setMerging] = useState(false)
  const state = stateWords[device.recording]
  const split = async (name: string) => {
    try {
      await admin.unmerge(name)
      toast.success(`${name} is its own device again`)
      await refresh()
    } catch (cause) {
      toast.error("Couldn't split it off", { description: cause instanceof Error ? cause.message : undefined })
    }
  }
  return (
    <>
      <div className="device-head">
        <Tile size={44}>
          <span className="tile-glyph">
            <Laptop size={20} strokeWidth={1.6} />
          </span>
        </Tile>
        <div className="device-title">
          <h2 className="h1">{label(device)}</h2>
          <p className="device-meta">
            <StatusDot tone={state.tone} /> {state.text}
            <span className="sep">·</span>
            {device.last ? `Last seen ${ago(device.last)}` : "Never seen"}
          </p>
        </div>
        <div className="device-actions">
          <Button onClick={() => setRenaming(true)}>
            <Pencil size={13} aria-hidden /> Rename
          </Button>
          <Button onClick={() => setMerging(true)} disabled={all.length < 2}>
            <Merge size={13} aria-hidden /> Merge…
          </Button>
        </div>
      </div>
      <Card>
        <Row label="Reports itself as" description={<span className="mono">{device.device}</span>} />
        <Row label="History" description={`${device.records.toLocaleString()} records from ${day(device.first)} to ${day(device.last)}`} />
        <Row
          label="Recording"
          description={
            device.recording === "quiet"
              ? "Nothing arrived in the last few minutes. The Mac may be asleep, off, or not running the app"
              : device.recording === "paused"
                ? "Paused from Settings or the menu bar; it starts again by itself"
                : device.recording === "off"
                  ? "Recording is switched off in Settings › Recording"
                  : "Sending records now"
          }
          control={
            <Button variant="ghost" render={<Link to="/settings/recording" />}>
              Settings
            </Button>
          }
        />
      </Card>
      {device.names.length > 1 && (
        <Card>
          <CardHeader title={`Shown as one device: ${device.names.length} names`} />
          {device.names.map((n) => (
            <Row
              key={n.device}
              label={<span className="mono">{n.device}</span>}
              description={`${n.records.toLocaleString()} records · ${day(n.first)} – ${day(n.last)}`}
              control={
                n.device === device.device ? (
                  <Badge>Kept name</Badge>
                ) : (
                  <Button variant="ghost" size="xs" onClick={() => split(n.device)}>
                    <Split size={12} aria-hidden /> Split off
                  </Button>
                )
              }
            />
          ))}
        </Card>
      )}
      <RenameDialog device={device} open={renaming} onOpenChange={setRenaming} done={refresh} />
      <MergeDialog device={device} all={all} open={merging} onOpenChange={setMerging} done={refresh} />
    </>
  )
}

function DevicesPage() {
  const { device: param } = useParams()
  const navigate = useNavigate()
  const devices = usePolled(admin.devices, 15_000)
  const list = devices.data?.devices
  const selected = useMemo(() => list?.find((d) => d.device === param || d.names.some((n) => n.device === param)) ?? list?.[0], [list, param])

  return (
    <>
      <PageHeader crumbs={selected ? [{ label: "Devices", to: "/devices" }, { label: label(selected) }] : undefined} title="Devices" actions={<Button render={<Link to="/settings/server#server.pair" />}><Link2 size={13} aria-hidden /> Pair a device</Button>} />
      <PageSidebar>
        <SidebarGroup label="Sending data">
          {!list && <Skeleton width="80%" />}
          {list?.length === 0 && <SidebarEmpty>No devices yet</SidebarEmpty>}
          {list?.map((d) => (
            <button key={d.device} type="button" className="sidebar-row" data-active={d.device === selected?.device ? "" : undefined} aria-current={d.device === selected?.device ? "page" : undefined} onClick={() => navigate(`/devices/${encodeURIComponent(d.device)}`)}>
              <StatusDot tone={stateWords[d.recording].tone} />
              <span className="sidebar-row-label">{label(d)}</span>
              <span className="sidebar-row-meta">{d.last ? ago(d.last) : ""}</span>
            </button>
          ))}
        </SidebarGroup>
      </PageSidebar>
      <Page width="settings">
        {!list ? (
          devices.error ? (
            <Empty title="Devices didn't load" description={devices.error.message} />
          ) : (
            <Skeleton width="100%" height={160} />
          )
        ) : !selected ? (
          <>
            <PageIntro title="Devices" description="Each Mac sending data to this server" />
            <Empty title="No devices yet" description="Pair a Mac with a one-time link and it shows up here">
              <Button render={<Link to="/settings/server#server.pair" />}>Pair a device</Button>
            </Empty>
          </>
        ) : (
          <DeviceDetail device={selected} all={list} refresh={devices.refresh} />
        )}
      </Page>
    </>
  )
}

export { DevicesPage }
