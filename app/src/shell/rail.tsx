import { Command, Moon, Sun } from "lucide-react"
import type { ReactNode } from "react"
import { NavLink } from "react-router"
import { Tooltip, TooltipContent, TooltipTrigger } from "@ui-sdk/react/tooltip"
import { Kbd } from "../ui"
import { useAppearance } from "./appearance"
import { useCommands } from "./commands"
import { primaryNav, settingsNav, type NavItem } from "./nav"

function Tip({ label, shortcut }: { label: string; shortcut?: string }) {
  return (
    <TooltipContent className="tooltip" side="right" sideOffset={8}>
      {label}
      {shortcut && <Kbd>{shortcut}</Kbd>}
    </TooltipContent>
  )
}

function RailLink({ item }: { item: NavItem }) {
  const Icon = item.icon
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <NavLink to={item.to} className="rail-item" aria-label={item.label}>
            <Icon aria-hidden size={17} strokeWidth={1.6} />
          </NavLink>
        }
      />
      <Tip label={item.label} shortcut={`⌘${item.key}`} />
    </Tooltip>
  )
}

function RailButton({ label, shortcut, onClick, children }: { label: string; shortcut?: string; onClick: () => void; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger render={<button type="button" className="rail-item" aria-label={label} onClick={onClick} />}>{children}</TooltipTrigger>
      <Tip label={label} shortcut={shortcut} />
    </Tooltip>
  )
}

function Rail() {
  const { setPaletteOpen } = useCommands()
  const { dark, setTheme } = useAppearance()
  return (
    <nav className="rail" aria-label="Pages">
      <div className="rail-group">
        {primaryNav.map((item) => (
          <RailLink key={item.to} item={item} />
        ))}
      </div>
      <div className="rail-group">
        <RailButton label="Search pages and commands" shortcut="⌘K" onClick={() => setPaletteOpen(true)}>
          <Command aria-hidden size={16} strokeWidth={1.6} />
        </RailButton>
        <RailButton label={dark ? "Switch to light theme" : "Switch to dark theme"} onClick={() => setTheme(dark ? "light" : "dark")}>
          {dark ? <Sun aria-hidden size={16} strokeWidth={1.6} /> : <Moon aria-hidden size={16} strokeWidth={1.6} />}
        </RailButton>
        <RailLink item={settingsNav} />
      </div>
    </nav>
  )
}

export { Rail }
