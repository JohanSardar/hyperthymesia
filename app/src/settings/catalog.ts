import { Bell, Bot, CircleDot, HardDrive, Info, Keyboard, type LucideIcon, Palette, Plug, Server, Wrench } from "lucide-react"
import { bindable } from "../shell/keys"

type SettingsPageId = "recording" | "agent" | "connections" | "storage" | "server" | "appearance" | "notifications" | "shortcuts" | "advanced" | "about"

type SettingsPage = { id: SettingsPageId; label: string; group: string; icon: LucideIcon; description: string }

/** One row. Its words are what the page shows and what search reads. */
type SettingItem = { id: string; page: SettingsPageId; label: string; description: string; keywords?: string[] }

const pages: SettingsPage[] = [
  { id: "recording", label: "Recording", group: "This Mac", icon: CircleDot, description: "What this Mac records, how often, and what it never records" },
  { id: "agent", label: "AI agent", group: "This Mac", icon: Bot, description: "The model that writes up your day, its schedule and its spending limit" },
  { id: "connections", label: "Connections", group: "Your data", icon: Plug, description: "Defaults for new connections, and what the agent may do with them" },
  { id: "storage", label: "Storage and privacy", group: "Your data", icon: HardDrive, description: "Disk used, how long each type is kept, backups, export and deleting a time range" },
  { id: "server", label: "Devices and server", group: "Your data", icon: Server, description: "This server's status and address, pairing devices, remote access and API keys" },
  { id: "appearance", label: "Appearance", group: "App", icon: Palette, description: "Theme, density and text size" },
  { id: "notifications", label: "Notifications", group: "App", icon: Bell, description: "Which desktop notifications the Mac app shows" },
  { id: "shortcuts", label: "Shortcuts", group: "App", icon: Keyboard, description: "Every keyboard shortcut. Click one to change it" },
  { id: "advanced", label: "Advanced", group: "System", icon: Wrench, description: "Logs, repair and restarting the server" },
  { id: "about", label: "About", group: "System", icon: Info, description: "Version, the tour and where things are" },
]

const keepTypes: { type: string; label: string; what: string }[] = [
  { type: "screen_text", label: "screen text", what: "The words read from your screen" },
  { type: "screen_pictures", label: "screen pictures", what: "Pictures saved by older versions, and the text read from them. New versions save no pictures" },
  { type: "apps", label: "apps and windows", what: "Which app and window were in front, and clicks on buttons" },
  { type: "activity", label: "activity counts", what: "How many keys, clicks and scrolls per minute. Which keys is never recorded" },
  { type: "connections", label: "connection records", what: "Everything pulled from your connections" },
]

