import { request } from "./http"

type ProviderId = "vercel" | "codex" | "grok"
type Appearance = "system" | "light" | "dark"

type ModelChoice = { provider: ProviderId; model: string; effort: string | null }

type SetupProgress = { step: string; finished: boolean; tour_pending: boolean }

type Preferences = {
  model: ModelChoice | null
  mac_recording: boolean
  chunk_retention_days: number
  appearance: Appearance
  /** Null for installations from before in-app setup. */
  setup?: SetupProgress | null
  /** Monthly limit in US dollars; null means none. */
  spend_limit_usd?: number | null
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

type RunState = "running" | "done" | "failed" | "waiting" | "stopped" | "skipped"

type AgentRun = {
  id: number
  kind: "pass" | "setup" | "check"
  job_id: number | null
  window_start: number | null
  label: string | null
  provider: ProviderId | null
  model: string | null
  effort: string | null
  started_at: number
  finished_at: number | null
  state: RunState
  problem: string | null
  cost_usd: number | null
  input_tokens: number | null
  output_tokens: number | null
  revision_id: number | null
  steps: number
}

type AgentRunDetail = AgentRun & {
  log: { at: number; text: string }[]
  window_end: number | null
  job: { state: string; attempts: number; max_attempts: number; available_at: number; source_id: string | null } | null
}

type Upcoming = { job_id: number; window_start: number; available_at: number; catch_up: boolean; held: boolean; rerun: boolean }

type AgentRunsResponse = {
  runs: AgentRun[]
  next_before: number | null
  upcoming: Upcoming[]
  month_spend_usd: number
  spend_limit_usd: number | null
}

type LastResult = { id: number; at: number; state: RunState; problem: string | null; model: string | null } | null

type ProviderStatus = {
  id: ProviderId
  method: "key" | "signin"
  signed_in: boolean
  in_use: boolean
  last_check: LastResult
  last_run: LastResult
  signin: SigninState | null
}

type SigninState = "idle" | "waiting" | "done" | "failed"
type SigninStatus = { provider: ProviderId; state: SigninState; url?: string | null; signed_in: boolean }

type Permissions = { accessibility: boolean; screen: boolean; input: boolean; at: string }

type GuideStep = { id: string; state: string; detail?: string | null; changed_at: string }

type SetupStatus = {
  app: { installed: boolean; paired: boolean; paired_here: boolean; server: string | null; running: boolean }
  permissions: Permissions | null
  guide: { run: string; status: string; steps: GuideStep[] } | null
  pairing: { state: "done" | "failed"; error?: string } | null
  last_record_at: number | null
}

type ProofItem = { at: number; app: string | null; title: string | null; lines: string[] }

type CheckResult = { ok: boolean; message: string; problem: string | null; seconds?: number }

type Source = { id: string; name: string; description: string; enabled: boolean; status: string }
type Today = { entries_today: number; active_sources: number; last_pull_at: string | null }

const api = {
  settings: () => request<SettingsResponse>("/v1/settings"),
  saveSettings: (settings: Preferences) => request<{ settings: Preferences }>("/v1/settings", { method: "PUT", body: settings }),
  catalog: (provider: ProviderId) => request<Catalog>(`/v1/models?provider=${provider}`),
  checkModel: () => request<CheckResult>("/v1/model/check", { method: "POST" }),
  agentRuns: (query: { before?: number; kind?: "check" } = {}) => {
    const params = new URLSearchParams({ limit: "100" })
    if (query.before) params.set("before", String(query.before))
    if (query.kind) params.set("kind", query.kind)
    return request<AgentRunsResponse>(`/v1/agent-runs?${params}`)
  },
  agentRun: (id: number) => request<AgentRunDetail>(`/v1/agent-runs/${id}`),
  rerun: (id: number) => request<{ queued: boolean }>(`/v1/agent-runs/${id}/rerun`, { method: "POST" }),
  providers: () => request<{ providers: ProviderStatus[] }>("/v1/providers"),
  signinStart: (id: ProviderId) => request<SigninStatus>(`/v1/providers/${id}/signin`, { method: "POST" }),
  signinStatus: (id: ProviderId) => request<SigninStatus>(`/v1/providers/${id}/signin`),
  signinCancel: (id: ProviderId) => request<void>(`/v1/providers/${id}/signin`, { method: "DELETE" }),
  setupStatus: () => request<SetupStatus>("/v1/setup"),
  requestPermissions: () => request<{ run: string }>("/v1/setup/permissions", { method: "POST" }),
  proof: () => request<{ items: ProofItem[]; since: number }>("/v1/setup/proof"),
  pairElsewhere: (link: string) => request<{ queued: boolean }>("/v1/setup/pair-elsewhere", { method: "POST", body: { link } }),
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
export type {
  AgentRun,
  AgentRunDetail,
  AgentRunsResponse,
  CheckResult,
  GuideStep,
  LastResult,
  Permissions,
  ProofItem,
  ProviderStatus,
  RunState,
  SetupProgress,
  SetupStatus,
  SigninState,
  SigninStatus,
  Upcoming,
}
export type { ApiKey, Appearance, Catalog, CatalogModel, ModelChoice, Pass, Preferences, ProviderId, Pull, RunsResponse, SettingsResponse, Source, Today }
