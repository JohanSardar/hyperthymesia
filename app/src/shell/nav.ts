import { Bot, CalendarDays, ChartNoAxesGantt, History, MonitorSmartphone, Plug, Search, Settings, type LucideIcon } from "lucide-react"

type NavItem = {
  to: string
  label: string
  icon: LucideIcon
  /** One line for the palette and the sidebar's page list. */
  hint: string
  /** Extra words the palette matches. */
  keywords?: string[]
  /** Key after ⌘ that opens the page. */
  key: string
}

// Every page the app will have. Later tickets fill pages in; they never edit this list.
const primaryNav: NavItem[] = [
  { to: "/today", label: "Today", icon: CalendarDays, hint: "Your day as it happens", keywords: ["home", "day"], key: "1" },
  { to: "/timeline", label: "Timeline", icon: ChartNoAxesGantt, hint: "Every day, hour by hour", keywords: ["history", "past"], key: "2" },
  { to: "/search", label: "Search", icon: Search, hint: "Find anything you've seen", keywords: ["find", "recall"], key: "3" },
  { to: "/connections", label: "Connections", icon: Plug, hint: "Apps and services that feed your memory", keywords: ["sources", "mcp", "api"], key: "4" },
  { to: "/agent", label: "Agent", icon: Bot, hint: "What the agent reads and when", keywords: ["ai", "model"], key: "5" },
  { to: "/runs", label: "Runs", icon: History, hint: "Every pass and pull", keywords: ["jobs", "passes", "pulls"], key: "6" },
  { to: "/devices", label: "Devices", icon: MonitorSmartphone, hint: "Other computers paired with this one", keywords: ["pair", "computers"], key: "7" },
]

const settingsNav: NavItem = { to: "/settings", label: "Settings", icon: Settings, hint: "Model, recording, storage and access", keywords: ["preferences", "options"], key: "," }

const allNav = [...primaryNav, settingsNav]

function navFor(pathname: string) {
  return allNav.find((item) => pathname === item.to || pathname.startsWith(`${item.to}/`))
}

export { allNav, navFor, primaryNav, settingsNav }
export type { NavItem }
