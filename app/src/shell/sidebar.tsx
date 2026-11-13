import { PanelLeft } from "lucide-react"
import { NavLink } from "react-router"
import { IconButton } from "../ui"
import { primaryNav, settingsNav, type NavItem } from "./nav"
import { useSidebar } from "./sidebar-state"

function NavRow({ item }: { item: NavItem }) {
  const Icon = item.icon
  return (
    <NavLink to={item.to} className="nav-item">
      <Icon aria-hidden size={14} strokeWidth={1.75} />
      <span>{item.label}</span>
    </NavLink>
  )
}

function Sidebar() {
  const { open, toggle } = useSidebar()
  return (
    <aside className="sidebar" data-open={open ? "" : undefined} aria-label="Sidebar" inert={!open}>
      <div className="sidebar-head">
        <span className="wordmark">Hyperthymesia</span>
        <IconButton label="Close sidebar" shortcut="⌘B" side="right" onClick={toggle}>
          <PanelLeft size={14} strokeWidth={1.75} aria-hidden />
        </IconButton>
      </div>
      <nav className="sidebar-nav" aria-label="Main">
        {primaryNav.map((item) => (
          <NavRow key={item.to} item={item} />
        ))}
      </nav>
      <div className="sidebar-foot">
        <NavRow item={settingsNav} />
      </div>
    </aside>
  )
}

export { Sidebar }
