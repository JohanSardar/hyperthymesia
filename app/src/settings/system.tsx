import { Check, RotateCw, TriangleAlert } from "lucide-react"
import { useState, type ReactNode } from "react"
import { admin, waitForServer, type RepairStep } from "../server/admin"
import { usePolled } from "../server/connections"
import { useReplayTour } from "../tour/tour"
import { Button, Card, Confirm, ConfirmTrigger, Skeleton, Spinner, buttonAttrs, toast } from "../ui"
import { errorText, Setting } from "./parts"

function AdvancedPage(_: { actionsSlot: (node: ReactNode) => ReactNode }) {
  const server = usePolled(admin.server)
  const [lines, setLines] = useState<string[] | null>(null)
  const [logPath, setLogPath] = useState<string | null>(null)
  const [steps, setSteps] = useState<RepairStep[] | null>(null)
  const [busy, setBusy] = useState<"logs" | "repair" | "restart" | null>(null)

  const loadLogs = async () => {
    setBusy("logs")
    try {
      const out = await admin.logs()
      setLines(out.lines)
      setLogPath(out.path)
    } catch (cause) {
      toast.error("Couldn't read the log", { description: errorText(cause) })
    } finally {
      setBusy(null)
    }
  }
  const repair = async () => {
    setBusy("repair")
    try {
      setSteps((await admin.repair()).steps)
    } catch (cause) {
      toast.error("The check didn't run", { description: errorText(cause) })
    } finally {
      setBusy(null)
    }
  }
  const restart = async () => {
    setBusy("restart")
    try {
      await admin.restart()
      toast("Restarting…")
      if (await waitForServer()) toast.success("The server is back")
      else toast.error("The server hasn't come back yet", { description: "Check the log, or restart the Mac app" })
    } catch (cause) {
      toast.error("Couldn't restart", { description: errorText(cause) })
    } finally {
      setBusy(null)
      void server.refresh()
    }
  }
  const supervised = server.data?.supervised
  return (
    <>
      <Card>
        <Setting
          id="advanced.logs"
          description={logPath ? <span className="mono path">{logPath}</span> : lines && !logPath ? "This server isn't writing a log file" : undefined}
          control={
            <Button onClick={() => void loadLogs()} disabled={busy !== null}>
              {busy === "logs" ? <Spinner size={12} /> : lines ? "Reload" : "Show"}
            </Button>
          }
        >
          {lines && lines.length > 0 && <pre className="server-log">{lines.join("\n")}</pre>}
        </Setting>
        <Setting
          id="advanced.repair"
          control={
            <Button onClick={() => void repair()} disabled={busy !== null}>
              {busy === "repair" ? (
                <>
                  <Spinner size={12} /> Checking…
                </>
              ) : (
                "Check and repair"
              )}
            </Button>
          }
        >
          {steps && (
            <ul className="repair-steps">
              {steps.map((s) => (
                <li key={s.step} data-ok={s.ok ? "" : undefined}>
                  {s.ok ? <Check size={13} aria-hidden /> : <TriangleAlert size={13} aria-hidden />}
                  <span className="row-label">{s.step}</span>
                  <span className="row-description">{s.detail}</span>
                </li>
              ))}
            </ul>
          )}
        </Setting>
        <Setting
          id="advanced.restart"
          description={supervised === false ? "This server wasn't started by the Mac's service manager, so it wouldn't come back after stopping. Restart it the way you started it." : undefined}
          control={
            <Confirm
              title="Restart the server?"
              description={server.data?.running_passes ? "A write-up is running now; it stops and runs again later, which can cost again." : "It's back in a few seconds. Recording keeps going and uploads catch up."}
              confirmLabel="Restart"
              trigger={
                <ConfirmTrigger {...buttonAttrs("default", "sm")} disabled={busy !== null || !supervised}>
                  {busy === "restart" ? <Spinner size={12} /> : <RotateCw size={13} aria-hidden />} Restart
                </ConfirmTrigger>
              }
              onConfirm={restart}
            />
          }
        />
      </Card>
    </>
  )
}

function AboutPage(_: { actionsSlot: (node: ReactNode) => ReactNode }) {
  const server = usePolled(admin.server)
  const replay = useReplayTour()
  return (
    <Card>
      <Setting id="about.version" description={server.data ? `Hyperthymesia ${server.data.version}` : <Skeleton width={120} />} />
      <Setting id="about.tour" control={<Button onClick={() => void replay()}>Replay the tour</Button>} />
      <Setting id="about.license" />
    </Card>
  )
}

export { AboutPage, AdvancedPage }
