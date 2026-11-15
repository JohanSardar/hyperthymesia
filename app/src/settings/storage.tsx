import { Copy, Download, RotateCcw, Trash2, TriangleAlert } from "lucide-react"
import { useEffect, useState, type ReactNode } from "react"
import { admin, waitForServer, type Backup, type ErasePlan, type LogEntry, type Removed } from "../server/admin"
import { usePolled } from "../server/connections"
import { useSettings } from "../server/settings"
import { Badge, Button, Callout, Card, Confirm, ConfirmTrigger, Dialog, Empty, IconButton, Input, Select, Skeleton, Spinner, buttonAttrs, toast } from "../ui"
import { keepTypes } from "./catalog"
import { bytes, count, dateTime, errorText, RestoreDefaults, Setting, useSave } from "./parts"

const typeNames: Record<string, string> = {
  screen_text: "Screen text",
  screen_pictures: "Screen pictures",
  apps: "Apps and windows",
  activity: "Activity counts",
  connections: "Connection records",
  system: "Recorder health and clock",
  write_ups: "Write-ups",
}

const keepOptions = [
  { value: "forever", label: "Forever" },
  { value: "1825", label: "5 years" },
  { value: "365", label: "1 year" },
  { value: "180", label: "6 months" },
  { value: "90", label: "90 days" },
  { value: "30", label: "30 days" },
  { value: "7", label: "7 days" },
]

/** `datetime-local` value for a Date, in local time. */
function localInput(d: Date) {
  const pad = (n: number) => String(n).padStart(2, "0")
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}
function iso(local: string) {
  const d = new Date(local)
  return Number.isNaN(d.getTime()) ? null : d.toISOString()
}

function Usage() {
  const storage = usePolled(() => admin.storage())
  const [fresh, setFresh] = useState(false)
  const data = storage.data
  const total = data ? data.types.reduce((sum, t) => sum + t.bytes, 0) : 0
  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text)
      toast.success("Copied path")
    } catch {
      toast.error("Couldn't copy")
    }
  }
  return (
    <>
      <Setting
        id="storage.usage"
        description={
          data
            ? `${bytes(data.databases)} of databases on disk${data.disk_free != null ? `, ${bytes(data.disk_free)} free on this disk` : ""}. Measured ${new Date(data.measured_at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}`
            : "Measuring… this reads every record once and can take a few seconds"
        }
        control={
          <Button
            variant="ghost"
            disabled={fresh}
            onClick={async () => {
              setFresh(true)
              try {
                storage.setData(await admin.storage(true))
              } finally {
                setFresh(false)
              }
            }}
          >
            {fresh ? "Measuring…" : "Measure again"}
          </Button>
        }
      >
        {!data ? (
          <Skeleton width="100%" height={96} />
        ) : (
          <div className="usage">
            {data.types.map((t) => (
              <div className="usage-row" key={t.type}>
                <span className="usage-name">{typeNames[t.type] ?? t.type}</span>
                <span className="usage-bar" aria-hidden>
                  <span style={{ width: `${total ? Math.max(1, (t.bytes / total) * 100) : 0}%` }} />
                </span>
                <span className="usage-size">{bytes(t.bytes)}</span>
                <span className="usage-meta">
                  {count(t.records, t.type === "write_ups" ? "write-up" : "record")}
                  {t.first && ` · since ${new Date(t.first).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" })}`}
                  {t.extra.files ? ` · ${count(t.extra.files, "file")}` : ""}
                </span>
              </div>
            ))}
          </div>
        )}
      </Setting>
      <Setting
        id="storage.folder"
        description={data ? <span className="mono path">{data.state_dir}</span> : undefined}
        control={
          data && (
            <IconButton label="Copy path" onClick={() => copy(data.state_dir)}>
              <Copy size={14} aria-hidden />
            </IconButton>
          )
        }
      />
    </>
  )
}

