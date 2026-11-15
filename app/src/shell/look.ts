import { useEffect } from "react"
import { useSettings } from "../server/settings"
import type { Density } from "../server/api"

// Base sizes from tokens.css; the font size setting moves every step together.
const text: Record<string, [number, number]> = {
  "2xs": [11, 14],
  xs: [12, 16],
  sm: [13, 18],
  base: [14, 20],
  lg: [16, 22],
  xl: [20, 26],
  "2xl": [26, 32],
  display: [32, 38],
}
const space: Record<string, number> = { "1": 4, "1-5": 6, "2": 8, "2-5": 10, "3": 12, "3-5": 14, "4": 16, "5": 20, "6": 24, "8": 32, "10": 40 }
const controls: Record<string, number> = { xs: 24, sm: 28, md: 32, lg: 40 }
const scale: Record<Density, number> = { compact: 0.8, comfortable: 1, spacious: 1.25 }

/** Applies the font size and density settings as token overrides on the page. */
function applyLook(fontSize = 14, density: Density = "comfortable") {
  const root = document.documentElement.style
  const delta = fontSize - 14
  for (const [name, [size, leading]] of Object.entries(text)) {
    if (delta) {
      root.setProperty(`--text-${name}`, `${size + delta}px`)
      root.setProperty(`--leading-${name}`, `${leading + Math.round(delta * 1.4)}px`)
    } else {
      root.removeProperty(`--text-${name}`)
      root.removeProperty(`--leading-${name}`)
    }
  }
  const factor = scale[density] ?? 1
  for (const [name, value] of Object.entries(space)) {
    if (factor === 1) root.removeProperty(`--space-${name}`)
    else root.setProperty(`--space-${name}`, `${Math.round(value * factor)}px`)
  }
  for (const [name, value] of Object.entries(controls)) {
    const sized = Math.round(value * (factor === 1 ? 1 : factor > 1 ? 1.12 : 0.88)) + Math.max(0, delta) * 2
    if (sized === value) root.removeProperty(`--control-${name}`)
    else root.setProperty(`--control-${name}`, `${sized}px`)
  }
  document.documentElement.dataset.density = density
}

/** Keeps the page's look in step with the saved settings. */
function useLook() {
  const { data } = useSettings()
  const fontSize = data?.settings.font_size
  const density = data?.settings.density
  useEffect(() => {
    applyLook(fontSize ?? 14, density ?? "comfortable")
  }, [fontSize, density])
}

export { applyLook, useLook }
