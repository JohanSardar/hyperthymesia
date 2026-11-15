import { useMemo } from "react"
import { useSettings } from "../server/settings"
import { allNav } from "./nav"

/** A command that has a key combination the owner can change. */
type Bindable = { id: string; label: string; description: string; group: string; combo: string }

// Combinations are written "mod+shift+k": mod is ⌘ on a Mac and Ctrl elsewhere.
const bindable: Bindable[] = [
  { id: "palette", label: "Command palette", description: "Search pages, commands and your history from anywhere", group: "General", combo: "mod+k" },
  ...allNav.map((item) => ({ id: `go:${item.to}`, label: `Open ${item.label}`, description: item.hint, group: "Pages", combo: `mod+${item.key}` })),
  { id: "sidebar:toggle", label: "Show or hide the sidebar", description: "The column beside the icons, with this page's own list", group: "General", combo: "mod+b" },
  { id: "nav:back", label: "Go back", description: "The page you were on before", group: "General", combo: "mod+[" },
  { id: "nav:forward", label: "Go forward", description: "Undo a Go back", group: "General", combo: "mod+]" },
  { id: "settings:search", label: "Search settings", description: "Jump to the settings search box", group: "General", combo: "mod+shift+f" },
]

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform)

/** The combination a key press makes, or null if it has no modifier. */
function comboOf(event: KeyboardEvent | React.KeyboardEvent): string | null {
  const key = event.key.toLowerCase()
  if (["meta", "control", "shift", "alt"].includes(key)) return null
  const mod = isMac ? event.metaKey : event.ctrlKey
  if (!mod && !event.altKey && !(isMac && event.ctrlKey)) return null
  const parts = [mod && "mod", isMac && event.ctrlKey && "ctrl", event.altKey && "alt", event.shiftKey && "shift"].filter(Boolean) as string[]
  // Option and Shift change event.key on a Mac keyboard; use the physical key.
  const physical = event.code.startsWith("Key") ? event.code.slice(3).toLowerCase() : event.code.startsWith("Digit") ? event.code.slice(5) : key
  return [...parts, event.altKey || event.shiftKey ? physical : key].join("+")
}

const glyphs: Record<string, string> = { mod: isMac ? "⌘" : "Ctrl+", ctrl: "⌃", alt: isMac ? "⌥" : "Alt+", shift: isMac ? "⇧" : "Shift+" }

/** "mod+shift+k" → "⇧⌘K" on a Mac. */
function formatCombo(combo: string) {
  const parts = combo.split("+")
  const key = parts.pop() ?? ""
  const order = ["ctrl", "alt", "shift", "mod"]
  const mods = parts.sort((a, b) => order.indexOf(a) - order.indexOf(b)).map((p) => glyphs[p] ?? p)
  return mods.join("") + (key.length === 1 ? key.toUpperCase() : key)
}

/** Every bindable command's current combination. */
function useBindings() {
  const { data } = useSettings()
  const custom = data?.settings.shortcuts
  return useMemo(() => {
    const out: Record<string, string> = {}
    for (const b of bindable) out[b.id] = custom?.[b.id] ?? b.combo
    return out
  }, [custom])
}

export { bindable, comboOf, formatCombo, useBindings }
export type { Bindable }