function KeepRows() {
  const { data } = useSettings()
  const save = useSave()
  const retention = data!.settings.retention ?? {}
  const [asking, setAsking] = useState<{ type: string; days: number } | null>(null)
  const set = (type: string, value: string | null) => {
    if (!value) return
    if (value === "forever") {
      const { [type]: _gone, ...rest } = retention
      void save({ retention: rest }, "Kept forever")
      return
    }
    setAsking({ type, days: Number(value) })
  }
  const asked = asking ? keepTypes.find((t) => t.type === asking.type) : null
  return (
    <>
      {keepTypes.map((t) => {
        const days = retention[t.type]
        return (
          <Setting
            key={t.type}
            id={`storage.keep.${t.type}`}
            control={
              <Select
                aria-label={`Keep ${t.label} for`}
                width={144}
                value={days ? String(days) : "forever"}
                onValueChange={(v) => set(t.type, v)}
                items={days && !keepOptions.some((o) => o.value === String(days)) ? [...keepOptions, { value: String(days), label: `${days} days` }] : keepOptions}
              />
            }
          />
        )
      })}
      <Confirm
        open={asking !== null}
        onOpenChange={(open) => !open && setAsking(null)}
        title={`Keep ${asked?.label ?? ""} for ${keepOptions.find((o) => o.value === String(asking?.days))?.label.toLowerCase() ?? `${asking?.days} days`}?`}
        description={`At the next weekly clean-up, ${asked?.label ?? "records"} older than that are deleted for good, after a backup is taken. Write-ups of those days are kept. You can run the clean-up now from the row below.`}
        confirmLabel="Set the limit"
        destructive
        onConfirm={() => {
          if (asking) void save({ retention: { ...retention, [asking.type]: asking.days } }, "Limit set")
          setAsking(null)
        }}
      />
    </>
  )
}

function CleanUp({ onDone }: { onDone: () => void }) {
  const { data } = useSettings()
  const storage = usePolled(() => admin.storage())
  const [busy, setBusy] = useState(false)
  const limits = Object.keys(data!.settings.retention ?? {}).length
  const last = storage.data?.last_clean_up
  const run = async () => {
    setBusy(true)
    try {
      const result = await admin.cleanUp()
      if (result.nothing_set) toast("No limits are set, so nothing was deleted")
      else if (result.action === "clean_up_skipped") toast.error("Nothing was deleted", { description: result.error })
      else if (result.action === "cleaned_up") toast.success("Clean-up done", { description: `A backup was taken first: ${result.backup}` })
      else toast.success("Nothing was old enough to delete")
      onDone()
      await storage.refresh()
    } catch (cause) {
      toast.error("Clean-up failed", { description: errorText(cause) })
    } finally {
      setBusy(false)
    }
  }
  return (
    <Setting
      id="storage.cleanup"
      description={
        limits === 0
          ? "Off: every type is kept forever, so nothing is deleted automatically"
          : `${count(limits, "limit")} set. ${last ? `Last run ${dateTime(last.at)}.` : "Not run yet."} It runs once a week and takes a backup first; if the disk is too full for one, it deletes nothing`
      }
      control={
        <Confirm
          title="Run the clean-up now?"
          description="Takes a backup, then deletes records older than their keep-for limit. This can take a few minutes."
          confirmLabel="Run clean-up"
          destructive
          trigger={
            <ConfirmTrigger {...buttonAttrs("default", "sm")} disabled={busy || limits === 0}>
              {busy ? "Running…" : "Run now"}
            </ConfirmTrigger>
          }
          onConfirm={run}
        />
      }
    />
  )
}

