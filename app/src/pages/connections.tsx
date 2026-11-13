import { ChevronDown, Code2, FileJson, Plus, Server, Terminal } from "lucide-react"
import { useMemo, useState } from "react"
import { Link, useNavigate } from "react-router"
import { featured, matches, useCatalog, type FullCatalog } from "../connections/catalog"
import { Logo } from "../connections/logo"
import { CatalogGrid, SectionTitle, StatusLabel } from "../connections/parts"
import { ago } from "../lib/time"
import { api, usePolled, type Source } from "../server/connections"
import { PageHeader } from "../shell"
import { buttonAttrs, Card, CardHeader, Empty, Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger, Page, SearchInput, Skeleton } from "../ui"
import "../connections/connections.css"

const PER_CATEGORY = 6

function AddOwnMenu({ label = "Add your own" }: { label?: string }) {
  const navigate = useNavigate()
  return (
    <Menu onAction={(value) => navigate(`/connections/new?kind=${value}`)}>
      <MenuTrigger {...buttonAttrs("default", "sm")}>
        <Plus size={14} aria-hidden />
        {label}
      </MenuTrigger>
      <MenuContent align="end">
        <MenuItem value="mcp" icon={<Server size={14} />}>MCP server</MenuItem>
        <MenuItem value="api" icon={<Code2 size={14} />}>API</MenuItem>
        <MenuItem value="cli" icon={<Terminal size={14} />}>Command line tool</MenuItem>
        <MenuSeparator />
        <MenuItem value="import" icon={<FileJson size={14} />}>Paste a config</MenuItem>
      </MenuContent>
    </Menu>
  )
}

function CategoriesMenu({ catalog }: { catalog: FullCatalog }) {
  const navigate = useNavigate()
  return (
    <Menu onAction={(value) => navigate(`/connections/browse?category=${encodeURIComponent(String(value))}`)}>
      <MenuTrigger className="tab">
        Categories <ChevronDown size={12} aria-hidden />
      </MenuTrigger>
      <MenuContent align="end">
        <MenuItem value="all">All</MenuItem>
        <MenuSeparator />
        {catalog.categories.map((c) => (
          <MenuItem key={c} value={c}>
            {c}
          </MenuItem>
        ))}
      </MenuContent>
    </Menu>
  )
}

/** The catalog entry a connection was made from, matched by name, for its logo. */
function useLogoIds(catalog: FullCatalog | null) {
  return useMemo(() => new Map((catalog?.entries ?? []).map((e) => [e.name.toLowerCase(), e.id])), [catalog])
}

function YourConnections({ sources, logoIds }: { sources: Source[]; logoIds: Map<string, string> }) {
  return (
    <Card>
      <CardHeader title={`${sources.length} ${sources.length === 1 ? "connection" : "connections"}`} actions={<AddOwnMenu label="Add" />} />
      {sources.map((s) => (
        <Link key={s.id} to={`/connections/${s.id}`} className="yours-row" data-off={s.enabled ? undefined : ""}>
          <Logo id={logoIds.get(s.name.toLowerCase())} name={s.name} size={36} />
          <span className="row-text">
            <span className="row-label">{s.name}</span>
            <span className="yours-meta">
              <StatusLabel source={s} />
              <span className="sep">·</span>
              <span>{s.last_pull_at ? `Pulled ${ago(s.last_pull_at)}` : "Not pulled yet"}</span>
            </span>
          </span>
        </Link>
      ))}
    </Card>
  )
}

