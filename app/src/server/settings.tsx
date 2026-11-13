import { createContext, use, useCallback, useEffect, useMemo, useState, type ReactNode } from "react"
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

  const reload = useCallback(async () => {
    try {
      const next = await api.settings()
      setData(next)
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
      if (!data) return
      const next = { ...data.settings, ...patch }
      if (patch.appearance) setTheme(patch.appearance)
      const saved = await api.saveSettings(next)
      setData((current) => (current ? { ...current, settings: saved.settings } : current))
    },
    [data],
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