function Backups({ version }: { version: number }) {
  const backups = usePolled(admin.backups)
  const [busy, setBusy] = useState<string | null>(null)
  useEffect(() => {
    void backups.refresh()
  }, [version])
  const backupNow = async () => {
    setBusy("new")
    try {
      const made = await admin.backupNow()
      toast.success("Backup done", { description: made.path })
      await backups.refresh()
    } catch (cause) {
      toast.error("No backup was made", { description: errorText(cause) })
    } finally {
      setBusy(null)
    }
  }
  const restore = async (b: Backup) => {
    setBusy(b.name)
    try {
      await admin.restore(b.name)
      toast("Restoring…", { description: "The server restarts with the backup. This page reloads when it's back." })
      const back = await waitForServer(600_000)
      if (back) window.location.reload()
      else toast.error("The server hasn't come back yet", { description: "Check Advanced › Server log" })
    } catch (cause) {
      toast.error("Couldn't restore", { description: errorText(cause) })
      setBusy(null)
    }
  }
  const remove = async (b: Backup) => {
    try {
      await admin.deleteBackup(b.name)
      toast.success("Backup deleted")
      await backups.refresh()
    } catch (cause) {
      toast.error("Couldn't delete the backup", { description: errorText(cause) })
    }
  }
  const reason = (r: string) => ({ manual: "Made by you", "before-delete": "Taken before a delete", "before-clean-up": "Taken before a clean-up" })[r] ?? r
  const restored = backups.data?.restore
  return (
    <>
      <Setting
        id="storage.backup"
        description={backups.data ? <>Saved in <span className="mono path">{backups.data.dir}</span>. Keys and server settings are left out on purpose</> : undefined}
        control={
          <Button onClick={() => void backupNow()} disabled={busy !== null}>
            {busy === "new" ? (
              <>
                <Spinner size={12} /> Backing up…
              </>
            ) : (
              "Back up now"
            )}
          </Button>
        }
      />
      <Setting id="storage.backups">
        {restored && (
          <Callout tone={restored.ok ? "primary" : "danger"} title={restored.ok ? `Restored ${restored.backup}` : `Restoring ${restored.backup} failed`}>
            {restored.ok ? <>Your state from before is kept at <span className="mono path">{restored.previous_state}</span>.</> : restored.error}
          </Callout>
        )}
        {!backups.data ? (
          <Skeleton width="100%" height={48} />
        ) : backups.data.backups.length === 0 ? (
          <p className="row-description">No backups yet.</p>
        ) : (
          <div className="backup-list">
            {backups.data.backups.map((b) => (
              <div className="backup-row" key={b.name}>
                <span className="backup-text">
                  <span className="row-label">{b.at ? dateTime(b.at) : b.name}</span>
                  <span className="row-description">
                    {reason(b.reason)} · {bytes(b.bytes)}
                    {!b.complete && " · unfinished, can't be restored"}
                  </span>
                </span>
                <span className="backup-actions">
                  <Confirm
                    title="Restore this backup?"
                    description={`The server restarts with your history as it was on ${b.at ? dateTime(b.at) : b.name}. The current state is set aside beside the data folder, not deleted, so anything recorded since stays there. API keys, paired devices and connections' sign-ins stay as they are now.`}
                    confirmLabel="Restore and restart"
                    trigger={
                      <ConfirmTrigger {...buttonAttrs("ghost", "sm")} disabled={!b.complete || busy !== null}>
                        {busy === b.name ? <Spinner size={12} /> : <RotateCcw size={13} aria-hidden />} Restore
                      </ConfirmTrigger>
                    }
                    onConfirm={() => restore(b)}
                  />
                  <Confirm
                    title="Delete this backup?"
                    description="Its copy of your history is removed from disk for good. Deletes it backed up can't be undone after this."
                    confirmLabel="Delete backup"
                    destructive
                    trigger={
                      <ConfirmTrigger {...buttonAttrs("ghost", "sm", undefined, true)} aria-label="Delete backup" disabled={busy !== null}>
                        <Trash2 size={13} aria-hidden />
                      </ConfirmTrigger>
                    }
                    onConfirm={() => remove(b)}
                  />
                </span>
              </div>
            ))}
          </div>
        )}
        {backups.data && backups.data.set_aside.length > 0 && (
          <p className="row-description">
            Set aside by a restore: {backups.data.set_aside.map((s) => <span key={s.name} className="mono path">{s.path} </span>)}. Delete these in Finder when you no longer need them.
          </p>
        )}
      </Setting>
    </>
  )
}

function RangePicker({ start, end, setStart, setEnd, presets = true }: { start: string; end: string; setStart: (v: string) => void; setEnd: (v: string) => void; presets?: boolean }) {
  const preset = (minutes: number | "today" | "yesterday") => {
    const now = new Date()
    if (minutes === "today" || minutes === "yesterday") {
      const from = new Date(now)
      from.setHours(0, 0, 0, 0)
      if (minutes === "yesterday") from.setDate(from.getDate() - 1)
      const to = minutes === "today" ? now : new Date(from.getTime() + 86_400_000)
      setStart(localInput(from))
      setEnd(localInput(to))
      return
    }
    setStart(localInput(new Date(now.getTime() - minutes * 60_000)))
    setEnd(localInput(now))
  }
  return (
    <div className="range-picker">
      <label className="range-field">
        <span className="field-label">From</span>
        <Input type="datetime-local" value={start} onChange={(e) => setStart(e.currentTarget.value)} aria-label="From" />
      </label>
      <label className="range-field">
        <span className="field-label">To</span>
        <Input type="datetime-local" value={end} onChange={(e) => setEnd(e.currentTarget.value)} aria-label="To" />
      </label>
      {presets && (
        <div className="range-presets">
          <Button variant="ghost" size="xs" onClick={() => preset(15)}>Last 15 min</Button>
          <Button variant="ghost" size="xs" onClick={() => preset(60)}>Last hour</Button>
          <Button variant="ghost" size="xs" onClick={() => preset("today")}>Today</Button>
          <Button variant="ghost" size="xs" onClick={() => preset("yesterday")}>Yesterday</Button>
        </div>
      )}
    </div>
  )
}

