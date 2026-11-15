import { PanelLeft, Search } from "lucide-react"
import { useRef, type KeyboardEvent, type PointerEvent } from "react"
import { NavLink, useLocation } from "react-router"
import { IconButton, Kbd } from "../ui"
import { useCommands } from "./commands"
import { allNav, navFor } from "./nav"
import { useSidebar } from "./sidebar-state"

function PageList() {
  return (
    <div className="sidebar-group">
      <div className="sidebar-group-head">
        <span className="sidebar-group-label">Pages</span>
      </div>
      {allNav.map((item) => {
        const Icon = item.icon
        return (
          <NavLink key={item.to} to={item.to} className="sidebar-row">
            <Icon aria-hidden size={15} strokeWidth={1.6} />
            <span className="sidebar-row-label">{item.label}</span>
            <span className="sidebar-row-meta">⌘{item.key}</span>
          </NavLink>
        )
      })}
    </div>
  )
}

function ResizeHandle() {
  const { width, setWidth, resetWidth } = useSidebar()
  const start = useRef<{ x: number; width: number } | null>(null)
  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    start.current = { x: event.clientX, width }
    document.documentElement.dataset.resizing = ""
  }
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (start.current) setWidth(start.current.width + event.clientX - start.current.x)
  }
  const onPointerUp = () => {
    start.current = null
    delete document.documentElement.dataset.resizing
  }
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? 48 : 16
    if (event.key === "ArrowLeft") setWidth(width - step)
    else if (event.key === "ArrowRight") setWidth(width + step)
    else if (event.key === "Enter") resetWidth()
    else return
    event.preventDefault()
  }
  return (
    <div
      className="sidebar-resize"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      aria-valuenow={Math.round(width)}
      tabIndex={0}
      title="Drag to resize, double-click to reset"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={resetWidth}
      onKeyDown={onKeyDown}
    />
  )
}

function Sidebar() {
  const { open, toggle, filled, setSlot } = useSidebar()
  const { setPaletteOpen } = useCommands()
  const { pathname } = useLocation()
  const current = navFor(pathname)
  return (
    <aside className="sidebar" aria-label="Sidebar" inert={!open}>
      <div className="sidebar-head">
        <span className="sidebar-title">{current?.label ?? (pathname.startsWith("/kit") ? "Component kit" : "Hyperthymesia")}</span>
        <IconButton label="Hide sidebar" shortcut="⌘B" side="right" onClick={toggle}>
          <PanelLeft size={15} strokeWidth={1.6} aria-hidden />
        </IconButton>
      </div>
      <button type="button" className="sidebar-search" onClick={() => setPaletteOpen(true)}>
        <Search aria-hidden size={14} />
        <span>Search or jump to…</span>
        <Kbd>⌘K</Kbd>
      </button>
      <div className="sidebar-body">
        <div ref={setSlot} className="sidebar-slot" hidden={!filled} />
        {!filled && <PageList />}
      </div>
      <div className="sidebar-foot">
        <span className="wordmark">Hyperthymesia</span>
      </div>
      <ResizeHandle />
    </aside>
  )
}

export { Sidebar }
