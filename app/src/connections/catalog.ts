import { useEffect, useState } from "react"
import { api, type Catalog, type CatalogEntry } from "../server/connections"
import { blurb } from "./blurbs"
import { extraCatalog } from "./extra-catalog"

type FullCatalog = { entries: CatalogEntry[]; categories: string[]; byId: Map<string, CatalogEntry> }

const FEATURED = ["gmail-gws", "slack", "notion", "x-github", "spotify", "oura", "linear", "apple-bridge"]
const CATEGORY_ORDER = ["Mail", "Calendar", "Chat", "Notes", "Tasks", "Reading", "Files", "Code", "Work", "Money", "Health", "Music", "Media", "Activity", "Design", "Cloud", "Apple"]

let cached: Promise<FullCatalog> | null = null

function merge(server: Catalog): FullCatalog {
  const names = new Set(server.entries.map((e) => e.name.toLowerCase()))
  const entries = [...server.entries, ...extraCatalog.filter((e) => !names.has(e.name.toLowerCase()))]
  const present = new Set(entries.map((e) => e.category))
  const categories = [...CATEGORY_ORDER.filter((c) => present.has(c)), ...[...present].filter((c) => !CATEGORY_ORDER.includes(c)).sort()]
  return { entries, categories, byId: new Map(entries.map((e) => [e.id, e])) }
}

function loadCatalog() {
  cached ??= api.catalog().then(merge).catch((error) => {
    cached = null
    throw error
  })
  return cached
}

function useCatalog() {
  const [catalog, setCatalog] = useState<FullCatalog | null>(null)
  const [error, setError] = useState<Error | null>(null)
  useEffect(() => {
    let live = true
    loadCatalog().then((c) => live && setCatalog(c), (e: Error) => live && setError(e))
    return () => {
      live = false
    }
  }, [])
  return { catalog, error }
}

function featured(catalog: FullCatalog) {
  return FEATURED.map((id) => catalog.byId.get(id)).filter((e): e is CatalogEntry => Boolean(e))
}

function matches(entry: CatalogEntry, query: string) {
  const q = query.trim().toLowerCase()
  if (!q) return true
  return [entry.name, entry.category, blurb(entry), entry.description, entry.connection.kind].some((field) => field.toLowerCase().includes(q))
}

/** How the connection reaches the source, in a person's words. */
function reach(entry: CatalogEntry) {
  const c = entry.connection
  if (c.kind === "cli") return { label: "Command line", detail: c.argv[0] }
  if (c.kind === "mcp") return { label: c.transport === "stdio" ? "Local MCP server" : "Remote MCP server", detail: c.transport === "http" ? c.url : c.argv[0] }
  return { label: "API", detail: c.base_url.replace(/\{\{(\w+)\}\}/g, "<$1>") }
}

function authLabel(entry: CatalogEntry) {
  if (entry.auth === "key") return "Read-only token"
  if (entry.auth === "existing_login") return "Your existing sign-in"
  return "No sign-in"
}

export { authLabel, featured, matches, reach, useCatalog }
export type { FullCatalog }
