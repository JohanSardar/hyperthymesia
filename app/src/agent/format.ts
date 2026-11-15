import type { AgentRun, ProviderId } from "../server/api"

const at = (us: number) => new Date(us / 1000)

const clock = (date: Date) => date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })

function dayWord(date: Date) {
  const today = new Date()
  const yesterday = new Date(today.getTime() - 86_400_000)
  if (date.toDateString() === today.toDateString()) return "Today"
  if (date.toDateString() === yesterday.toDateString()) return "Yesterday"
  return date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })
}

/** "6:00–10:00 PM" (or "18:00–22:00"), the hours of a four-hour window. */
function windowHours(startUs: number) {
  const start = at(startUs)
  const end = new Date(start.getTime() + 4 * 3_600_000)
  const short = (d: Date) => d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })
  const [a, b] = [short(start), short(end)]
  const suffix = (t: string) => t.match(/\s?[^\d:\s]+$/)?.[0] ?? ""
  return suffix(a) && suffix(a) === suffix(b) ? `${a.slice(0, -suffix(a).length)}–${b}` : `${a}–${b}`
}

/** "Today, 6:00–10:00 PM" for a four-hour window. */
function windowLabel(startUs: number) {
  return `${dayWord(at(startUs))}, ${windowHours(startUs)}`
}

/** The local YYYY-MM-DD of a window, for its day page. */
function dayKey(startUs: number) {
  const d = at(startUs)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`
}

function runTitle(run: AgentRun) {
  if (run.kind === "setup") return `Set up ${run.label ?? "a connection"}`
  if (run.kind === "check") return "Test"
  return run.window_start ? windowLabel(run.window_start) : "Pass"
}

function seconds(fromUs: number, toUs: number) {
  return Math.max(0, Math.round((toUs - fromUs) / 1e6))
}

function duration(totalSeconds: number) {
  if (totalSeconds < 60) return `${totalSeconds}s`
  if (totalSeconds < 3600) return `${Math.floor(totalSeconds / 60)}m ${totalSeconds % 60}s`
  return `${Math.floor(totalSeconds / 3600)}h ${Math.floor((totalSeconds % 3600) / 60)}m`
}

function money(usd: number) {
  if (usd === 0) return "$0"
  if (usd < 0.01) return `$${usd.toFixed(4)}`
  return `$${usd.toFixed(2)}`
}

/** Cost in plain words: subscriptions report no dollar cost. */
function costLabel(run: Pick<AgentRun, "cost_usd" | "provider" | "state">) {
  if (run.state === "running" && run.cost_usd === null) return "Counting…"
  if (run.state === "skipped") return "Nothing spent"
  // Subscriptions bill no single run; fx reports no dollar cost for them.
  if ((run.cost_usd === null || run.cost_usd === 0) && run.provider && isPlan(run.provider)) return "In your plan"
  if (run.cost_usd === null) return "Not reported"
  return money(run.cost_usd)
}

const isPlan = (id: ProviderId) => id === "codex" || id === "grok"

function tokens(n: number | null) {
  if (n === null) return "—"
  return n >= 10_000 ? `${Math.round(n / 1000)}k` : n.toLocaleString()
}

function ago(us: number) {
  const s = Math.round((Date.now() - us / 1000) / 1000)
  if (s < 0) {
    const ahead = -s
    if (ahead < 60) return "in under a minute"
    if (ahead < 3600) return `in ${Math.round(ahead / 60)} min`
    if (ahead < 86400) return `in ${Math.round(ahead / 3600)} h`
    return `on ${at(us).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })}`
  }
  if (s < 45) return "just now"
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`
  if (s < 86400) return `${Math.round(s / 3600)} h ago`
  return `${Math.round(s / 86400)} d ago`
}

export { ago, at, clock, costLabel, dayKey, dayWord, duration, isPlan, money, runTitle, seconds, tokens, windowHours, windowLabel }
