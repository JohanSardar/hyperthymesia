import { useSettings } from "../server/settings"
import { ArrowRight, Check } from "lucide-react"
import type { ReactNode } from "react"
import { Link } from "react-router"
import type { CatalogEntry, SetupRun, Source } from "../server/connections"
import { StatusDot } from "../ui"
import { blurb } from "./blurbs"
import { Logo } from "./logo"
import { connectionStatus } from "./status"

function kindLabel(entry: CatalogEntry) {
  const c = entry.connection
  return c.kind === "mcp" ? "MCP" : c.kind === "cli" ? "CLI" : "API"
}

function CatalogRow({ entry, index = 0, connected }: { entry: CatalogEntry; index?: number; connected?: boolean }) {
  return (
    <Link to={`/connections/catalog/${entry.id}`} className="catalog-row" style={{ ["--i" as string]: index }}>
      <Logo id={entry.id} name={entry.name} />
      <span className="catalog-row-text">
        <span className="catalog-row-name">
          <span className="row-label">{entry.name}</span>
          {entry.connection.kind !== "api" && <span className="kind-chip">{kindLabel(entry)}</span>}
        </span>
        <span className="catalog-row-description">{blurb(entry)}</span>
      </span>
      {connected && (
        <span className="connected-mark">
          <Check size={12} aria-hidden /> Added
        </span>
      )}
    </Link>
  )
}

function CatalogGrid({ entries, connectedNames }: { entries: CatalogEntry[]; connectedNames?: Set<string> }) {
  return (
    <div className="catalog-grid">
      {entries.map((entry, i) => (
        <CatalogRow key={entry.id} entry={entry} index={i} connected={connectedNames?.has(entry.name.toLowerCase())} />
      ))}
    </div>
  )
}

function SectionTitle({ title, to, count }: { title: ReactNode; to?: string; count?: number }) {
  return (
    <div className="section-head">
      <h3 className="h2">{title}</h3>
      {to && (
        <Link to={to} className="view-all">
          View all{count ? ` ${count}` : ""} <ArrowRight size={14} aria-hidden />
        </Link>
      )}
    </div>
  )
}

function StatusLabel({ source, setup }: { source: Source; setup?: SetupRun | null }) {
  const status = connectionStatus(source, setup)
  return (
    <span className="status-label" data-tone={status.tone} data-busy={status.busy ? "" : undefined}>
      <StatusDot tone={status.tone} />
      <span>{status.label}</span>
    </span>
  )
}

export { CatalogGrid, CatalogRow, kindLabel, SectionTitle, StatusLabel }

/** How far back a first pull reaches; the default comes from Settings › Connections. */
const HISTORY = [
  { value: "0", label: "Only new entries" },
  { value: "1", label: "The last day" },
  { value: "3", label: "The last 3 days" },
  { value: "7", label: "The last week" },
  { value: "14", label: "The last 2 weeks" },
  { value: "30", label: "The last 30 days" },
]
function useDefaultHistory() {
  return String(useSettings().data?.settings.connection_history_days ?? 1)
}
export { HISTORY, useDefaultHistory }
