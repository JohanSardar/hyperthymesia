import { useEffect, useMemo, useRef, useState, type ReactNode } from "react"
import { createPortal } from "react-dom"
import { NavLink, useLocation, useNavigate, useParams, useSearchParams } from "react-router"
import { useSettings } from "../server/settings"
import { PageHeader, PageSidebar, SidebarEmpty, SidebarGroup } from "../shell"
import { AgentSettingsPage, ConnectionsSettingsPage } from "../settings/agent"
import { pageById, pages, searchSettings, type SettingsPageId } from "../settings/catalog"
import { AppearancePage, NotificationsPage, ShortcutsPage } from "../settings/look"
import { RecordingPage } from "../settings/recording"
import { ServerPage } from "../settings/server"
import { StoragePage } from "../settings/storage"
import { AboutPage, AdvancedPage } from "../settings/system"
import { Card, Empty, Page, Row, SearchInput, Skeleton } from "../ui"
import "./settings.css"

type PageProps = { actionsSlot: (node: ReactNode) => ReactNode }

const bodies: Record<SettingsPageId, (props: PageProps) => ReactNode> = {
  recording: RecordingPage,
  agent: AgentSettingsPage,
  connections: ConnectionsSettingsPage,
  storage: StoragePage,
  server: ServerPage,
  appearance: AppearancePage,
  notifications: NotificationsPage,
  shortcuts: ShortcutsPage,
  advanced: AdvancedPage,
  about: AboutPage,
}

/** Words around the first match, so a hit in a long explanation shows why it matched. */
function snippet(text: string, query: string) {
  const word = query.toLowerCase().split(/\s+/).find(Boolean) ?? ""
  const at = text.toLowerCase().indexOf(word)
  if (at < 0 || text.length <= 90) return text
  const start = Math.max(0, at - 30)
  return `${start ? "…" : ""}${text.slice(start, start + 90)}${start + 90 < text.length ? "…" : ""}`
}

function SettingsSidebar({ query, setQuery, current }: { query: string; setQuery: (q: string) => void; current: SettingsPageId }) {
  const navigate = useNavigate()
  const [params, setParams] = useSearchParams()
  const input = useRef<HTMLInputElement>(null)
  const results = useMemo(() => searchSettings(query), [query])
  const groups = [...new Set(pages.map((p) => p.group))]
  // ⇧⌘F (rebindable) lands here.
  useEffect(() => {
    if (params.get("search")) {
      input.current?.focus()
      params.delete("search")
      setParams(params, { replace: true })
    }
  }, [params, setParams])
  return (
    <PageSidebar>
      <div className="settings-search">
        <SearchInput
          ref={input}
          size="sm"
          placeholder="Search settings"
          aria-label="Search settings"
          value={query}
          onChange={(e) => setQuery(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && results[0]) navigate(`/settings/${results[0].page.id}#${results[0].item.id}`)
            if (e.key === "Escape") setQuery("")
          }}
        />
      </div>
      {query.trim() ? (
        <SidebarGroup label={`${results.length} ${results.length === 1 ? "match" : "matches"}`}>
          {results.length === 0 && <SidebarEmpty>No setting matches “{query.trim()}”</SidebarEmpty>}
          {results.map(({ item, page }) => (
            <NavLink key={item.id} to={`/settings/${page.id}#${item.id}`} className="sidebar-row settings-hit">
              <span className="settings-hit-text">
                <span className="sidebar-row-label">{item.label}</span>
                <span className="settings-hit-meta">
                  {page.label} · {snippet(item.description, query)}
                </span>
              </span>
            </NavLink>
          ))}
        </SidebarGroup>
      ) : (
        groups.map((group) => (
          <SidebarGroup key={group} label={group}>
            {pages
              .filter((p) => p.group === group)
              .map((p) => {
                const Icon = p.icon
                return (
                  <NavLink key={p.id} to={`/settings/${p.id}`} className="sidebar-row" aria-current={p.id === current ? "page" : undefined} data-active={p.id === current ? "" : undefined}>
                    <Icon aria-hidden size={15} strokeWidth={1.6} />
                    <span className="sidebar-row-label">{p.label}</span>
                  </NavLink>
                )
              })}
          </SidebarGroup>
        ))
      )}
    </PageSidebar>
  )
}

function SettingsPage() {
  const { page: param } = useParams()
  const { hash } = useLocation()
  const navigate = useNavigate()
  const { data, error } = useSettings()
  const [query, setQuery] = useState("")
  const [actions, setActions] = useState<HTMLElement | null>(null)
  const id: SettingsPageId = param && pageById.has(param as SettingsPageId) ? (param as SettingsPageId) : "recording"
  const page = pageById.get(id)!
  const Body = bodies[id]

  useEffect(() => {
    if (param && !pageById.has(param as SettingsPageId)) navigate("/settings", { replace: true })
  }, [param, navigate])

  // A search hit opens its page and shows the row.
  useEffect(() => {
    if (!hash || !data) return
    const row = document.getElementById(`setting-${decodeURIComponent(hash.slice(1))}`)
    if (!row) return
    row.scrollIntoView({ block: "center", behavior: "smooth" })
    row.dataset.highlight = ""
    const timer = window.setTimeout(() => delete row.dataset.highlight, 2200)
    return () => window.clearTimeout(timer)
  }, [hash, id, data])

  const actionsSlot = (node: ReactNode) => (actions ? createPortal(node, actions) : null)

  return (
    <>
      <PageHeader crumbs={[{ label: "Settings", to: "/settings" }, { label: page.label }]} />
      <SettingsSidebar query={query} setQuery={setQuery} current={id} />
      <Page width="settings" className="settings-page">
        <div className="page-intro">
          <div>
            <p className="settings-for">Settings for this server</p>
            <h2 className="h1">{page.label}</h2>
            <p className="page-intro-description">{page.description}</p>
          </div>
          <div className="page-intro-actions" ref={setActions} />
        </div>
        {!data ? (
          error ? (
            <Empty title="Settings didn't load" description={error} />
          ) : (
            <Card>
              {[0, 1, 2].map((i) => (
                <Row key={i} label={<Skeleton width={140} />} description={<Skeleton width={260} />} />
              ))}
            </Card>
          )
        ) : (
          <Body key={id} actionsSlot={actionsSlot} />
        )}
      </Page>
    </>
  )
}

export { SettingsPage }
