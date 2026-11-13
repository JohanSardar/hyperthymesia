import { Activity, Bot, History, Plug, Settings, type LucideIcon } from "lucide-react"

type NavItem = { to: string; label: string; icon: LucideIcon }

const primaryNav: NavItem[] = [
  { to: "/today", label: "Today", icon: Activity },
  { to: "/connections", label: "Connections", icon: Plug },
  { to: "/agent", label: "Agent", icon: Bot },
  { to: "/runs", label: "Runs", icon: History },
]

const settingsNav: NavItem = { to: "/settings", label: "Settings", icon: Settings }

export { primaryNav, settingsNav }
export type { NavItem }
