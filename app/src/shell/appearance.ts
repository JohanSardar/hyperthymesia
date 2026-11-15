import { useCallback } from "react"
import { useSettings } from "../server/settings"
import { setTheme, useTheme, type Theme } from "../theme"
import { toast } from "../ui"

/** The theme, saved to the server so every window and reload agrees. */
function useAppearance() {
  const [theme] = useTheme()
  const { save } = useSettings()
  const set = useCallback(
    (next: Theme) => {
      setTheme(next)
      save({ appearance: next }).catch(() => toast.error("Couldn't save the theme", { description: "It applies until the app reloads" }))
    },
    [save],
  )
  const dark = theme === "dark" || (theme === "system" && !window.matchMedia("(prefers-color-scheme: light)").matches)
  return { theme, dark, setTheme: set }
}

export { useAppearance }
