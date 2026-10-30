# Hyperthymesia — Design & Build Plan (v3, consensus)

Status: **consensus plan** (2026-10-30). Agreed between Claude (author) and Astra (adversarial reviewer, GPT-6 Astra) over two review rounds. The debate is in [`docs/review/`](review/). Nothing has been measured yet; every performance figure is a target until Gate A reports.

---

## 0. Goal and fixed decisions (set by the user; not debated)

**Goal:** an exact, *meaningful* replica of every day. Not "was on Chrome" but "spent 22 min comparing standing desks across the laptop and phone, then ordered one". Health and location get added later on the same timeline.

- Devices do lightweight capture of everything possible. **All data goes to one server, and all processing (including OCR) happens there.**
- **Modular core:** no source-specific logic. Every source and output is a connector (process / MCP / API).
- LLM interpretation every **4 h**. No human review loop. No LLM budget cap.
- Interpretations are kept forever. Raw screen video is kept ~30 days, then keyframes plus text.
- Phone is app-level in v1. Output in v1 is an **API plus a basic web dashboard**. The server host is undecided.
- **Rust** (functional style), chosen for speed and for safety when coding agents work on it.

## 1. Ground rules (the debate's main outcomes)

1. **Honesty over false precision.** Second-resolution timestamps don't prove second-accurate human activity. The system separates:
   - **device state**: on, asleep, locked, agent health
   - **input activity**
   - **foreground exposure**: what was on screen and focused
   - **inferred engagement**: what you were actually doing, which is an LLM inference
   - **coverage**: what was observed at all
   
   Durations are computed deterministically over observed intervals, at the source's recorded resolution, with explicit coverage gaps and timing uncertainty. `unknown` and `unobserved` are first-class values. Unsupported cases produce **abstention, not invention**.
2. **Record now, compute later.** From the first capture, record all provenance that's available at observation time (clock domains, boot IDs, sequences, native IDs, revisions, coverage, gap reasons) and explicitly mark fields that aren't available. Sophisticated estimation can wait (attention weights, calibrated error bars, verifier models, "as known then" replays). **The minimum needed to avoid false claims, unauthorized disclosure, or irreversible evidence loss is never deferred.**
3. **Instrument the app, not the pixels.** Meaning comes first from semantic sources (agent sessions, browser, editor, shell, git, calendar), then from OS signals, then from accessibility text. Screen pixels are universal *evidence and fallback*, never the backbone. The day must still be described correctly when screen capture lapses.
4. **Lossless evidence.** Raw observations are immutable. Every derived layer can be rebuilt and is versioned. Short actions are never merged away; "glance" is an interpretation, not a deletion.

---

## 2. Architecture

```
 DEVICE (thin: capture · policy · sample · buffer · transport-encode)
   hx-agent (Rust .app) ── focus/AX/input/presence, SCK → HEVC replay + sparse OCR-grade stills
   companions ─────────── browser ext (native messaging) · editor ext · shell hook
   SQLite outbox (capped, prioritized) ──▶ uploader (zstd NDJSON + resumable blobs)
                                   │  Tailscale only
 SERVER                            ▼
   Ingest: authenticate producer namespace → commit raw L0 (+ blobs) → ack
   Async jobs (Postgres queue: leases, attempts, idempotent outputs, quarantine):
     normalize L0→L1 ─▶ OCR / keyframe processor ─▶ compile L1→L2 interval index
     ─▶ interpret (4 h) ─▶ publish L3 revision ─▶ embed
   Pull connectors (cursor/backfill/checkpoint): T3, Codex, Claude Code, git, Apple Bridge, Oura…
   Surfaces: REST /v1 · MCP · basic dashboard (Rust templates + htmx)
```

### Data layers
| Layer | Content | Notes |
|---|---|---|
| L0 | raw envelopes, as received | immutable; unpartitioned table at first; global dedup key |
| L0 blobs | HEVC replay chunks, OCR-grade stills | content-addressed, checksummed, atomically published; retention-managed |
| L1 | normalized observations (generic facts / spans / entities / relationships + extension payload) | versioned normalizers; failures quarantined, L0 kept |
| L2 | **lossless interval index**: per-source lanes on a global time axis, with references to every event inside each interval | deterministic; prompt packets are a *separate projection* |
| L3 | episodes, tasks (as **interval sets**), optional goal hypotheses, day summaries: structured claims with evidence and provenance | revisions, published atomically; superseded versions kept |

Store: **Postgres** (data, FTS, pgvector, job queue) plus content-addressed blob files. No Kafka, ClickHouse or Timescale until a benchmark justifies one.

---

## 3. Event and connector contract (frozen as v1 in Gate A)

