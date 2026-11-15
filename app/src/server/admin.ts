import { request } from "./http"

type RecordingState = "recording" | "paused" | "off" | "quiet"

type DeviceName = { device: string; records: number; first: string | null; last: string | null }
type Device = {
  device: string
  name: string | null
  records: number
  first: string | null
  last: string | null
  recording: RecordingState
  /** Every recorded name shown as this device, itself included. */
  names: DeviceName[]
}
type PairCode = { id: string; name: string; expires_at: string; used_at: string | null; revoked: boolean }

type NeverRecord = { apps: { bundle_id: string; name: string }[]; sites: string[] }
type Recording = {
  enabled: boolean
  paused_until: string | null
  never_record: NeverRecord
  changed_at: string | null
  profile?: string
}

type StorageType = { type: string; records: number; bytes: number; first: string | null; extra: { files?: number; file_bytes?: number; text_read?: number } }
type Storage = {
  types: StorageType[]
  measured_at: string
  databases: number
  disk_free: number | null
  state_dir: string
  backups_dir: string
  last_clean_up: { at: string; backup?: string; removed?: number } | null
}

type Backup = { name: string; reason: string; at: string | null; complete: boolean; bytes: number }
type Backups = {
  dir: string
  backups: Backup[]
  set_aside: { name: string; at: string; path: string }[]
  restore: { at: string; action: string; backup: string; ok: boolean; error?: string; previous_state?: string } | null
}

type Removed = {
  records: number
  screen_text: number
  screen_pictures: number
  text_read_from_pictures: number
  apps_and_windows: number
  activity: number
  connection_records: number
  files: number
  file_bytes: number
  write_ups: number
  windows: number
  lookups: number
  first: string | null
  last: string | null
}
type ErasePlan = { plan: Removed; agent_tables: { name: string; rows: number }[]; disk_free: number | null }
type LogEntry = {
  at: string
  action: "deleted_range" | "cleaned_up" | "clean_up_skipped" | "restored" | "restore_failed" | "backup_deleted" | (string & {})
  start?: string
  end?: string
  backup?: string
  removed?: Removed
  types?: Record<string, { older_than_days: number; removed: Removed }>
  error?: string
  previous_state?: string
  agent_tables_cleared?: string[]
}

type ServerInfo = {
  version: string
  address: string | null
  local_url: string
  started_at: string | null
  supervised: boolean
  state_dir: string
  log_file: string | null
  jobs: { kind: string; state: string; count: number }[]
  running_passes: number
  last_record_at: string | null
  remote: { url: string | null; tailscale: string | null }
}

type RepairStep = { step: string; ok: boolean; detail: string }

const admin = {
  devices: () => request<{ devices: Device[]; pairings: PairCode[] }>("/v1/devices"),
  rename: (device: string, name: string | null) => request<unknown>(`/v1/devices/${encodeURIComponent(device)}`, { method: "PUT", body: { name } }),
  merge: (devices: string[], into: string) => request<unknown>("/v1/devices/merge", { method: "POST", body: { devices, into } }),
  unmerge: (device: string) => request<void>(`/v1/devices/${encodeURIComponent(device)}/unmerge`, { method: "POST" }),
  pair: (name: string, reach: "local" | "remote") =>
    request<{ link: string; server: string; expires_at: string }>("/v1/devices/pair", { method: "POST", body: { name, reach } }),
  recording: () => request<Recording>("/v1/recording"),
  pause: (until: string | null) => request<Recording>("/v1/recording/pause", { method: "PUT", body: { until } }),
  neverRecord: (rules: NeverRecord) => request<Recording>("/v1/recording/never-record", { method: "PUT", body: rules }),
  storage: (fresh = false) => request<Storage>(`/v1/storage${fresh ? "?fresh=1" : ""}`),
  cleanUp: () => request<LogEntry & { nothing_set?: boolean }>("/v1/storage/clean-up", { method: "POST" }),
  backups: () => request<Backups>("/v1/backups"),
  backupNow: () => request<{ name: string; path: string }>("/v1/backups", { method: "POST" }),
  deleteBackup: (name: string) => request<void>(`/v1/backups/${encodeURIComponent(name)}`, { method: "DELETE" }),
  restore: (name: string) => request<{ restarting: boolean }>(`/v1/backups/${encodeURIComponent(name)}/restore`, { method: "POST" }),
  erasePlan: (start: string, end: string) => request<ErasePlan>("/v1/erase/plan", { method: "POST", body: { start, end } }),
  erase: (start: string, end: string, agentTables: boolean) =>
    request<LogEntry>("/v1/erase", { method: "POST", body: { start, end, confirm: "delete", agent_tables: agentTables } }),
  eraseLog: () => request<{ entries: LogEntry[]; path: string }>("/v1/erase/log"),
  exportUrl: (from: string, to: string) => `/v1/export?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`,
  server: () => request<ServerInfo>("/v1/server"),
  logs: () => request<{ path: string | null; lines: string[] }>("/v1/server/logs"),
  repair: () => request<{ steps: RepairStep[] }>("/v1/server/repair", { method: "POST" }),
  restart: () => request<{ restarting: boolean }>("/v1/server/restart", { method: "POST" }),
}

/** Waits until the server answers again after a restart. */
async function waitForServer(timeoutMs = 120_000) {
  const until = Date.now() + timeoutMs
  await new Promise((r) => setTimeout(r, 1500))
  while (Date.now() < until) {
    try {
      const response = await fetch("/healthz", { cache: "no-store" })
      if (response.ok) return true
    } catch {
      // Still restarting.
    }
    await new Promise((r) => setTimeout(r, 1000))
  }
  return false
}

export { admin, waitForServer }
export type { Backup, Backups, Device, DeviceName, ErasePlan, LogEntry, NeverRecord, PairCode, Recording, RecordingState, Removed, RepairStep, ServerInfo, Storage, StorageType }
