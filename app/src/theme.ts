import { useCallback, useSyncExternalStore } from "react"

type Theme = "system" | "dark" | "light"

const key = "hx-theme"
const listeners = new Set<() => void>()

function read(): Theme {
  try {
    const value = localStorage.getItem(key)
    return value === "light" || value === "system" ? value : "dark"
  } catch {
    return "dark"
  }
}

// Dark is the default; the attribute is always set so tokens never guess.
function apply(theme: Theme) {
  document.documentElement.dataset.theme = theme
}

function setTheme(theme: Theme) {
  try {
    if (theme === "dark") localStorage.removeItem(key)
    else localStorage.setItem(key, theme)
  } catch {
    // Storage can be blocked; the choice still applies for this visit.
  }
  apply(theme)
  current = theme
  listeners.forEach((listener) => listener())
}

let current = read()
apply(current)

function useTheme() {
  const theme = useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => current,
  )
  return [theme, useCallback(setTheme, [])] as const
}

export { setTheme, useTheme }
export type { Theme }
