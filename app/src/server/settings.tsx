import { createContext, use, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react"
import { setTheme } from "../theme"
import { api, type Preferences, type SettingsResponse } from "./api"

type SettingsState = {
  data: SettingsResponse | null
  error: string | null
  reload: () => Promise<void>
  save: (patch: Partial<Preferences>) => Promise<void>
}

const SettingsContext = createContext<SettingsState | null>(null)

function SettingsProvider({ children }: { children: ReactNode }) {
  const [data, setData] = useState<SettingsResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  // Preferences including saves still in flight, so overlapping saves don't undo each other.
  const latest = useRef<Preferences | null>(null)

  const reload = useCallback(async () => {
    try {
      const next = await api.settings()
      setData(next)
      latest.current = next.settings
      setError(null)
      setTheme(next.settings.appearance)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The server didn't answer")
    }
  }, [])

  useEffect(() => {
    void reload()
  }, [reload])

  const save = useCallback(
    async (patch: Partial<Preferences>) => {
      if (!latest.current) return
      const next = { ...latest.current, ...patch }
      latest.current = next
      if (patch.appearance) setTheme(patch.appearance)
      try {
        const saved = await api.saveSettings(next)
        setData((current) => (current ? { ...current, settings: saved.settings } : current))
      } catch (cause) {
        void reload()
        throw cause
      }
    },
    [reload],
  )

  const value = useMemo(() => ({ data, error, reload, save }), [data, error, reload, save])
  return <SettingsContext value={value}>{children}</SettingsContext>
}

function useSettings() {
  const value = use(SettingsContext)
  if (!value) throw new Error("useSettings needs SettingsProvider")
  return value
}

export { SettingsProvider, useSettings }
