import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandGroupLabel,
  CommandInput,
  CommandItem,
  CommandItemIcon,
  CommandList,
} from "@ui-sdk/react/command"
import { Dialog, DialogBackdrop, DialogContent, DialogTitle } from "@ui-sdk/react/dialog"
import { CornerDownLeft, Search } from "lucide-react"
import { Kbd } from "../ui"
import { useCommands, type AppCommand } from "./commands"

// Groups show in this order; page groups follow.
const order = ["Go to", "Commands"]

function grouped(commands: AppCommand[]) {
  const groups = new Map<string, AppCommand[]>()
  for (const command of commands) groups.set(command.group, [...(groups.get(command.group) ?? []), command])
  return [...groups.entries()].sort(([a], [b]) => {
    const ia = order.indexOf(a)
    const ib = order.indexOf(b)
    return (ia < 0 ? order.length : ia) - (ib < 0 ? order.length : ib)
  })
}

// Predictable matching: the label first, then extra words. Fuzzy matching
// over descriptions sent "devices" to Connections ("services").
function score(item: { label: string; keywords?: string[] }, query: string) {
  const q = query.trim().toLowerCase()
  if (!q) return 1
  const label = item.label.toLowerCase()
  if (label.startsWith(q)) return 4
  if (label.split(/\s+/).some((word) => word.startsWith(q))) return 3
  if (label.includes(q)) return 2
  return item.keywords?.some((word) => word.toLowerCase().split(/\s+/).some((part) => part.startsWith(q))) ? 1 : 0
}

function Palette() {
  const { commands, paletteOpen, setPaletteOpen } = useCommands()
  return (
    <Dialog open={paletteOpen} onOpenChange={setPaletteOpen}>
      <DialogBackdrop className="scrim palette-scrim" />
      <DialogContent className="palette" initialFocus=".palette-input">
        <DialogTitle className="sr-only">Search pages and commands</DialogTitle>
        <Command
          className="palette-command"
          filter={score}
          onAction={(value) => {
            const command = commands.find((c) => c.id === value)
            setPaletteOpen(false)
            // Run after the dialog starts closing so focus lands where the command sends it.
            if (command) queueMicrotask(() => void command.run())
          }}
        >
          <div className="palette-search">
            <Search aria-hidden size={15} className="palette-search-icon" />
            <CommandInput className="palette-input" aria-label="Search pages and commands" placeholder="Search pages or run a command" />
            <Kbd>esc</Kbd>
          </div>
          <CommandList className="palette-list">
            <CommandEmpty className="palette-empty">Nothing matches. Try a page name, like “Timeline”.</CommandEmpty>
            {grouped(commands).map(([group, items]) => (
              <CommandGroup key={group} className="palette-group">
                <CommandGroupLabel className="palette-group-label">{group}</CommandGroupLabel>
                {items.map((command) => {
                  const Icon = command.icon
                  return (
                    <CommandItem key={command.id} value={command.id} label={command.label} keywords={command.keywords} className="palette-item">
                      {Icon && (
                        <CommandItemIcon className="palette-item-icon">
                          <Icon size={15} strokeWidth={1.75} />
                        </CommandItemIcon>
                      )}
                      <span className="palette-item-label">{command.label}</span>
                      {command.shortcut && <Kbd>{command.shortcut}</Kbd>}
                    </CommandItem>
                  )
                })}
              </CommandGroup>
            ))}
          </CommandList>
          <div className="palette-foot">
            <span>
              <Kbd>↑</Kbd>
              <Kbd>↓</Kbd> to move
            </span>
            <span>
              <Kbd>
                <CornerDownLeft size={10} aria-hidden />
              </Kbd>{" "}
              to open
            </span>
          </div>
        </Command>
      </DialogContent>
    </Dialog>
  )
}

export { Palette }