function Export() {
  const [start, setStart] = useState(() => localInput(new Date(Date.now() - 86_400_000)))
  const [end, setEnd] = useState(() => localInput(new Date()))
  const from = iso(start)
  const to = iso(end)
  const valid = from && to && to > from
  return (
    <Setting id="storage.export">
      <RangePicker start={start} end={end} setStart={setStart} setEnd={setEnd} />
      <div>
        <a className="btn" data-variant="default" data-size="sm" href={valid ? admin.exportUrl(from!, to!) : undefined} aria-disabled={!valid} download>
          <Download size={13} aria-hidden /> Download
        </a>
      </div>
    </Setting>
  )
}

function removedLines(r: Removed) {
  return [
    r.screen_text && count(r.screen_text, "screen text record"),
    r.screen_pictures && count(r.screen_pictures, "screen picture"),
    r.text_read_from_pictures && count(r.text_read_from_pictures, "text read from a picture", "texts read from pictures"),
    r.apps_and_windows && count(r.apps_and_windows, "app or window record", "app and window records"),
    r.activity && count(r.activity, "activity count"),
    r.connection_records && count(r.connection_records, "connection record"),
    r.files && `${count(r.files, "stored file")} (${bytes(r.file_bytes)})`,
    r.write_ups && count(r.write_ups, "write-up"),
    r.lookups && count(r.lookups, "saved lookup"),
  ].filter(Boolean) as string[]
}