function ConnectionsPage() {
  const { catalog, error } = useCatalog()
  const sources = usePolled(api.connections, 10_000)
  const [query, setQuery] = useState("")
  const logoIds = useLogoIds(catalog)
  const connectedNames = useMemo(() => new Set((sources.data ?? []).map((s) => s.name.toLowerCase())), [sources.data])

  const results = useMemo(() => (catalog && query.trim() ? catalog.entries.filter((e) => matches(e, query)) : null), [catalog, query])

  return (
    <>
      <PageHeader title="Connections" actions={<AddOwnMenu />} />
      <Page width="explore">
        <div className="explore-head">
          <div>
            <h2 className="h1">Connections</h2>
            <p className="page-intro-description">Personal sources the agent reads and pulls from every four hours</p>
          </div>
          <SearchInput className="explore-search" placeholder="Search connections" value={query} onChange={(e) => setQuery(e.currentTarget.value)} aria-label="Search connections" />
        </div>

        <div className="explore-tabs">
          <div className="tab-row">
            <span className="tab" data-selected="">
              Explore{catalog && <span className="tab-count">{catalog.entries.length}</span>}
            </span>
            <Link className="tab" to="/connections/browse?category=all">
              All sources
            </Link>
          </div>
          {catalog && <CategoriesMenu catalog={catalog} />}
        </div>

        {results ? (
          <section className="section">
            <SectionTitle title={`${results.length} ${results.length === 1 ? "result" : "results"}`} />
            {results.length ? (
              <CatalogGrid entries={results} connectedNames={connectedNames} />
            ) : (
              <Empty title={`Nothing called “${query.trim()}” yet`} description="Add it yourself as an MCP server, an API or a command line tool">
                <AddOwnMenu />
              </Empty>
            )}
          </section>
        ) : (
          <>
            {sources.data && sources.data.length > 0 && (
              <section className="section">
                <SectionTitle title="Yours" />
                <YourConnections sources={sources.data} logoIds={logoIds} />
              </section>
            )}

            {error && <Empty title="Couldn't load the catalog" description={error.message} />}

            {!catalog && !error && (
              <section className="section">
                <Skeleton width={80} height={14} />
                <div className="catalog-grid">
                  {Array.from({ length: 6 }, (_, i) => (
                    <div className="catalog-row" key={i}>
                      <Skeleton width={40} height={40} />
                      <span className="catalog-row-text">
                        <Skeleton width={96} />
                        <Skeleton width="80%" />
                      </span>
                    </div>
                  ))}
                </div>
              </section>
            )}

            {catalog && (
              <>
                <section className="section">
                  <SectionTitle title="Featured" />
                  <CatalogGrid entries={featured(catalog)} connectedNames={connectedNames} />
                </section>
                {catalog.categories.map((category) => {
                  const entries = catalog.entries.filter((e) => e.category === category)
                  return (
                    <section className="section" key={category}>
                      <SectionTitle
                        title={category}
                        to={entries.length > PER_CATEGORY ? `/connections/browse?category=${encodeURIComponent(category)}` : undefined}
                        count={entries.length}
                      />
                      <CatalogGrid entries={entries.slice(0, PER_CATEGORY)} connectedNames={connectedNames} />
                    </section>
                  )
                })}
              </>
            )}

            <section className="section">
              <SectionTitle title="Add your own" />
              <div className="add-own">
                <Link className="add-own-card" to="/connections/new?kind=mcp">
                  <Logo name="MCP" kind="mcp" size={36} />
                  <span className="row-label">MCP server</span>
                  <span className="row-description">A remote server over HTTP, or one this Mac launches</span>
                </Link>
                <Link className="add-own-card" to="/connections/new?kind=api">
                  <Logo name="API" kind="api" size={36} />
                  <span className="row-label">API</span>
                  <span className="row-description">Any read-only JSON API, with your token in a header</span>
                </Link>
                <Link className="add-own-card" to="/connections/new?kind=cli">
                  <Logo name="CLI" kind="cli" size={36} />
                  <span className="row-label">Command line tool</span>
                  <span className="row-description">A tool on this Mac you've already installed and signed in to</span>
                </Link>
              </div>
            </section>
          </>
        )}
      </Page>
    </>
  )
}

export { AddOwnMenu, ConnectionsPage, useLogoIds }