### 3.1 Envelope v1: semantics, not just fields
- **Identity:**
  - `envelope_version`
  - `observation_id` (immutable, persisted before upload)
  - `connector_instance` (installation / account / stream, authenticated against allowed namespaces)
  - `source_key` (native object or event key)
  - `revision` (source revision or content digest)
  - `operation` (`observe | upsert | delete`)
  - `sequence` or `cursor` / `generation`
  
  Dedup key: `(connector_instance, source_key, revision)`.
- **Time:**
  - `source_time` and its resolution
  - `observed_time` (collection)
  - `server_received_time`
  - `device_id`, `boot_id`, `clock_domain`
  - monotonic capture time (`mach_continuous_time` domain)
  - reference to the latest clock-calibration sample, with error bound
  - timezone offset at capture
  
  Intervals are half-open. Durations within a boot come from the monotonic domain. Wall-clock steps split the mapping; unrelated clocks are never subtracted.
- **Semantics:**
  - `kind` + `schema_uri` / version
  - `payload`
  - `blobs[]`
  - actor / subject / resource
  - session or causal links (e.g. the T3 thread ↔ provider session ↔ tool call)
  - collection scope and completeness
- **Policy:** a sensitivity class. Unknown data defaults to `restricted`. Producer labels can never *grant* disclosure.

### 3.2 Connector protocol (independent of transport)
`describe_capabilities` → `backfill(snapshot scope)` → `changes(cursor)` → `checkpoint` (acknowledged only after durable storage) → `coverage` → `health`.
- Incomplete scans are reported as incomplete; missing rows are never read as deletions.
- Transports: HTTP push (default), scheduled pull, MCP-source adapter (structured content). MCP alone isn't treated as a complete change feed.
- WASM/Extism and a marketplace are deferred.

### 3.3 Coverage and health
- Collectors emit per-collector status: last sequence, last observed time, queue age, drop/overflow counters and lifecycle errors.
- Unexplained gaps become `coverage_unknown` until reconciled; crashes are stored as a bounded interval.
- Metadata health keeps running while content capture is paused.

---

## 4. Device capture (macOS)

### 4.1 Signals
| Signal | Mechanism | Default |
|---|---|---|
| Focus, launch, quit, space | `NSWorkspace` notifications | on |
| Window and title changes | `AXObserver` (unsupported notifications reported) | on |
| Device state | sleep/wake, lock, display, power, thermal | on |
| Input activity | `CGEventTap` listen-only → 1 s buckets; callback does no AX and no I/O, only a bounded handoff | on |
| Click context | AX hit-test sampled *after* the click; labeled as an observation at its own sample time | on |
| Native app text | bounded AX extraction (time and node deadlines via `AXUIElementSetMessagingTimeout`); **never force Chromium/Electron AX** | on |
| Replay video | ScreenCaptureKit (drops idle frames) → VideoToolbox HEVC, low bitrate, 60 s chunks | on |
| OCR-grade stills | sparse stills of the foreground context on focus change plus periodic sampling while content changes, **under a configured cadence/byte budget**; skipped captures recorded as coverage limits; kept even when text exists | on |
| OCR | **none on device**; server only | — |
| Raw keystrokes | never | — |
| Audio | not in v1 | — |

### 4.2 Execution model
- The main thread owns the Cocoa/CFRunLoop.
- The event tap runs on its own run loop.
- ScreenCaptureKit delivers frames on dispatch queues.
- AX work runs on worker threads with deadlines.
- Tokio handles I/O and upload.

Every collector has overflow counters, cancellation, restart/re-registration on wake and on permission changes, and explicit missing-data output.

### 4.3 Companions
- **Browser extension (MV3):** correlates window focus, OS focus, the active tab, document identity, visibility, navigation including SPA route changes, and bounded viewport/content snapshots with truncation metadata. Its outbox persists across service-worker restarts. It talks to the agent through **native messaging** with an origin allowlist.
- **Editor extension** and **zsh hook:** use the same rigor. Shell argv is treated as potentially sensitive, and the hook doesn't claim coverage of agent or noninteractive commands.

### 4.4 Outbox, privacy, packaging
- **Outbox:** a SQLite outbox with separate budgets for high-value events vs. bulk media. Eviction emits loss events. A blob is only deleted after the server acknowledges it.
- **Privacy:** one shared capture policy across all collectors. **Explicit exclusions apply before any local persistence**, including stills and video. If private-window state can't be established reliably, that content is suppressed and a metadata-only gap is emitted. Secure Input pauses content collection, not metadata.
- **Packaging:** a signed menu-bar `.app` plus a LaunchAgent. Permission loss is detected and recorded the moment it happens. Onboarding is an explicit flow; background jobs never trigger permission prompts. Permission continuity is a known risk that varies by OS version (macOS re-prompts for screen capture).