function DeleteRange({ onDone }: { onDone: () => void }) {
  const [start, setStart] = useState(() => localInput(new Date(Date.now() - 3_600_000)))
  const [end, setEnd] = useState(() => localInput(new Date()))
  const [plan, setPlan] = useState<ErasePlan | null>(null)
  const [checking, setChecking] = useState(false)
  const [running, setRunning] = useState(false)
  const [result, setResult] = useState<LogEntry | null>(null)
  const from = iso(start)
  const to = iso(end)
  const valid = Boolean(from && to && to > from)

  const review = async () => {
    if (!valid) return
    setChecking(true)
    try {
      const p = await admin.erasePlan(from!, to!)
      setPlan(p)
    } catch (cause) {
      toast.error("Couldn't check the range", { description: errorText(cause) })
    } finally {
      setChecking(false)
    }
  }
  const run = async () => {
    setRunning(true)
    try {
      const done = await admin.erase(from!, to!)
      setResult(done)
      setPlan(null)
      toast.success("Deleted", { description: `Backup ${done.backup} was taken first` })
      onDone()
    } catch (cause) {
      // The server says whether anything went before the failure.
      toast.error("The delete didn't finish", { description: errorText(cause) })
    } finally {
      setRunning(false)
    }
  }
  const p = plan?.plan
  const wider = p && p.first && p.last && (new Date(p.first) < new Date(from!) || new Date(p.last) > new Date(to!))
  const tables = plan?.agent_tables.filter((t) => t.rows > 0) ?? []
  return (
    <Setting id="storage.delete">
      <RangePicker start={start} end={end} setStart={setStart} setEnd={setEnd} />
      <div>
        <Button variant="destructive" disabled={!valid || checking} onClick={() => void review()}>
          {checking ? (
            <>
              <Spinner size={12} /> Checking…
            </>
          ) : (
            <>
              <Trash2 size={13} aria-hidden /> Review what goes
            </>
          )}
        </Button>
      </div>
      {result && (
        <Callout tone="primary" title={`Deleted ${dateTime(result.start)} – ${dateTime(result.end)}`}>
          {result.removed && removedLines(result.removed).join(", ")}. The backup <span className="mono">{result.backup}</span> holds it if you change your mind; restore it from Backups above.
        </Callout>
      )}
      <Dialog
        open={plan !== null}
        onOpenChange={(open) => !open && !running && setPlan(null)}
        size="sm"
        title="Delete this time for good?"
        description={`${dateTime(from)} – ${dateTime(to)}`}
        footer={
          <>
            <Button variant="ghost" disabled={running} onClick={() => setPlan(null)}>
              Cancel
            </Button>
            <Button variant="destructive" disabled={running || !p || (p.records === 0 && p.write_ups === 0 && p.lookups === 0)} onClick={() => void run()}>
              {running ? (
                <>
                  <Spinner size={12} /> Backing up, then deleting…
                </>
              ) : (
                "Back up and delete"
              )}
            </Button>
          </>
        }
      >
        {p && (
          <div className="erase-plan">
            {p.records === 0 && p.write_ups === 0 && p.lookups === 0 ? (
              <p className="row-description">Nothing is stored in this range.</p>
            ) : (
              <>
                <p className="row-label">This removes:</p>
                <ul className="erase-list">
                  {removedLines(p).map((line) => (
                    <li key={line}>{line}</li>
                  ))}
                  {p.write_ups > 0 && <li>The write-ups of {count(p.windows, "four-hour window")} it touches, and any that used them as context. Parts of those windows outside this range are written up again on the agent's next runs</li>}
                </ul>
                {wider && (
                  <Callout tone="warning" icon={<TriangleAlert size={14} />} title="Some records reach past the range">
                    A record that started before the range or ran past it is removed whole, so some of what goes reaches from {dateTime(p.first)} to {dateTime(p.last)}. Other records outside the range stay.
                  </Callout>
                )}
                <p className="row-description">
                  It goes from the day page, the timeline, search and everything the agent can read, and records from this time are refused if a Mac still has some queued. First a full backup is taken
                  {plan.disk_free != null && ` (${bytes(plan.disk_free)} free)`}; if it can't be, nothing is deleted. The deletion log records what went, never the content.
                </p>
                {tables.length > 0 && (
                  <p className="row-description">
                    The agent's own tables are emptied too ({tables.map((t) => `${t.name}: ${t.rows} rows`).join(", ")}). They have no times, so they may hold something from this range. The agent fills them again at its next setup.
                  </p>
                )}
              </>
            )}
          </div>
        )}
      </Dialog>
    </Setting>
  )
}

function DeletionLog({ version }: { version: number }) {
  const log = usePolled(admin.eraseLog)
  useEffect(() => {
    void log.refresh()
  }, [version])
  const words = (e: LogEntry): ReactNode => {
    switch (e.action) {
      case "deleted_range":
        return (
          <>
            Deleted {dateTime(e.start)} – {dateTime(e.end)}: {e.removed ? removedLines(e.removed).join(", ") || "nothing was stored" : ""}. Backup {e.backup}
          </>
        )
      case "cleaned_up":
        return (
          <>
            Clean-up:{" "}
            {Object.entries(e.types ?? {})
              .map(([type, t]) => `${typeNames[type] ?? type} older than ${t.older_than_days} days (${count(t.removed.records, "record")})`)
              .join(", ")}
            . Backup {e.backup}
          </>
        )
      case "deleting":
        return (
          <>
            {e.start ? `Started deleting ${dateTime(e.start)} – ${dateTime(e.end)}` : "Started a clean-up"} after backup {e.backup}. Nothing logged that it finished, so check the timeline; restoring that backup puts everything back
          </>
        )
      case "clean_up_skipped":
        return <>Clean-up skipped: {e.error}</>
      case "restored":
        return <>Restored backup {e.backup}. The state before is at {e.previous_state}</>
      case "restore_failed":
        return <>Restoring {e.backup} failed: {e.error}</>
      case "backup_deleted":
        return <>Deleted backup {e.backup}</>
      default:
        return e.action
    }
  }
  return (
    <Setting id="storage.log" description={log.data ? <>Kept in <span className="mono path">{log.data.path}</span></> : undefined}>
      {!log.data ? (
        <Skeleton width="100%" height={40} />
      ) : log.data.entries.length === 0 ? (
        <Empty title="Nothing deleted yet" />
      ) : (
        <div className="log-list">
          {log.data.entries.map((e, i) => (
            <div className="log-entry" key={i}>
              <time className="log-entry-time">{dateTime(e.at)}</time>
              <span className="log-entry-text">
                {(e.action === "restore_failed" || e.action === "clean_up_skipped" || e.action === "deleting") && <Badge tone="warning">Problem</Badge>} {words(e)}
              </span>
            </div>
          ))}
        </div>
      )}
    </Setting>
  )
}

function StoragePage({ actionsSlot }: { actionsSlot: (node: ReactNode) => ReactNode }) {
  const { data } = useSettings()
  const save = useSave()
  const [version, setVersion] = useState(0)
  const bump = () => setVersion((v) => v + 1)
  const limits = Object.keys(data!.settings.retention ?? {})
  return (
    <>
      {actionsSlot(
        <RestoreDefaults
          changes={limits.length ? ["keep every type forever"] : []}
          keeps="Backups, the deletion log and your history are not touched."
          onRestore={() => save({ retention: {} }, "Defaults restored")}
        />,
      )}
      <Card>
        <Usage key={version} />
      </Card>
      <Card>
        <KeepRows />
        <CleanUp onDone={bump} />
      </Card>
      <Card>
        <Backups version={version} />
      </Card>
      <Card>
        <Export />
      </Card>
      <Card>
        <DeleteRange onDone={bump} />
        <DeletionLog version={version} />
      </Card>
    </>
  )
}

export { StoragePage }