const items: SettingItem[] = [
  // Recording
  { id: "recording.on", page: "recording", label: "Record on this Mac", description: "Screen text, apps and windows on every paired Mac. Off stops recording within a few seconds and keeps your permissions; entries already queued still upload", keywords: ["capture", "stop", "start", "off"] },
  { id: "recording.pause", page: "recording", label: "Pause", description: "Stop recording for a while. It starts again by itself when the time is up. The menu bar can pause too", keywords: ["break", "privacy", "resume"] },
  { id: "recording.pace", page: "recording", label: "Recording pace", description: "Balanced reads the screen every 10 seconds on power and every 30 on battery, and slows down when the Mac is hot. Saver reads it every 2 minutes, always, for the least battery and CPU", keywords: ["battery", "saver", "balanced", "energy", "cpu", "rate", "frequency"] },
  { id: "recording.apps", page: "recording", label: "Never record these apps", description: "Nothing from these apps is recorded: no name, window title, text or clicks. Time in them shows as a gap", keywords: ["exclude", "block", "ignore", "privacy", "never-record"] },
  { id: "recording.sites", page: "recording", label: "Never record these sites", description: "Pages on these sites and their subdomains are not recorded. While any site is listed, a browser page whose address can't be read is skipped too", keywords: ["exclude", "block", "website", "domain", "url", "privacy", "never-record"] },
  { id: "recording.private", page: "recording", label: "Private windows", description: "The browser extension never records private or incognito windows. The Mac recorder can't tell a private window from a normal one yet, so add sites you want kept out to the list above", keywords: ["incognito", "private browsing", "safari", "chrome"] },
  { id: "recording.permissions", page: "recording", label: "Permissions", description: "macOS lets the Mac app read your screen only with Accessibility and Screen Recording. This page watches them and updates by itself", keywords: ["accessibility", "screen recording", "macos", "privacy", "allow"] },

  // AI agent
  { id: "agent.model", page: "agent", label: "Model", description: "The agent uses this for every write-up and connection setup. Changes apply to its next run", keywords: ["provider", "codex", "grok", "vercel", "llm", "ai"] },
  { id: "agent.test", page: "agent", label: "Test the model", description: "Sends one short request. Your provider may charge a little for it", keywords: ["check"] },
  { id: "agent.gateway", page: "agent", label: "Vercel AI Gateway key", description: "Saved on this server and never shown again", keywords: ["api key", "token"] },
  { id: "agent.page", page: "agent", label: "Agent page", description: "Accounts and sign-in, the schedule, the monthly spending limit and what the agent set up from each connection", keywords: ["schedule", "spending", "limit", "budget", "accounts", "sign in"] },
  { id: "agent.runs", page: "agent", label: "Runs", description: "Every write-up and setup run, with its cost, what it wrote and why one failed", keywords: ["history", "cost", "jobs"] },

  // Connections
  { id: "connections.list", page: "connections", label: "Your connections", description: "Sources besides this Mac: their status, last pull and what the agent set up", keywords: ["sources", "mcp", "api", "cli"] },
  { id: "connections.history", page: "connections", label: "First pull reaches back", description: "How many days of history a new connection pulls the first time. Each day can add a few paid write-ups, so keep it low", keywords: ["history days", "backfill", "default"] },
  { id: "connections.rules", page: "connections", label: "What the agent may do", description: "Connecting a source turns it on. The agent then sets up reads and pulls by itself. It can't add sources, and it can't turn back on anything you switched off", keywords: ["permissions", "rules", "off switch"] },
  { id: "connections.keys", page: "connections", label: "Where keys are kept", description: "In private files on this server, outside the data folder's databases and outside backups. Keys are never put into prompts, though the agent's command line can still read files on this computer", keywords: ["secrets", "tokens", "credentials"] },
  { id: "connections.local", page: "connections", label: "“Keep it local”", description: "Writing “keep it local” in a connection's description is an instruction to the agent, not a wall. The agent runs commands on this computer and can reach the network", keywords: ["local", "privacy", "sandbox"] },
  { id: "connections.remove", page: "connections", label: "Removing a connection", description: "Not possible yet. Switching one off stops every read and pull and keeps its key and entries", keywords: ["delete", "disconnect", "remove"] },

  // Storage and privacy
  { id: "storage.usage", page: "storage", label: "Disk used", description: "How much space each type of record takes on this server", keywords: ["space", "size", "bytes", "gb"] },
  ...keepTypes.map((t) => ({ id: `storage.keep.${t.type}`, page: "storage" as const, label: `Keep ${t.label} for`, description: `${t.what}. Older ones are deleted once a week, after a backup. Write-ups are kept`, keywords: ["retention", "keep", "delete old", "expire", t.type.replace("_", " ")] })),
  { id: "storage.cleanup", page: "storage", label: "Clean-up", description: "Applies the keep-for limits above. It runs once a week and takes a backup first; if the disk is too full for one, it deletes nothing", keywords: ["retention", "purge"] },
  { id: "storage.backup", page: "storage", label: "Back up now", description: "A complete copy of your history beside the data folder. Keys and server settings are left out on purpose", keywords: ["backup", "copy", "snapshot"] },
  { id: "storage.backups", page: "storage", label: "Backups", description: "Restore one to go back to it. The current state is set aside, not deleted, and the server restarts", keywords: ["restore", "backup", "undo"] },
  { id: "storage.export", page: "storage", label: "Export", description: "Every record and write-up in a time range as a JSON lines file", keywords: ["download", "json", "jsonl", "takeout"] },
  { id: "storage.delete", page: "storage", label: "Delete a time range", description: "Removes everything recorded in a time range, everywhere: day page, timeline, search and anything the agent can read. A backup is taken first and the delete is logged", keywords: ["erase", "remove", "forget", "purge", "privacy"] },
  { id: "storage.log", page: "storage", label: "Deletion log", description: "Every delete, clean-up and restore: when, which range, how much went and which backup holds it", keywords: ["history", "audit"] },
  { id: "storage.folder", page: "storage", label: "Data folder", description: "Where your history is stored on this server", keywords: ["path", "location", "state"] },

  // Devices and server
  { id: "server.status", page: "server", label: "This server", description: "Whether it's running, its version, and what it's working on", keywords: ["status", "health", "uptime", "version"] },
  { id: "server.address", page: "server", label: "Address", description: "Where this server listens. It only answers this computer", keywords: ["url", "port", "localhost"] },
  { id: "server.pair", page: "server", label: "Pair a device", description: "Makes a one-time link, good for 15 minutes. Open it on the Mac you want to pair", keywords: ["pairing", "link", "add device", "connect mac"] },
  { id: "server.remote", page: "server", label: "Remote access", description: "This server only listens on this computer. To reach it from elsewhere, put a private HTTPS address in front of it, such as Tailscale Serve, and enter that address here", keywords: ["tailscale", "https", "phone", "vpn", "remote url"] },
  { id: "server.devices", page: "server", label: "Devices", description: "Every Mac sending data, when it was last seen, renaming and merging duplicates", keywords: ["macs", "rename", "merge"] },
  { id: "server.keys", page: "server", label: "API keys", description: "Programs use these to read your history through /v1. Paired Macs and this app have keys here too", keywords: ["token", "access", "revoke"] },

  // Appearance
  { id: "appearance.theme", page: "appearance", label: "Theme", description: "Dark, light, or follow the system", keywords: ["dark mode", "light mode", "colour", "color"] },
  { id: "appearance.density", page: "appearance", label: "Density", description: "How much room rows and controls take", keywords: ["compact", "spacious", "spacing"] },
  { id: "appearance.font-size", page: "appearance", label: "Text size", description: "Makes every piece of text in the app bigger or smaller", keywords: ["font", "zoom", "larger", "smaller"] },

  // Notifications
  { id: "notifications.day_ready", page: "notifications", label: "Your day is written up", description: "When a new part of today is ready to read", keywords: ["ready", "write-up"] },
  { id: "notifications.failed_run", page: "notifications", label: "A write-up failed", description: "When the agent couldn't write up part of your day", keywords: ["error", "pass"] },
  { id: "notifications.failed_pull", page: "notifications", label: "A connection failed", description: "When a connection couldn't fetch new data", keywords: ["error", "pull"] },
  { id: "notifications.permission_lost", page: "notifications", label: "A permission was turned off", description: "When Accessibility, Screen Recording or Input Monitoring is switched off and recording can't be complete", keywords: ["accessibility", "screen recording"] },

  // Shortcuts
  ...bindable.map((b) => ({ id: `shortcuts.${b.id}`, page: "shortcuts" as const, label: b.label, description: b.description, keywords: ["keyboard", "key", "hotkey", "shortcut"] })),

  // Advanced
  { id: "advanced.logs", page: "advanced", label: "Server log", description: "The last lines the server wrote. Keys are blanked out", keywords: ["log", "debug", "errors"] },
  { id: "advanced.repair", page: "advanced", label: "Check and repair", description: "Checks the database and search index, makes the data folder private again and retries stuck processing. Starts no AI runs", keywords: ["fix", "integrity", "doctor"] },
  { id: "advanced.restart", page: "advanced", label: "Restart the server", description: "Stops and starts the server. Recording keeps going and uploads catch up", keywords: ["reboot", "reload"] },

  // About
  { id: "about.version", page: "about", label: "Version", description: "The version of the server and this app", keywords: ["build", "release"] },
  { id: "about.tour", page: "about", label: "Tour", description: "A short walk through Today, Connections, Agent and Settings", keywords: ["help", "guide", "welcome", "getting started"] },
  { id: "about.license", page: "about", label: "License", description: "Hyperthymesia is open source under the AGPL 3.0", keywords: ["open source", "agpl"] },
]

const itemById = new Map(items.map((i) => [i.id, i]))
const pageById = new Map(pages.map((p) => [p.id, p]))

/** Rows whose name, explanation, extra words or page contain every word typed. */
function searchSettings(query: string) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (!words.length) return []
  const scored = items.flatMap((item) => {
    const page = pageById.get(item.page)!
    const label = item.label.toLowerCase()
    const hay = [label, item.description.toLowerCase(), ...(item.keywords ?? []), page.label.toLowerCase()].join(" \u0000 ")
    if (!words.every((w) => hay.includes(w))) return []
    const score = words.every((w) => label.includes(w)) ? 2 : 1
    return [{ item, page, score }]
  })
  return scored.sort((a, b) => b.score - a.score)
}

export { itemById, items, keepTypes, pageById, pages, searchSettings }
export type { SettingItem, SettingsPage, SettingsPageId }