---

## 5. Server processing

### 5.1 Normalize, extract, compile
- **Normalizers** are versioned, async and quarantinable.
- **Visual processor:** decodes stills and keyframes and runs OCR behind an interface (Vision if the server is a Mac, otherwise an evaluated portable engine or a vision model). Exact hashes allow safe cache hits; perceptual similarity is only a sampling hint. Coverage is recorded per surface (complete / partial / truncated / unknown).
- **Compiler** (deterministic, pure L1 → L2):
  - each source/device is compiled independently into lanes
  - lanes are overlaid on a global axis
  - every transition and micro-switch is kept
  - app/domain changes are *hints*, not task boundaries
  - calendar entries are planned context until corroborated
  - aggregate-only data (e.g. phone app-day totals) stays in aggregate cards and is never placed into specific seconds

### 5.2 Interpreter (every 4 h, plus scheduled revisions)
- **Inputs:** an immutable input manifest (L2 range + 30 min overlap, source revisions, compiler / normalizer / prompt / model versions, entity-memory snapshot) and a bounded evidence packet. The interpreter has **no tools**; if it needs more evidence, it goes through a read-only broker.
- **Outputs:** structured claims (`actor, action, object, outcome, time support, evidence relation`), each marked **observed / reported-by-source / inferred**.
  - Completion claims ("ordered", "merged") require outcome evidence; otherwise the claim is an *attempt*.
  - Tasks are **disjoint interval sets** (interleaving allowed).
  - Goals are optional hypotheses and may be `unknown`.
  - Engagement bounds are estimates, not calibrated intervals.
  - An optional `primary` lane per interval may be inferred, with an `unknown` remainder; there are no fractional attention weights in v1.
- **Model strategy:** a strong model at every stage. Gate C compares a **single joint pass vs. staged** (episodes → task linking → goals) on identical fixtures and keeps whichever scores better. Original evidence stays reachable at every stage, and splits and merges can be repaired.
- **Checks** (outside the prompt): schema validation, allowed evidence references, claim/evidence consistency, contradiction checks and actor/source separation. Generated Hyperthymesia content (including recaptures of the dashboard) **never counts as independent corroboration**. All source content is treated as data, never as instructions, to defend against prompt injection.
- **Revisions:**
  - Watermarks track how complete each source is.
  - Dirty ranges expand to the boundaries of the spans they touch.
  - Overlap is reconciled through stable observation membership plus task merge/split aliases.
  - A revision is published atomically only if its input generation is still current.
  - The prior view stays served while a rerun is in progress.
  - Entity memory is stored as provenance-backed assertions, not prose.
  - A day gets a scheduled revision at T+24 h and on late data. That revision is not a claim of completeness.

### 5.3 Surfaces
- **REST `/v1`:**
  - `POST /events`, `POST /blobs`
  - `GET /timeline?from&to&level`
  - `GET /search`
  - `GET /evidence/{id}`
  - `GET /entities`
  - `GET /now`
  
  Each response exposes `observed_through`, `interpreted_through`, coverage and revision.
- **MCP:** offered if useful, after Gate C. Tokens are scoped by layer, source/domain and sensitivity.
- **Dashboard:** basic. Lanes, zoom levels, an evidence drawer and search. Captured HTML is escaped, and no remote content is fetched.

### 5.4 Security, privacy, retention
- **Network:** Tailscale only. Device credentials are bound to permitted namespaces and can be revoked at the API.
- **Webhooks:** none on the private server. Outbound polling with cursors is the default. A narrow authenticated relay is allowed later if a connector needs webhooks.
- **Data classes:** unknown data is `restricted`. Egress rules are enforced on the server before any LLM call or embedding. Restrictions propagate by lineage to blobs, OCR, logs, backups, API/evidence reads, summaries and embeddings.
- **Retention:** ships **with the first stored blob**. Raw video is kept ~30 days. What survives after that is a defined contract: time-indexed keyframes, decoded text and extraction metadata, event identities, blob manifests, and explicit `evidence_expired` states. GC is reference-safe and pinned by active jobs.
- **Backups:** a supported Postgres backup method, coordinated with the blob manifests, plus an exercised restore drill.

---

