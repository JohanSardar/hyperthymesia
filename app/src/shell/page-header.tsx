import { PanelLeft } from "lucide-react"
import type { ReactNode } from "react"
import { Link } from "react-router"
import { IconButton } from "../ui"
import { useSidebar } from "./sidebar-state"

type Crumb = { label: ReactNode; to?: string }

function PageHeader({ title, crumbs, actions }: { title?: ReactNode; crumbs?: Crumb[]; actions?: ReactNode }) {
  const { open, toggle } = useSidebar()
  return (
    <header className="page-header">
      <div className="page-header-start">
        {!open && (
          <IconButton label="Open sidebar" shortcut="⌘B" onClick={toggle}>
            <PanelLeft size={14} strokeWidth={1.75} aria-hidden />
          </IconButton>
        )}
        {crumbs ? (
          <nav className="page-header-title" aria-label="Breadcrumb">
            {crumbs.map((crumb, index) => (
              <span key={index} className="crumb">
                {index > 0 && <span className="crumb-sep" aria-hidden>›</span>}
                {crumb.to ? <Link to={crumb.to}>{crumb.label}</Link> : <span aria-current="page">{crumb.label}</span>}
              </span>
            ))}
          </nav>
        ) : (
          <h1 className="page-header-title">{title}</h1>
        )}
      </div>
      {actions && <div className="page-header-actions">{actions}</div>}
    </header>
  )
}

export { PageHeader }
