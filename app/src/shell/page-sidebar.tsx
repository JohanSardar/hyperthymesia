import { useEffect, type ReactNode } from "react"
import { createPortal } from "react-dom"
import { useSidebar } from "./sidebar-state"

/**
 * A page's own sidebar content. Render it anywhere in a page; it shows in the
 * shell's sidebar under the page title. Without one, the sidebar lists pages.
 */
function PageSidebar({ children }: { children: ReactNode }) {
  const { slot, setFilled } = useSidebar()
  useEffect(() => {
    setFilled(true)
    return () => setFilled(false)
  }, [setFilled])
  return slot ? createPortal(children, slot) : null
}

/** A labelled group of rows inside PageSidebar. */
function SidebarGroup({ label, actions, children }: { label?: ReactNode; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="sidebar-group">
      {(label || actions) && (
        <div className="sidebar-group-head">
          {label && <span className="sidebar-group-label">{label}</span>}
          {actions}
        </div>
      )}
      {children}
    </div>
  )
}

/** One-line empty text for a sidebar list ("No saved searches yet"). */
function SidebarEmpty({ children }: { children: ReactNode }) {
  return <p className="sidebar-empty">{children}</p>
}

export { PageSidebar, SidebarEmpty, SidebarGroup }
