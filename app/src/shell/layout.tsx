import { ArrowLeft, ArrowRight, Component, Monitor, Moon, PanelLeft, RotateCw, Sun } from "lucide-react"
import { useCallback, useEffect, useMemo, useState, type CSSProperties } from "react"
import { Outlet, useLocation, useNavigate } from "react-router"
import { Toaster } from "../ui"
import { useAppearance } from "./appearance"
import { CommandsProvider, useCommands, useRegisterCommands, type AppCommand } from "./commands"
import { allNav } from "./nav"
import { Palette } from "./palette"
import { Rail } from "./rail"
import { Sidebar } from "./sidebar"
import { SidebarContext } from "./sidebar-state"

const openKey = "hx-sidebar"
const widthKey = "hx-sidebar-width"
const defaultWidth = 248
const minWidth = 200
const maxWidth = 400
// The main panel never gets narrower than this while the sidebar is docked.
const minMain = 420
const railWidth = 52

const narrow = () => window.matchMedia("(max-width: 720px)").matches

function store(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key)
    else localStorage.setItem(key, value)
  } catch {
    // Storage can be blocked; the change still applies for this visit.
  }
}

function readOpen() {
  if (narrow()) return false
  try {
    return localStorage.getItem(openKey) !== "closed"
  } catch {
    return true
  }
}

function readWidth() {
  try {
    const value = Number(localStorage.getItem(widthKey))
    return value ? clampWidth(value) : defaultWidth
  } catch {
    return defaultWidth
  }
}

function clampWidth(value: number) {
  const room = Math.max(minWidth, window.innerWidth - railWidth - minMain)
  return Math.round(Math.min(Math.max(value, minWidth), maxWidth, room))
}

function useBuiltInCommands(toggleSidebar: () => void, resetWidth: () => void) {
  const navigate = useNavigate()
  const { setTheme } = useAppearance()
  const commands = useMemo<AppCommand[]>(
    () => [
      ...allNav.map((item) => ({
        id: `go:${item.to}`,
        label: item.label,
        group: "Go to",
        icon: item.icon,
        shortcut: `⌘${item.key}`,
        keywords: item.keywords,
        run: () => navigate(item.to),
      })),
      { id: "go:/kit", label: "Component kit", group: "Go to", icon: Component, keywords: ["design", "components", "styles"], run: () => navigate("/kit") },
      { id: "theme:dark", label: "Use dark theme", group: "Commands", icon: Moon, keywords: ["appearance", "night"], run: () => setTheme("dark") },
      { id: "theme:light", label: "Use light theme", group: "Commands", icon: Sun, keywords: ["appearance", "day"], run: () => setTheme("light") },
      { id: "theme:system", label: "Match the system theme", group: "Commands", icon: Monitor, keywords: ["appearance", "auto"], run: () => setTheme("system") },
      { id: "sidebar:toggle", label: "Show or hide the sidebar", group: "Commands", icon: PanelLeft, shortcut: "⌘B", keywords: ["panel", "collapse"], run: toggleSidebar },
      { id: "sidebar:reset", label: "Reset sidebar width", group: "Commands", icon: PanelLeft, keywords: ["panel", "size"], run: resetWidth },
      { id: "nav:back", label: "Go back", group: "Commands", icon: ArrowLeft, shortcut: "⌘[", keywords: ["previous"], run: () => navigate(-1) },
      { id: "nav:forward", label: "Go forward", group: "Commands", icon: ArrowRight, shortcut: "⌘]", keywords: ["next"], run: () => navigate(1) },
      { id: "app:reload", label: "Reload the app", group: "Commands", icon: RotateCw, keywords: ["refresh"], run: () => window.location.reload() },
    ],
    [navigate, setTheme, toggleSidebar, resetWidth],
  )
  useRegisterCommands(commands)
}

function useShortcuts(toggleSidebar: () => void) {
  const navigate = useNavigate()
  const { paletteOpen, setPaletteOpen } = useCommands()
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey) return
      const key = event.key.toLowerCase()
      if (key === "k" && !event.shiftKey) {
        event.preventDefault()
        setPaletteOpen(!paletteOpen)
        return
      }
      if (paletteOpen || event.shiftKey) return
      const page = allNav.find((item) => item.key === key)
      if (page) {
        event.preventDefault()
        navigate(page.to)
      } else if (key === "b") {
        event.preventDefault()
        toggleSidebar()
      } else if (key === "[") {
        event.preventDefault()
        navigate(-1)
      } else if (key === "]") {
        event.preventDefault()
        navigate(1)
      }
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [navigate, paletteOpen, setPaletteOpen, toggleSidebar])
}

function Shell() {
  const [open, setOpen] = useState(readOpen)
  const [width, setWidthState] = useState(readWidth)
  const [slot, setSlot] = useState<HTMLElement | null>(null)
  const [filled, setFilled] = useState(false)

  const toggle = useCallback(() => {
    setOpen((value) => {
      if (!narrow()) store(openKey, value ? "closed" : null)
      return !value
    })
  }, [])
  const setWidth = useCallback((value: number) => {
    const next = clampWidth(value)
    setWidthState(next)
    store(widthKey, String(next))
  }, [])
  const resetWidth = useCallback(() => {
    setWidthState(clampWidth(defaultWidth))
    store(widthKey, null)
  }, [])

  // Narrow windows overlay the sidebar; close it after each navigation there.
  const { pathname } = useLocation()
  useEffect(() => {
    if (narrow()) setOpen(false)
  }, [pathname])

  // Keep the docked sidebar from squeezing the page when the window shrinks.
  useEffect(() => {
    const onResize = () => setWidthState((value) => clampWidth(value))
    window.addEventListener("resize", onResize)
    return () => window.removeEventListener("resize", onResize)
  }, [])

  useBuiltInCommands(toggle, resetWidth)
  useShortcuts(toggle)

  const sidebar = useMemo(
    () => ({ open, toggle, width, setWidth, resetWidth, slot, setSlot, filled, setFilled }),
    [open, toggle, width, setWidth, resetWidth, slot, filled],
  )

  return (
    <SidebarContext value={sidebar}>
      <div className="shell" data-sidebar={open ? "open" : "closed"} style={{ "--sidebar-width": `${width}px` } as CSSProperties}>
        <Rail />
        <Sidebar />
        {open && <div className="sidebar-scrim" onClick={toggle} aria-hidden />}
        <main className="main">
          <Outlet />
        </main>
      </div>
      <Palette />
      <Toaster />
    </SidebarContext>
  )
}

function Layout() {
  return (
    <CommandsProvider>
      <Shell />
    </CommandsProvider>
  )
}

export { Layout }
