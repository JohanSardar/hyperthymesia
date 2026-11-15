import { ArrowLeft, ArrowRight, PanelLeft } from "lucide-react"
import { useEffect, useRef, type ReactNode } from "react"
import { Link, useLocation, useNavigate, useNavigationType } from "react-router"
import { IconButton } from "../ui"
import { useSidebar } from "./sidebar-state"

type Crumb = { label: ReactNode; to?: string }

// The app runs in a window with no browser chrome, so the header carries back and forward.
let furthest = 0

function useHistoryButtons() {
  const location = useLocation()
  const type = useNavigationType()
  const index: number = (window.history.state as { idx?: number } | null)?.idx ?? 0
  const seen = useRef(location.key)
  useEffect(() => {
    if (seen.current === location.key) return
    seen.current = location.key
    if (type === "PUSH") furthest = index
  }, [location.key, type, index])
  furthest = Math.max(furthest, index)
  return { canBack: index > 0, canForward: index < furthest }
}

function PageHeader({ title, crumbs, actions }: { title?: ReactNode; crumbs?: Crumb[]; actions?: ReactNode }) {
  const { open, toggle } = useSidebar()
  const navigate = useNavigate()
  const { canBack, canForward } = useHistoryButtons()
  return (
    <header className="page-header">
      <div className="page-header-start">
        {!open && (
          <IconButton label="Show sidebar" shortcut="⌘B" onClick={toggle}>
            <PanelLeft size={15} strokeWidth={1.6} aria-hidden />
          </IconButton>
        )}
        <div className="page-header-history">
          <IconButton label="Back" shortcut="⌘[" disabled={!canBack} onClick={() => navigate(-1)}>
            <ArrowLeft size={15} strokeWidth={1.6} aria-hidden />
          </IconButton>
          <IconButton label="Forward" shortcut="⌘]" disabled={!canForward} onClick={() => navigate(1)}>
            <ArrowRight size={15} strokeWidth={1.6} aria-hidden />
          </IconButton>
        </div>
        {crumbs ? (
          <nav className="page-header-title" aria-label="Breadcrumb">
            {crumbs.map((crumb, index) => (
              <span key={index} className="crumb">
                {index > 0 && <span className="crumb-sep" aria-hidden>/</span>}
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
