import { useMemo, useState } from "react"
import { ConnectionsSidebar } from "./sidebar"
import { Link, useSearchParams } from "react-router"
import { AddOwnMenu } from "../pages/connections"
import { api, usePolled } from "../server/connections"
import { PageHeader } from "../shell"
import { Empty, Page, SearchInput, Skeleton } from "../ui"
import { matches, useCatalog } from "./catalog"
import { CatalogGrid } from "./parts"
import "./connections.css"

function BrowsePage() {
  const [params] = useSearchParams()
  const category = params.get("category") ?? "all"
  const { catalog } = useCatalog()
  const sources = usePolled(api.connections)
  const [query, setQuery] = useState("")
  const connectedNames = useMemo(() => new Set((sources.data ?? []).map((s) => s.name.toLowerCase())), [sources.data])

  const entries = useMemo(
    () => (catalog?.entries ?? []).filter((e) => (category === "all" || e.category === category) && matches(e, query)).sort((a, b) => a.name.localeCompare(b.name)),
    [catalog, category, query],
  )
  const title = category === "all" ? "All sources" : category

  return (
    <>
      <ConnectionsSidebar />
      <PageHeader crumbs={[{ label: "Connections", to: "/connections" }, { label: title }]} actions={<AddOwnMenu />} />
      <Page width="explore">
        <div className="explore-head">
          <div>
            <h2 className="h1">{title}</h2>
            <p className="page-intro-description">{catalog ? `${entries.length} ${entries.length === 1 ? "source" : "sources"}` : " "}</p>
          </div>
          <SearchInput className="explore-search" placeholder={`Search ${title.toLowerCase()}`} value={query} onChange={(e) => setQuery(e.currentTarget.value)} aria-label="Search" />
        </div>

        {catalog && (
          <div className="explore-tabs">
            <div className="tab-row" style={{ flexWrap: "wrap" }}>
              {["all", ...catalog.categories].map((c) => (
                <Link key={c} className="tab" to={`/connections/browse?category=${encodeURIComponent(c)}`} aria-current={c === category ? "page" : undefined}>
                  {c === "all" ? "All" : c}
                </Link>
              ))}
            </div>
          </div>
        )}

        {!catalog ? (
          <Skeleton width="100%" height={240} />
        ) : entries.length ? (
          <CatalogGrid key={category} entries={entries} connectedNames={connectedNames} />
        ) : (
          <Empty title="Nothing here yet" description="Add it yourself as an MCP server, an API or a command line tool">
            <AddOwnMenu />
          </Empty>
        )}
      </Page>
    </>
  )
}

export { BrowsePage }
