import { Plus } from "lucide-react"
import { NavLink, useParams } from "react-router"
import { ago } from "../lib/time"
import { api, usePolled } from "../server/connections"
import { PageSidebar, SidebarEmpty, SidebarGroup } from "../shell"
import { Skeleton, StatusDot } from "../ui"
import { connectionStatus } from "./status"

/** The Connections pages' sidebar: your sources, each with its status and last pull. */
function ConnectionsSidebar() {
  const { id } = useParams()
  const sources = usePolled(api.connections, 15_000)
  const list = sources.data
  return (
    <PageSidebar>
      <SidebarGroup label="Yours">
        {!list && <Skeleton width="70%" />}
        {list?.length === 0 && <SidebarEmpty>Nothing connected yet</SidebarEmpty>}
        {list?.map((s) => {
          const status = connectionStatus(s, null)
          return (
            <NavLink key={s.id} to={`/connections/${s.id}`} className="sidebar-row" data-active={s.id === id ? "" : undefined} title={status.label}>
              <StatusDot tone={status.tone} />
              <span className="sidebar-row-label">{s.name}</span>
              <span className="sidebar-row-meta">{s.enabled ? (s.last_pull_at ? ago(s.last_pull_at) : "—") : "Off"}</span>
            </NavLink>
          )
        })}
      </SidebarGroup>
      <SidebarGroup label="Add">
        <NavLink to="/connections" end className="sidebar-row">
          <Plus aria-hidden size={15} strokeWidth={1.6} />
          <span className="sidebar-row-label">Browse sources</span>
        </NavLink>
        <NavLink to="/connections/new?kind=mcp" className="sidebar-row">
          <Plus aria-hidden size={15} strokeWidth={1.6} />
          <span className="sidebar-row-label">Add your own</span>
        </NavLink>
      </SidebarGroup>
    </PageSidebar>
  )
}

export { ConnectionsSidebar }
