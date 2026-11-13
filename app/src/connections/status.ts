import type { SetupRun, Source } from "../server/connections"

type Tone = "idle" | "live" | "ok" | "warning" | "error"

/** One status for a connection, combining its switch, its own status and its agent setup. */
function connectionStatus(source: Source, setup?: SetupRun | null): { tone: Tone; label: string; busy?: boolean } {
  if (!source.enabled) return { tone: "idle", label: "Off" }
  if (source.status === "needs_reconnect") return { tone: "warning", label: "Needs reconnect" }
  if (source.status === "failed" || source.status === "needs_attention" || setup?.status === "failed") return { tone: "error", label: "Needs attention" }
  if (source.status === "setting_up" || setup?.status === "pending") return { tone: "live", label: "Setting up", busy: true }
  if (source.last_error) return { tone: "warning", label: "Last pull failed" }
  if (source.status === "ready") return { tone: "ok", label: "Ready" }
  return { tone: "idle", label: source.status.replace(/_/g, " ") }
}

export { connectionStatus }
export type { Tone }
