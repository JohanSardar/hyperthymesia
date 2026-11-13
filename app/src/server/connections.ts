import { useCallback, useEffect, useRef, useState } from "react"
import { request } from "./http"

type Auth = "key" | "existing_login" | "none"
type CustomKind = "mcp" | "api" | "cli"

type ApiTemplate = {
  kind: "api"
  base_url: string
  probe_path: string
  method?: "GET" | "POST"
  headers?: Record<string, string>
  body?: unknown
}
type McpHttpTemplate = { kind: "mcp"; transport: "http"; url: string; headers?: Record<string, string> }
type McpStdioTemplate = { kind: "mcp"; transport: "stdio"; argv: string[]; env?: Record<string, string> }
type CliTemplate = { kind: "cli"; argv: string[]; env?: Record<string, string> }
type ConnectionTemplate = ApiTemplate | McpHttpTemplate | McpStdioTemplate | CliTemplate

type CatalogEntry = {
  id: string
  name: string
  category: string
  description: string
  auth: Auth
  requirements: string
  variables: string[]
  connection: ConnectionTemplate
}

type Catalog = {
  entries: CatalogEntry[]
  categories: string[]
  custom_kinds: CustomKind[]
  cadence_seconds: number
}

type SourceStatus = "setting_up" | "ready" | "needs_reconnect" | "failed" | (string & {})

type Source = {
  id: string
  name: string
  description: string
  connector_instance: string
  enabled: boolean
  status: SourceStatus
  history_days: number
  connected_at: string
  last_pull_at: string | null
  last_error: string | null
  has_key: boolean
  oauth: boolean
  setup_generation: number
}

type SetupRun = {
  generation: number
  status: "pending" | "succeeded" | "failed" | (string & {})
  result: { summary?: string; no_pull?: string | null; recipes?: string[]; [key: string]: unknown } | null
  last_error: string | null
  created_at: string
  finished_at: string | null
}

type Pull = {
  id: string
  source_id: string
  name: string
  recipe: { name?: string; kind?: string; request?: { method?: string; path?: string; url?: string; argv?: string[]; tool?: string; [key: string]: unknown }; [key: string]: unknown }
  enabled: boolean
  owner_off: boolean
  next_run_at: string | null
  last_ok_at: string | null
  last_error: string | null
}

type Addon = { id: string; source_id: string; name: string; text: string; enabled: boolean; owner_off: boolean }

type AgentConnection = { source: Source; setup: SetupRun | null; cadence_seconds: number; pulls: Pull[]; addons: Addon[] }

type ConnectBody = {
  catalog_id?: string
  connection?: ConnectionTemplate
  import_config?: unknown
  import_name?: string
  name?: string
  description?: string
  key?: string
  history_days?: number
  variables?: Record<string, string>
  cli_home?: string
}

type Envelope = {
  kind: string
  connector_instance: string
  source_key: string
  observation_id: string
  time: { observed_at: string; source_start?: string | null; source_end?: string | null }
  payload: unknown
  subject?: string | null
  resource?: string | null
  actor?: string | null
  device?: string | null
}

type LogEntry = { id: number; received_at: string; envelope: Envelope }

type TodayFigures = { entries_today: number; active_sources: number; last_pull_at: string | null; start: string; end: string }

const api = {
  catalog: () => request<Catalog>("/v1/connections/catalog"),
  connections: () => request<Source[]>("/v1/connections"),
  agent: () => request<{ connections: AgentConnection[] }>("/v1/agent"),
  today: () => request<TodayFigures>(`/v1/today?utc_offset_min=${-new Date().getTimezoneOffset()}`),
  connect: (body: ConnectBody) =>
    request<{ id: string; connector_instance: string; enabled: boolean; status: SourceStatus }>("/v1/connections", { method: "POST", body }),
  setEnabled: (id: string, enabled: boolean) => request<unknown>(`/v1/connections/${id}`, { method: "PATCH", body: { enabled } }),
  retrySetup: (id: string) => request<unknown>(`/v1/connections/${id}/setup`, { method: "POST", body: {} }),
  reconnect: (id: string, key?: string) => request<unknown>(`/v1/connections/${id}/reconnect`, { method: "POST", body: key ? { key } : {} }),
  setPullEnabled: (id: string, enabled: boolean) => request<unknown>(`/v1/recipes/${id}`, { method: "PATCH", body: { enabled } }),
  setAddonEnabled: (id: string, enabled: boolean) => request<unknown>(`/v1/addons/${id}`, { method: "PATCH", body: { enabled } }),
  retryPull: (id: string) => request<unknown>(`/v1/recipes/${id}/retry`, { method: "POST", body: {} }),
  entries: (id: string, before?: number) =>
    request<{ entries: { id: number; envelope: Envelope }[]; next_before: number | null }>(
      `/v1/connections/${id}/entries?limit=50${before ? `&before=${before}` : ""}`,
    ),
}

/** Loads a resource and re-reads it every `every` ms while the page is visible. */
function usePolled<T>(load: () => Promise<T>, every = 0) {
  const [data, setData] = useState<T | undefined>(undefined)
  const [error, setError] = useState<Error | null>(null)
  const loadRef = useRef(load)
  loadRef.current = load

  const refresh = useCallback(async () => {
    try {
      const next = await loadRef.current()
      setData(next)
      setError(null)
      return next
    } catch (e) {
      setError(e as Error)
      return undefined
    }
  }, [])

  useEffect(() => {
    void refresh()
    if (!every) return
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void refresh()
    }, every)
    return () => window.clearInterval(timer)
  }, [refresh, every])

  return { data, error, refresh, setData }
}

/**
 * The live entry log. The server sends the latest 100 entries first, then every new one as it
 * commits. EventSource resumes from the last event ID after a dropped connection.
 */
function useEntryStream(onEntries: (entries: LogEntry[], initial: boolean) => void) {
  const [state, setState] = useState<"connecting" | "live" | "offline">("connecting")
  const handler = useRef(onEntries)
  handler.current = onEntries

  useEffect(() => {
    let first = true
    const source = new EventSource("/v1/entries/stream", { withCredentials: true })
    source.onopen = () => setState("live")
    source.onerror = () => setState(source.readyState === EventSource.CLOSED ? "offline" : "connecting")
    source.addEventListener("entries", (event) => {
      const data = JSON.parse((event as MessageEvent<string>).data) as { entries: LogEntry[] }
      handler.current(data.entries, first)
      first = false
      setState("live")
    })
    return () => source.close()
  }, [])

  return state
}

export { api, useEntryStream, usePolled }
export type {
  AgentConnection,
  Addon,
  ApiTemplate,
  Auth,
  Catalog,
  CatalogEntry,
  CliTemplate,
  ConnectBody,
  ConnectionTemplate,
  CustomKind,
  Envelope,
  LogEntry,
  McpHttpTemplate,
  McpStdioTemplate,
  Pull,
  SetupRun,
  Source,
  SourceStatus,
  TodayFigures,
}
