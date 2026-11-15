import type { LucideIcon } from "lucide-react"
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react"

type AppCommand = {
  id: string
  label: string
  /** The palette heading it shows under. */
  group: string
  icon?: LucideIcon
  /** Keys shown on the right, like "⌘B". Display only. */
  shortcut?: string
  keywords?: string[]
  run: () => unknown
}

type Registry = {
  commands: AppCommand[]
  register: (owner: string, commands: AppCommand[]) => () => void
  paletteOpen: boolean
  setPaletteOpen: (open: boolean) => void
}

const CommandsContext = createContext<Registry | null>(null)

function CommandsProvider({ children }: { children: ReactNode }) {
  const [owners, setOwners] = useState<Record<string, AppCommand[]>>({})
  const [paletteOpen, setPaletteOpen] = useState(false)
  // Stable, so registering commands never re-runs the effect that registered them.
  const register = useCallback((owner: string, commands: AppCommand[]) => {
    setOwners((current) => ({ ...current, [owner]: commands }))
    return () =>
      setOwners((current) => {
        const { [owner]: _removed, ...rest } = current
        return rest
      })
  }, [])
  const value = useMemo<Registry>(
    () => ({ commands: Object.values(owners).flat(), register, paletteOpen, setPaletteOpen }),
    [owners, register, paletteOpen],
  )
  return <CommandsContext value={value}>{children}</CommandsContext>
}

function useCommands() {
  const value = useContext(CommandsContext)
  if (!value) throw new Error("useCommands needs CommandsProvider")
  return value
}

let nextOwner = 0

/**
 * Adds commands to the ⌘K palette while the calling component is mounted.
 * Pages use it for their own actions ("Pull now", "New API key").
 */
function useRegisterCommands(commands: AppCommand[]) {
  const { register } = useCommands()
  const owner = useRef(`owner-${nextOwner++}`)
  const latest = useRef(commands)
  latest.current = commands
  const signature = commands.map((command) => `${command.id}:${command.label}:${command.shortcut ?? ""}`).join("|")
  useEffect(() => {
    const wrapped = latest.current.map((command) => ({ ...command, run: () => latest.current.find((c) => c.id === command.id)?.run() }))
    return register(owner.current, wrapped)
  }, [register, signature])
}

export { CommandsProvider, useCommands, useRegisterCommands }
export type { AppCommand }
