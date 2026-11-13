import {
  AppWindow,
  Bot,
  CalendarDays,
  Camera,
  Clock,
  FileText,
  GitCommitHorizontal,
  Globe,
  HeartPulse,
  Keyboard,
  MapPin,
  MessageSquare,
  MousePointerClick,
  Moon,
  Plug,
  Terminal,
  Video,
  Activity,
  type LucideIcon,
} from "lucide-react"
import type { Envelope } from "../server/connections"

type Line = { icon: LucideIcon; text: string; detail?: string }

const str = (value: unknown) => (typeof value === "string" && value.trim() ? value.trim() : undefined)
const clip = (text: string, max = 160) => {
  const flat = text.replace(/\s+/g, " ").trim()
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat
}
const host = (url: string) => {
  try {
    return new URL(url).host.replace(/^www\./, "")
  } catch {
    return url
  }
}
const duration = (ms: number) => {
  const s = Math.round(ms / 1000)
  if (s < 60) return `${s}s`
  const m = Math.round(s / 60)
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`
}

/** One readable line for any entry. Unknown kinds fall back to the first title-like field. */
function describe(envelope: Envelope): Line {
  const p = (envelope.payload ?? {}) as Record<string, unknown>
  switch (envelope.kind) {
    case "focus.span": {
      const title = str(p.window_title)
      return { icon: AppWindow, text: title ? `${str(p.app_name) ?? "App"} · ${clip(title, 120)}` : `Using ${str(p.app_name) ?? "an app"}` }
    }
    case "presence.span": {
      const state = str(p.state) ?? "active"
      const words: Record<string, string> = { active: "At the Mac", idle: "Away from the keyboard", locked: "Mac locked", device_asleep: "Mac asleep", display_off: "Display off" }
      return { icon: Moon, text: words[state] ?? state }
    }
    case "input.bucket":
      return { icon: Keyboard, text: `${Number(p.keys ?? 0)} keys, ${Number(p.clicks ?? 0)} clicks` }
    case "ui.click":
      return { icon: MousePointerClick, text: `Clicked ${str(p.label) ? `“${clip(str(p.label)!, 80)}”` : (str(p.role) ?? "something")}` }
    case "text.snapshot":
      return { icon: FileText, text: clip(str(p.text) ?? "Text on screen"), detail: str(p.surface) }
    case "web.visit": {
      const url = str(p.url) ?? ""
      return { icon: Globe, text: str(p.title) ? clip(str(p.title)!) : host(url), detail: host(url) }
    }
    case "shell.command": {
      const code = typeof p.exit_code === "number" && p.exit_code !== 0 ? ` (exit ${p.exit_code})` : ""
      return { icon: Terminal, text: `${clip(str(p.command) ?? "", 140)}${code}`, detail: str(p.cwd) }
    }
    case "code.commit":
      return { icon: GitCommitHorizontal, text: clip(str(p.message) ?? "Commit"), detail: str(p.repo) }
    case "agent.message": {
      const who = p.role === "user" ? "You" : p.role === "assistant" ? "Agent" : p.role === "tool" ? "Tool" : "Thread"
      return { icon: Bot, text: `${who}: ${clip(str(p.text) ?? "", 150)}`, detail: str(p.thread_title) }
    }
    case "calendar.event":
      return { icon: CalendarDays, text: clip(str(p.title) ?? "Event"), detail: str(p.location) }
    case "message.event": {
      const dir = p.direction === "sent" ? "To" : "From"
      const who = str(p.counterpart) ?? str(p.conversation)
      return { icon: MessageSquare, text: str(p.text) ? clip(str(p.text)!) : `${dir} ${who ?? "someone"}`, detail: who ? `${dir} ${who} · ${str(p.service) ?? ""}`.replace(/ · $/, "") : str(p.service) }
    }
    case "media.chunk":
      return { icon: Video, text: `Screen recording, ${Number(p.frames ?? 0)} frames` }
    case "media.still":
      return { icon: Camera, text: str(p.window_title) ? `Screenshot · ${clip(str(p.window_title)!, 120)}` : "Screenshot" }
    case "usage.aggregate":
      return { icon: Clock, text: `${str(p.app) ?? "App"} for ${duration(Number(p.seconds_estimated ?? 0) * 1000)}`, detail: str(p.day) }
    case "health.sample":
      return { icon: HeartPulse, text: `${str(p.metric) ?? "Health"}: ${p.value ?? ""} ${str(p.unit) ?? ""}`.trim() }
    case "location.visit":
      return { icon: MapPin, text: str(p.place_name) ?? "A place" }
    case "collector.health":
      return { icon: Activity, text: `${str(p.collector) ?? "Collector"} ${str(p.status) ?? ""}`.trim(), detail: str(p.detail) }
  }
  // Pulled records and other kinds: pick the most title-like field.
  const fields = ["title", "name", "summary", "subject", "text", "message", "content", "body", "description"]
  const nested = [p, (p.record ?? p.item ?? p.data ?? {}) as Record<string, unknown>]
  for (const obj of nested) {
    for (const field of fields) {
      const value = str(obj[field])
      if (value) return { icon: Plug, text: clip(value) }
    }
  }
  return { icon: Plug, text: str(envelope.subject) ?? str(envelope.resource) ?? envelope.kind }
}

/** When it happened: the source's own time when it has one, else when it was observed. */
function happenedAt(envelope: Envelope) {
  return envelope.time.source_start ?? envelope.time.observed_at
}

/** The source as a person would name it. */
function sourceLabel(connectorInstance: string, names: Map<string, string>) {
  const known = names.get(connectorInstance)
  if (known) return known
  const [prefix, rest = ""] = connectorInstance.split(":")
  if (prefix === "macos-agent") return "Mac"
  if (prefix === "macos-media") return "Screen"
  if (prefix === "companion") {
    const name = rest.split(":")[0]
    return name ? name.charAt(0).toUpperCase() + name.slice(1) : "Companion"
  }
  if (prefix === "source") return "Connection"
  return prefix
}

export { describe, happenedAt, sourceLabel }
export type { Line }