## 6. Language and agent-safety guardrails
- **Rust** for the agent, server, compiler, interpreter and connectors. **TypeScript** for the browser and editor extensions. zsh for the hook.
- **Platform bindings:** `objc2-*` as the single foundation, provisionally. The high-level `screencapturekit` crate is compared on the same Gate A spike and adopted only if it reduces audited glue code. All AppKit delegates and lifecycle code live inside the audited `hx-macos-sys` adapter, behind narrow safe interfaces. Versions and SDKs are pinned.
- `#![forbid(unsafe_code)]` everywhere else, plus `cargo deny` / `cargo vet` for dependencies.
- Newtype IDs and timestamps, `sqlx` checked queries, `clippy -D warnings`.
- **Evaluation, not snapshots alone:**
  - scripted known-action traces and adversarial fixtures: failed purchases, background agents, same-domain task switches, lost focus events, clock steps, stale evidence, prompt-injection pages
  - metrics: boundary error, duration error, claim precision, coverage/abstention, duplicate inflation, task-link quality
  - `insta` snapshots only guard the deterministic compiler against regressions
- Performance runs happen on a signed, already-authorized hardware runner; general CI replays captured fixtures.

---

## 7. Repo layout
```
crates/
  hx-schema          envelope v1, kinds, connector protocol, JSON Schema export
  hx-macos-sys       ONLY unsafe/FFI: AppKit lifecycle, SCK, VideoToolbox, AX, CGEventTap, IOKit
  hx-agent           collectors, policy, sampling, outbox, uploader, companion socket
  hx-agent-app       menu-bar app, onboarding, LaunchAgent
  hx-store           Postgres, blob store, retention/GC, backup manifests
  hx-jobs            queue: leases, attempts, idempotent outputs, quarantine
  hx-visual          server visual processor (decode, keyframes, OCR interface)
  hx-compile         pure L1 → L2
  hx-interpret       claims, checks, revisions, entity assertions, model providers
  hx-server          ingest, REST, dashboard (MCP later)
  hx-connector-sdk   protocol helpers for Rust connectors
connectors/          t3code, codex, claude-code, git, apple-bridge (aggregates), …
companions/          browser-ext, editor-ext, shell-hook
fixtures/            real source fixtures, scripted traces, adversarial cases
docs/                DESIGN.md, review/
```

---

## 8. Build gates

The engineering time budget for each gate is set before it starts. At expiry, the gate publishes **pass / fail / unknown** with bounded follow-ups. A spike is never extended silently, and unknown is never treated as passed. Only work that depends on an unresolved result is blocked.

**Gate A — feasibility and contracts (time-boxed spike)**
1. **Native skeleton:** a signed menu-bar process with focus, AX, event tap, ScreenCaptureKit, hardware encoding and still output. One 8 h soak including stop/start and sleep/wake. It reports memory growth, dropped observations and teardown results, in an already-authorized environment.
2. **Measurement report:** three workloads (static reading, coding with builds, video). For each: CPU, energy, memory, **combined** bytes (video + stills + events), and still legibility at a stated resolution and cadence, checked with one available server OCR option. (For scale: one 500 kB still every 10 s for 8 h is already 1.44 GB, so the still budget has to be measured, not assumed.)
3. **Versioned contract:** envelope v1 and the connector protocol, including clock, identity/revision, coverage and policy *semantics*.
4. **Source fixtures:** small real fixtures from T3 (the populated `orchestration_events` feed), Codex, Claude Code and Apple Bridge.

If resource targets are missed, Gate A produces an explicit feasibility report. That does not reopen Section 0.

**Gate B — reliable vertical slice**
- One Mac focus/input feed, the browser extension and one agent-session adapter, running end to end: outbox → ingest → interval compiler → basic day view.
- It replays duplicates, survives network loss, restarts and reordering, and shows that gaps and durations stay honest.
- Blob lifecycle and retention are included as soon as blobs exist.

**Gate C — meaning with an oracle**
- Strong-model interpretation on the 4 h schedule with claim checks, abstention and disjoint task intervals.
- Joint vs. staged inference is compared on the same evidence.
- Results are queryable through the API.

**Gate D — coverage and operations**
- server OCR at scale
- broader semantic connectors (Codex, Claude Code, git, editor, shell)
- Apple Bridge app-day aggregates, honestly labeled; a bounded InFocus-transition export added to **Apple Bridge** and qualified for real phone lanes
- late-data revisions
- backup/restore drills
- privacy and injection fixtures
- optional goal hypotheses
- MCP output

**Later:** health connectors (generic metric contract: units, point/interval/aggregate, sampling interval, algorithm version, source physiological day, corrections; Oura exposes discrete HR samples and sleep HRV arrays, not raw R–R; WHOOP exposes summaries), location, richer retrieval, plugin isolation (WASM), audio.

## 9. Open (user decisions, not engineering disputes)
- Server host: VPS vs. home machine. A Mac server would allow Vision OCR and local models.
- Still-capture budget, once Gate A has measured it.
- Audio/meeting capture.
- Whether to add a correction UI later (the contract already supports a `correction` source).
