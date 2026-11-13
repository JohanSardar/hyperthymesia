import { request } from "./http"

type ProviderId = "vercel" | "codex" | "grok"
type Appearance = "system" | "light" | "dark"

type ModelChoice = { provider: ProviderId; model: string; effort: string | null }

type Preferences = {
  model: ModelChoice | null
  mac_recording: boolean
  chunk_retention_days: number
  appearance: Appearance
}

type ApiKey = { id: string; name: string; prefixes: string[]; scopes: string[]; revoked: boolean }

type SettingsResponse = {
  settings: Preferences
  gateway_key_configured: boolean
  storage: { state_dir: string; published_blob_bytes: number; automatic_gc: boolean }
  api_keys: ApiKey[]
}

type CatalogModel = { id: string; name?: string; efforts?: string[] }

type Catalog = {
  provider: ProviderId
  models: CatalogModel[]
  error: string | null
  message?: string
  allows_custom_model: boolean
}

type Pass = {
  id: number
  kind: string
  state: string
  window_start: number
  generation: number
  attempts: number
  available_at: number | null
  lease_until: number | null
  has_error: boolean
  revision_id: string | null
}

type Pull = {
  id: number
  recipe_id: string
  source_id: string
  started_at: number
  finished_at: number | null
  status: string
  accepted: number | null
  duplicates: number | null
  rejected: number | null
}

type RunsResponse = {
  passes: Pass[]
  pulls: Pull[]
  next_pass_before: number | null
  next_pull_before: number | null
}

type Source = { id: string; name: string; description: string; enabled: boolean; status: string }
type Today = { entries_today: number; active_sources: number; last_pull_at: string | null }

const api = {
  settings: () => request<SettingsResponse>("/v1/settings"),
  saveSettings: (settings: Preferences) => request<{ settings: Preferences }>("/v1/settings", { method: "PUT", body: settings }),
  catalog: (provider: ProviderId) => request<Catalog>(`/v1/models?provider=${provider}`),
  checkModel: () => request<{ ok: boolean; message: string }>("/v1/model/check", { method: "POST" }),
  saveGatewayKey: (key: string) => request<void>("/v1/settings/gateway-key", { method: "PUT", body: { key } }),
  removeGatewayKey: () => request<void>("/v1/settings/gateway-key", { method: "DELETE" }),
  createKey: (body: { name: string; prefixes: string[]; scopes: string[] }) =>
    request<{ token: string; id: string }>("/v1/settings/tokens", { method: "POST", body }),
  revokeKey: (id: string) => request<void>(`/v1/settings/tokens/${encodeURIComponent(id)}`, { method: "DELETE" }),
  runs: (cursor: { pass_before?: number; pull_before?: number } = {}) => {
    const query = new URLSearchParams({ limit: "50" })
    if (cursor.pass_before) query.set("pass_before", String(cursor.pass_before))
    if (cursor.pull_before) query.set("pull_before", String(cursor.pull_before))
    return request<RunsResponse>(`/v1/runs?${query}`)
  },
  sources: async () => {
    const data = await request<Source[] | { sources: Source[] }>("/v1/sources")
    return Array.isArray(data) ? data : data.sources
  },
  today: () => request<Today>(`/v1/today?utc_offset_min=${-new Date().getTimezoneOffset()}`),
}

export { api }
export type { ApiKey, Appearance, Catalog, CatalogModel, ModelChoice, Pass, Preferences, ProviderId, Pull, RunsResponse, SettingsResponse, Source, Today }
