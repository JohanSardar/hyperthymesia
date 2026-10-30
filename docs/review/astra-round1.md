**Astra round 1 — changes required**

Reviewed all 256 lines of `docs/DESIGN.md`, draft v2, on 2026-10-30. Reviewed-file SHA-256: `a74df1d308790b48e612cbad3dd8301415486c10297ea0f004f0dce00b1387e3`.

Section 0 is settled. This review accepts server-side processing, thin devices, a source-independent connector core, interpretation every four hours, no human review loop, permanent interpretations, roughly 30-day raw-screen retention followed by keyframes/text, app-level phone data for v1, Rust or a safe functional language, API/basic dashboard, and no LLM budget cap. Recommendations below implement those decisions rather than reopen them.

The architecture's strongest idea is preserving evidence beneath interpretations. Its biggest defect is repeatedly treating incomplete observations as exact knowledge. A second-resolution timestamp does not establish second-accurate human activity. Keeping uncertainty explicit is necessary to pursue the stated goal honestly.

Evidence below distinguishes published API contracts, upstream implementation, local inspection, and my engineering conclusions. I did not run capture, measure resource usage, validate phone totals, change permissions, or inspect credential values. Local database reads used SQLite read-only connections. No source files were edited.

1. **Presence does not make human durations exact.**

   **Claim →** §5.2, lines 161–162: build active/idle/locked/asleep/agent-down presence, clip focus to it; this makes durations exact.

   **Why weak →** A person reading a document, watching a video, thinking about code, and leaving the desk can produce the same absence of input. A focused app identifies a foreground context, not attention. Conversely, a background build can run while the person works elsewhere. Applying a two-minute idle threshold retroactively or prospectively produces different durations; neither establishes which happened. `asleep` also conflates device sleep with human sleep. These are counterexamples to the inference, independent of any model's quality.

   **Proposed change →** Separate observed device state, input activity, foreground exposure, and inferred human engagement. Preserve timestamped transitions and explicit coverage intervals. Name device sleep accordingly. Give inferred episode boundaries an estimate plus uncertainty bounds; permit `unknown` and `unobserved`. Publish foreground duration, inferred engagement duration, and coverage separately. Target ≤1-second error for observable transitions under tested healthy capture conditions; do not transfer that guarantee to reading, intention, or unavailable phone data. This preserves the ambition while preventing fabricated precision.

   **Severity → blocker.** The current invariant would make confidently incorrect durations a core feature.

2. **The compiler destroys the boundaries the interpreter needs.**

   **Claim →** §5.2, line 163: split at app/domain/calendar/new-thread boundaries and merge switches under ten seconds into a parent as glances; LLM time bounds must come from these segments.

   **Why weak →** Two unrelated GitHub PRs share a domain; editing, testing, and deployment can happen in one terminal window. A six-second message or purchase confirmation can be the day's decisive event. A scheduled calendar boundary is not evidence that a meeting began. Calling all short switches glances is already semantic interpretation inside the supposedly deterministic compiler. Once one segment contains two tasks, a model restricted to whole-segment bounds cannot recover their durations.

   **Proposed change →** Make L2 a lossless interval index over observed transitions, with references to events inside intervals. Build compact prompt packets and visual coalescing as separate projections. Keep every micro-switch's original interval and make `glance` a reversible interpretation. Let semantic episodes select multiple disjoint intervals and split only at retained, evidenced boundaries. App/domain changes are useful hints, not mandatory task changes; calendar entries are planned context until corroborated. The 100–400 figure should be a presentation expectation, never a truncation rule.

   **Severity → major.** This creates an avoidable ceiling on downstream accuracy.

3. **Parallel lanes need explicit accounting, including phone-only periods.**

   **Claim →** §5.2, line 164: attach phone usage/messages to a laptop segment as parallel lanes; dashboard shows time per goal/project.

   **Why weak →** Attaching everything to laptop segments loses activity while the laptop is closed. Summing overlapping laptop, phone, and agent intervals double-counts elapsed human time. An A–B–A task cannot use `last_end - first_start` as active duration. Receiving a message establishes delivery, not that it was read. An app-day total cannot be placed into particular seconds at all.

   **Proposed change →** Compile each device/source independently, then overlay lanes on a global time axis. Represent task membership as interval sets. Calculate elapsed coverage using interval union; expose device exposure and agent execution as separate additive measures. For an exclusive human-time view, use explicitly inferred attention weights with total weight ≤1, plus an unknown remainder. Keep aggregate usage in an aggregate lane/card without inventing its within-day placement.

   **Severity → major.** Without these rules, credible-looking totals will contradict the underlying day.

4. **The clock plan is missing from the actual wire contract.**

   **Claim →** §3.4 promises wall clock, monotonic clock, and an NTP offset per batch; §4.1 actually carries only `occurred_at` and `ended_at`.

   **Why weak →** A clock correction at upload time does not describe the clock when an offline event was captured. Wall time can step; monotonic values reset across boots and need defined suspend behavior. Apple explicitly distinguishes [`mach_continuous_time`](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/mach_time.h), which advances during sleep, from `mach_absolute_time`. NTP estimates include network-delay/error considerations; an offset is not proof of a globally exact timestamp ([RFC 5905](https://www.rfc-editor.org/rfc/rfc5905.html)). Source event time is also different from collection time, as the [OpenTelemetry log model](https://opentelemetry.io/docs/specs/otel/logs/data-model/#field-observedtimestamp) explicitly models.

   **Proposed change →** Add an envelope version; source time and its resolution; observed time; server receipt time; device/boot/clock IDs; sequence; monotonic capture time; and references to clock calibration samples with error bounds. Preserve original values and version normalized UTC mappings. Compute same-boot durations from the selected monotonic domain; use piecewise mappings across clock steps, never subtract unrelated clocks. Record timezone/offset history for civil-day rendering and travel. Specify half-open intervals, DST days of 23/25 hours, cross-midnight splitting, and ordering when uncertainty overlaps. Calibrate periodically and on wake/clock change rather than treating every upload as a fresh authoritative offset.

   **Severity → blocker.** Retrofitting temporal provenance after capture cannot recover what was never recorded.

5. **A missing heartbeat does not mean the agent was down.**

   **Claim →** §3.4, line 93: a gap in 60-second heartbeats means agent-down.

   **Why weak →** The design deliberately buffers offline. A server sees the same heartbeat gap when Wi-Fi fails, upload stalls, the device sleeps, or the agent crashes. Meanwhile a live process can keep heartbeating with a dead event tap, stalled ScreenCaptureKit stream, or revoked AX permission. A 60-second sample cannot locate a crash to the second. This follows directly from the proposed outbox/transport architecture.

   **Proposed change →** Separate collector health, local capture continuity, uploader health, and server-observed connectivity. Heartbeats should carry per-collector status, last sequence, last observed timestamp, queue age, and drop counters. Persist capture lifecycle/error events locally. Mark an unresolved interval `coverage_unknown`; reconcile it on reconnect. Store the bounded interval in which a crash occurred instead of assigning a precise failure instant. Keep metadata health running when content capture is intentionally paused.

   **Severity → major.** Otherwise network outages become false behavioral records and partial collector failures become invisible.

6. **Device-side Vision OCR violates the fixed architecture.**

   **Claim →** §§3.1/3.3/7 put Vision OCR, its cache, and profile-dependent OCR execution in the Mac agent and `hx-macos-sys`.

   **Why wrong →** Section 0 fixes all processing on the server. OCR is interpretation of pixels, not an OS observation. It also undermines the stated strategy for a tiny device CPU budget. Vision's availability depends on an Apple host, while the server host is explicitly undecided. A future request to OCR an evicted local frame cannot recover it.

   **Proposed change →** Remove OCR from the device design. Thin devices collect OS/app observations, enforce capture exclusions, timestamp, spool, and perform necessary transport encoding. The server decodes, extracts keyframes, OCRs, deduplicates semantic content, normalizes, and interprets. Put OCR behind a server processor interface: Vision on a Mac server or an evaluated portable alternative on Linux. Store capture-time frame identity and coverage regardless of when OCR runs. Retain local hardware encoding as a transport mechanism with a measured cost, not an analysis stage.

   **Severity → blocker.** This is a direct contradiction of a fixed user decision.

7. **The performance targets have no defined workload or accounting boundary.**

   **Claim →** §§1/3: SCK change detection is free, HEVC is approximately zero CPU, and Full capture must fit 1.5% of one core, 120 MB RSS, and 1.5 GB/day.

   **Why weak →** Apple's [`SCFrameStatus.idle`](https://developer.apple.com/documentation/screencapturekit/scframestatus/idle) establishes that no new frame was generated when the display did not change. It does not mean semantically irrelevant animation disappears or that downstream work is free. Apple's [SCK session](https://developer.apple.com/videos/play/wwdc2022/10155/) describes surface-pool memory, queue-depth tradeoffs, and stalls when buffers are retained. Three 3840×2160 BGRA buffers alone represent about 95 MiB of image storage; shared IOSurfaces do not map neatly to process RSS. Encoder buffers, WindowServer/GPU work, filesystem activity, and extension work need accounting too. Hardware acceleration reduces CPU work, not energy to zero.

   The byte budget also constrains quality: 1.5 GB over eight active hours is about 417 kbit/s for **everything**, or about 139 kbit/s over 24 hours. Neither implies legible high-resolution text. Screenpipe's [current architecture page](https://docs.screenpipe.com/architecture) describes event-triggered screenshots with an idle fallback and gives whole-product resource figures; it does not isolate OCR as the dominant measured cost. The design's attribution is unsupported.

   **Proposed change →** Keep budgets as provisional acceptance targets, not established feasibility. Specify display count/resolution, pixel format, scaling, cursor policy, queue depth, hardware-encoder requirement/fallback, and recording-active hours. Benchmark static reading, fast scrolling, video, IDE builds, multiple displays, sleep/wake, and offline upload separately. Measure process CPU, total incremental system energy, memory footprint, upload bytes, frame loss, and text legibility. Use a signed, already-authorized hardware runner; general CI should replay captured fixtures. Reject a configuration that meets CPU targets by losing essential evidence.

   **Severity → major.** A hard target without a workload encourages silent quality degradation.

8. **Accessibility and event taps require an explicit failure-safe execution model.**

   **Claim →** §§3.1/3.4: AX text and click hit-tests are cheap; one Tokio process plus a ring buffer is sufficient.

   **Why weak →** AX calls communicate with other apps and can fail or block; Apple exposes [`AXUIElementSetMessagingTimeout`](https://developer.apple.com/documentation/applicationservices/1459345-axuielementsetmessagingtimeout) for a reason. [`AXObserverAddNotification`](https://developer.apple.com/documentation/applicationservices/1462089-axobserveraddnotification) can report unsupported notifications. Native does not mean a small, complete, responsive AX tree. A post-click hit-test may inspect a replacement window rather than the clicked element. AppKit also has thread-affinity requirements; a Tokio runtime does not replace a Cocoa/CFRunLoop. The [objc2 example](https://docs.rs/objc2/0.6.5/objc2/) explicitly uses a main-thread application run loop.

   **Proposed change →** Define the main-thread/run-loop owner, event-tap run loop, bounded callback handoff, AX worker deadlines, SCK dispatch queues, and Tokio I/O workers. Do no AX work in the event callback. Every collector needs overflow counters, cancellation, restart/re-registration, and explicit missing-data output. Bound AX calls by time as well as node count. Label click targets as observations at their actual sample time; do not guarantee they identify the original click. Start with focus/window metadata and a narrowly bounded AX extraction policy; add expensive traversals only where measured coverage justifies them.

   **Severity → major.** These details determine whether recording survives ordinary app hangs and wake cycles.

9. **Any available text is not a reason to suppress visual evidence.**

   **Claim →** §§2/3.1: screen/OCR is needed only where tiers 1–3 provide no text; perceptual hashes help cache OCR.

   **Why weak →** A remote desktop can expose toolbar labels while its useful contents are pixels. A dashboard can expose navigation text but omit its chart. A page extractor may return an old article while a canvas or modal changes. A one-digit price or test-status change can be semantically decisive despite a small perceptual difference. Conversely, a whole-page DOM snapshot includes material the person never saw. The deterministic fallback predicate cannot establish semantic completeness from nonempty text.

   **Proposed change →** Carry extraction coverage by surface/region, visibility, capture time, truncation, and capability. Preserve sparse visual evidence across authorized foreground contexts even when text exists, with event-adjacent captures where practical. On the server, use exact hashes for safe cache hits; treat perceptual similarity as a sampling hint, not proof of textual equality. Request OCR/vision for uncovered regions, stale text, or disagreements. Define active-display versus all-visible-displays policy explicitly. A window capture that includes occluded contents must not be described as evidence that the user saw them; Apple documents that behavior in its [SCK examples](https://developer.apple.com/videos/play/wwdc2022/10155/).

   **Severity → major.** The present fallback rule systematically misses exactly the changes that explain outcomes.

10. **Rust is a good choice; the binding plan is not yet a feasibility result.**

   **Claim →** §§3.4/6/7: generated `objc2-*` bindings make one unsafe crate sufficient for all macOS integration, with the rest forbidden from using unsafe code.

   **Why weak →** Generated APIs do not implement callback lifetimes, Objective-C delegate contracts, run-loop teardown, autorelease behavior, or safe cross-thread ownership. Even a basic objc2 AppKit delegate requires unsafe declarations; therefore the menu-bar application's unsafe surface must also be inside the permitted adapter. Merely forbidding unsafe in local crates does not audit unsafe dependencies.

   I checked the registries directly: [`objc2-screen-capture-kit`](https://crates.io/api/v1/crates/objc2-screen-capture-kit) is 0.3.2, [`cidre`](https://crates.io/api/v1/crates/cidre) 0.29.0, and [`screencapturekit`](https://crates.io/api/v1/crates/screencapturekit) 11.1.1 at review time. The indexed `screencapturekit/latest` page still displayed 11.0.0, so pin versions rather than copying “latest” examples blindly. [`cidre`](https://github.com/yury/cidre) covers many relevant frameworks, describes itself as a personal research project, and requires availability handling for newer selectors. [`screencapturekit`](https://docs.rs/screencapturekit/11.1.1/screencapturekit/) offers a higher-level SCK wrapper, but is not an AX/AppKit implementation or an independent safety audit. These are credible candidates, not interchangeable assurances.

   **Proposed change →** Keep Rust and prefer one coherent `objc2-*` foundation provisionally. Before building the full agent, prove a signed menu-bar skeleton, run loops, focus/AX callbacks, SCK delivery, encoder integration, and stop/restart without leaks. Compare the high-level SCK wrapper on that same spike; adopt it if it demonstrably reduces audited glue without ownership/conversion problems. Put AppKit delegates and lifecycle code inside the audited platform adapter, with narrow safe interfaces. Pin SDK/deployment targets and dependencies. Do not choose by download counts or claim that agents inherently write one language well.

   **Severity → major.** The language choice is settled enough; the platform boundary is not.

11. **Permission behavior needs a compatibility matrix, not a monthly countdown.**

   **Claim →** §1: capture re-prompts roughly monthly across Sequoia–Tahoe and cannot be suppressed outside MDM; §3.4 implies stable signing alone solves development TCC continuity.

   **Why weak →** The evidence given is secondary and spans changing OS policies. Apple now documents a restricted [Persistent Content Capture entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.persistent-content-capture) for approved VNC apps. That does **not** establish eligibility for Hyperthymesia, but it defeats the universal “outside MDM impossible” claim. Permission grants, reminders, stopped streams, code identity, and helper identity are distinct issues. Stable signing is sensible, not a guarantee for every launch/upgrade arrangement.

   **Proposed change →** State that unattended permission continuity is an OS/version-dependent operational risk. Test the supported installed bundle/helper identity on actual target releases, including reboot, update, revocation, and stream restart. Detect real capability loss immediately and record it. Do not depend on obtaining the restricted entitlement, suppress prompts, or infer failure from elapsed calendar time. Installation and permission onboarding should be a bounded explicit workflow, never something background jobs repeatedly trigger.

   **Severity → minor.** The resilience conclusion is good; the categorical supporting claim is too strong.

12. **The envelope lacks durable identity, revision, and provenance semantics.**

   **Claim →** §§3.4/4.1: producer-generated ULIDs make ingestion idempotent; connector version and a kind-specific JSON schema complete the contract.

   **Why weak →** A retry is idempotent only if it reuses the same ID. A fresh import of an old message with a fresh ULID is a duplicate. A streaming message changes after creation; calendar events move; wearable records are rescored; source records can be deleted. The envelope cannot distinguish any of those from new independent observations. `source: t3code` identifies a connector type, not an installation/account/stream. The [CloudEvents identity rule](https://github.com/cloudevents/spec/blob/v1.0.2/cloudevents/spec.md#id) deliberately makes source plus event identity the deduplication key; an ID format itself is not a delivery contract.

   **Proposed change →** Separate immutable observation identity from source-object identity. Add connector-instance ID, source-native object/event key, source revision or content digest, operation (`observe/upsert/delete`), source sequence/cursor/generation, schema URI/version, collection snapshot scope/completeness, and raw provenance. Include actor/subject/resource and causal/session relationships. For append-only sources, reuse native event IDs; for snapshots, derive stable identities from source object plus revision. Persist IDs before upload. Keep ULIDs as convenient identifiers if desired, never as the whole idempotency argument. Authenticate the producer's allowed namespaces rather than trusting its `source` string.

   **Severity → blocker.** Replay and mutable sources are required immediately, not at connector number one thousand.

13. **A list of transports is not a connector lifecycle contract.**

   **Claim →** §4.3: HTTP push, scheduled CLI/API pulls, MCP tools, and eventual WASM normalizers provide modularity at scale.

   **Why weak →** They do not specify pagination, durable checkpoints, backfill, deletions, incremental updates, rate limits, OAuth refresh, retries, schema evolution, or coverage. A bad normalizer in the synchronous ingest path can prevent preserving the very raw record needed to repair it. MCP can return validated [structured content](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#structured-content); it does not make an arbitrary query tool a complete resumable change feed. A successful tool response can still omit historical records.

   **Proposed change →** Define one transport-independent protocol with `describe_capabilities`, snapshot/backfill, resume cursor, change batch, checkpoint, coverage, and health semantics. Acknowledge a checkpoint only after its records are durably stored. Transport adapters must report incomplete scans rather than treating missing rows as deletions. Commit valid raw envelopes first; normalize asynchronously under versioned, bounded jobs, quarantining failures without losing L0. Keep source-specific mappings outside core; normalize into generic facts/spans/entities/relationships with extension payloads instead of forcing every future source into today's 17 categories. Version outputs separately from connector binaries. Defer Extism and a marketplace; support many implementations, not a thousand always-running processes.

   **Severity → major.** Source independence must include behavior and failure semantics, not just namespacing.

14. **The local agent-session sources already require revisions, correlation, and actor separation.**

   **Claim →** §§2/4.2/8: T3, Codex, and Claude Code are near-free semantic connectors emitting `agent.message` and identifying what the person worked on.

   **Why weak →** Read-only inspection of `~/.t3/userdata/statev2.sqlite` found 218,497 rows in `orchestration_events`, **zero** in `orchestration_v2_events`, and 9,958 in `orchestration_v2_projection_messages` at the inspection instant. Recent event types include `message.updated`, `turn-item.updated`, `node.updated`, and `provider-turn.updated`. There are native sequence/event/version fields and provider-thread/session binding tables. Picking a table by its newest-looking name would silently ingest nothing; treating updates as independent messages would inflate activity.

   Structural inspection of sampled local Codex JSONL found `session_meta`, `event_msg`, `response_item`, turn IDs, call IDs, and timestamped lifecycle data. Claude JSONL includes UUID/parent UUID/session IDs, sidechain markers, assistant messages, user-shaped records carrying tool results, and internal bookkeeping. The same coding session can appear in T3 and the provider logs. None of these events proves the user was attending to the output.

   **Proposed change →** Build each adapter against observed schema/version fixtures and fail visibly on unknown versions. For this T3 installation, evaluate the populated ordered event feed plus versioned projections; use explicit column lists, read-only WAL-aware transactions, and short reads. Preserve update semantics. Tail JSONL using file-generation/offset checkpoints, tolerate partial final lines, and deduplicate using native IDs. Correlate T3/provider sessions, turns, calls, and child agents; retain both provenance paths while avoiding duplicate claims. Model human requests, assistant execution, tool results, and observed user exposure separately. The same applies to shell hooks: interactive `preexec/precmd` does not cover noninteractive agent commands or prove background-job completion.

   **Severity → major.** These are observed properties of this Mac, not speculative integration edge cases.

15. **Apple Bridge currently exports the wrong shape for the proposed phone lane.**

   **Claim →** §§1/5.2/8: Apple Bridge supplies iPhone app usage with about 20 hours of lag, usable as overlapping timeline spans.

   **Why wrong/weak →** The local implementation emits `usage.device` and **`usage.app_day`** records, not timestamped usage spans ([UsageSource.swift:68](/Users/johansardar/apple-bridge/native/Sources/UsageSource/UsageSource.swift:68)). [UsageRollup.swift:19](/Users/johansardar/apple-bridge/native/Sources/UsageSource/UsageRollup.swift:19) caps sessions at four hours, produces `seconds_estimated`, and explicitly says parity is unproven and unclosed sessions are omitted. The [20-hour observation](/Users/johansardar/apple-bridge/research/screentime-probe-results.md:19) was a single probe on the MacBook Air, not a latency guarantee for this Mac. Its later probe notes also leave an alternative Shortcuts route unqualified; “the only path” is not established.

   Apple DTS does confirm that [`DeviceActivityReportExtension` is intentionally sandboxed against exporting its report data](https://developer.apple.com/forums/thread/818174). That supports rejecting a naive report-export app, not upgrading Biome estimates into precise usage.

   **Proposed change →** For the existing bridge, import app-day totals with their uncertainty and revision semantics, display them honestly, and do not place them into seconds. To obtain lanes, add a bounded export of source InFocus transitions with native timestamps, source-device identity, decoder version, gaps, and lock evidence; reconstruct estimated spans on Hyperthymesia's server. Make that a separately qualified capability. Track observed lag distribution and completeness; allow arbitrary late arrivals. Treat Biome as a versioned best-effort private-format adapter, not an Apple-supported usage ledger.

   **Severity → major.** The claimed integration cannot produce the stated output from its current exported data.

16. **Browser instrumentation must capture exposure, not just documents and tabs.**

   **Claim →** §3.2: tab focus, URL/title, readable text, selection/scroll, and media-playing state yield a useful cheap browser feed.

   **Why weak →** A tab can be active in an unfocused window; navigation is not exposure; background media is not attention. Chrome's [`tabs.onActivated`](https://developer.chrome.com/docs/extensions/reference/api/tabs#event-onActivated) fires for a window's active-tab change and may fire before a URL is available. SPA route changes and document updates need their own handling. MV3 [service workers can terminate after inactivity](https://developer.chrome.com/docs/extensions/develop/concepts/service-workers/lifecycle#idle-and-shutdown), so in-memory state is insufficient. Readability is useful for articles, not a general representation of IDEs, inboxes, dashboards, PDFs, or canvases.

   **Proposed change →** Correlate browser-window focus, OS app focus, active tab, document identity, page visibility, navigation/SPA changes, and bounded viewport/content snapshots. Persist extension state and an acknowledged event outbox; reconcile on worker restart. Record extraction coverage/truncation and distinguish loaded content from observed visibility. Choose [native messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) with an explicit extension-origin allowlist for v1, rather than leaving localhost authentication unspecified. The content script should forward bounded observations, not authorize arbitrary local commands. Treat editor and shell integrations with the same lifecycle rigor.

   **Severity → major.** Otherwise the highest-ranked semantic source produces misleading reading durations and unexplained gaps.

17. **The cited papers justify experimentation, not this exact A/B/C architecture.**

   **Claim →** §§1/5.2: TMI and Tempo motivate a fast episode labeler, strong task separator, then strong goal attachment.

   **Why weak →** [TMI §4.1](https://arxiv.org/html/2608.20319v1#S4.SS1) reports 0.974 **Adjusted Rand Index** on synthetic interleavings of task sessions, not 97.4% factual accuracy on an ordinary person's day. Its grounding uses screenshots around actions and its implementation uses a strong model throughout. Appendix D.3 explicitly reports that segmentation errors persist when later stages cannot resegment. It does not validate domain splitting, ten-second merging, a cheap first model, or this capture diet. [Tempo](https://arxiv.org/html/2605.00497v2) studies a week-long deployment with 14 participants and treats inferred life goals as interpretive artifacts; it does not establish autonomous goal truth. [Activity Frames](https://arxiv.org/abs/2608.05784) supports deterministic compilation as a promising technique, with evidence from one person's corpus rather than a general guarantee.

   **Proposed change →** Keep deterministic evidence assembly and interleaved task linking. Initially implement strong-model evidence-grounded episode reconstruction plus task linking, with optional goal hypotheses as a view. Compare that with a single strong joint pass and the proposed three-stage pipeline on identical held-out fixtures. Preserve access to original evidence at every stage and support split/merge repair. Do not economize on the earliest semantic stage without an accuracy result; there is no budget constraint requiring that. Keep four-hour scheduling. Permit “goal unknown,” alternatives, and tasks with several plausible goals rather than manufacturing a new life goal every window.

   **Severity → major.** The present staging is plausible but overcommitted and has known error-propagation failure modes.

18. **A valid evidence ID does not make a claim true, and snapshot tests cannot establish accuracy.**

   **Claim →** §§5.2/6: require citations and confidence; test schema/evidence validity and one golden day rather than wording.

   **Why wrong/weak →** “Ordered a desk” can cite a valid product-page screenshot. “Merged PR” can cite a click even when the server rejected it. A model can correctly cite an assistant's invented statement and still make a false autobiographical claim. Existence of a source is not entailment. Self-reported confidence is not calibrated probability. Snapshot tests preserve the current compiler's output, including incorrect semantics. None of this requires a human review loop to fix.

   **Proposed change →** Make factual claims structured: actor, action, object, outcome, time support, and evidence relation. Distinguish observed, reported-by-source, and inferred assertions. Require outcome evidence for completion claims; otherwise record an attempt. Add automatic claim/evidence checks, contradiction checks, and a verifier allowed to abstain, without calling it a proof. Evaluate on scripted known-action traces and synthetic adversarial fixtures, plus held-out traces where ground truth exists. Measure boundary error, duration error, claim precision, coverage/abstention, duplicate inflation, and task-link quality. Include failed purchases, background agents, same-domain task switches, lost focus events, clock steps, and stale evidence. Confidence calibration must come from measured results. No user approval or per-day correction is introduced.

   **Severity → blocker.** The proposed tests can pass while the system invents a convincing day.

19. **Four-hour overlap and stale days need revision semantics, not just reruns.**

   **Claim →** §5.2: process the last four hours plus 30 minutes; rerun at T+24 hours or on staleness; update canonical L3 and entity memory.

   **Why weak →** The overlap can duplicate episodes and fork task identities. Late events can extend spans across window/day boundaries. A rerun may finish after a newer run and overwrite better data. A correction to yesterday's task identity can invalidate today's entity memory. Current memory can also leak hindsight into an “as known then” replay. A T+24-hour pass is a scheduled revision, not a guarantee that every source is complete.

   **Proposed change →** Give each job an immutable input manifest, source revisions, compiler/normalizer/prompt/model/config versions, and entity-memory snapshot. Maintain source completeness watermarks and dirty ranges expanded through affected span boundaries/dependencies. Reconcile overlap through stable observation membership and explicit task merge/split aliases. Publish canonical pointers atomically only if the job still matches the desired input generation; retain superseded versions. Coalesce late-arrival work and retry failures while retaining the prior usable view. Store entity changes as provenance-backed assertions, not mutable prose. Expose `observed_through`, `interpreted_through`, source coverage, and revision in `/now` and timeline responses. Distinguish hindsight replay from reconstruction using only then-available evidence.

   **Severity → major.** Otherwise reprocessing creates races, duplicates, and self-reinforcing false memory.

20. **Postgres is the right v1 default, but the physical and durability contracts are incomplete.**

   **Claim →** §5.1: monthly Postgres partitions, FTS, pgvector, and a jobs table handle the expected personal volume.

   **Why weak →** I agree with the store choice; “tens of millions of rows” alone is not a capacity argument. One always-on one-second stream already produces 31.5 million rows/year; text size, duplicate layers, indexes, and embeddings dominate different workloads. A concrete trap: PostgreSQL 18 requires a partitioned table's unique constraint to include the partition key ([partitioning limitations](https://www.postgresql.org/docs/18/ddl-partitioning.html#DDL-PARTITIONING-DECLARATIVE-LIMITATIONS)). `UNIQUE(id)` therefore cannot simply be added to an event table partitioned by time. A jobs table alone also does not establish reliable processing. [`SKIP LOCKED`](https://www.postgresql.org/docs/18/sql-select.html#SQL-FOR-UPDATE-SHARE) is useful for queue consumers, not an exactly-once guarantee.

   **Proposed change →** Keep Postgres plus content-addressed blob files; do not add Kafka/ClickHouse/Timescale now. Specify global identity deduplication separately from time-partitioned payload storage, or begin with an unpartitioned event table until partitioning has a measured benefit. Index typed time/source/entity columns, avoid indexing all JSON indiscriminately, and store repeated snapshot bodies once with separate observation references. Use compact numeric batches where appropriate; second accuracy does not require a dense row per second at every layer. Embed retrieval units, not every raw sample. Define job leases, attempt limits, idempotent output keys, transactional enqueue with ingest, recovery, and queue isolation for interactive versus backfill work. Benchmark representative payloads and queries before adding another database.

   **Severity → major.** The decision is sound; deduplication and reliable execution still need an implementable design.

21. **Retention and backup rules contradict unlimited rebuildability.**

   **Claim →** §§3.4/5.1/8: every layer is rebuildable, raw video expires after about 30 days, the capped outbox sheds video, and retention jobs arrive in M7.

   **Why weak →** After raw frames disappear, improved OCR cannot recover previously missed text or unseen detail from arbitrary moments. L0 envelopes that reference deleted chunks are not a complete reconstruction source. A lost upload may be evicted locally before the server has any copy. A 5 GB cap at the stated 1.5 GB/day ceiling represents only about 3.3 days before other overhead. Expiration can race with interpretation/backfill. Finally, encrypted restic backups do not alone make a consistent backup of a running Postgres database; PostgreSQL describes the constraints on [filesystem backups](https://www.postgresql.org/docs/18/backup-file.html).

   **Proposed change →** Keep the fixed retention policy, but define the surviving evidence contract: time-indexed keyframes, decoded text and extraction metadata, original event identities, blob manifests, and explicit evidence-expired states. Say which analyses remain reproducible after expiration. Implement retention alongside the first stored blob, including job pins, reference-safe GC, upload acknowledgement, missing-blob handling, and eviction loss events. Use independent budgets/reserved space for high-value events and bulk media. Finalize chunks with checksums and atomic publication; durable ingest must distinguish accepted metadata from completed blob transfer. Back up Postgres using a supported logical/physical method and coordinate its manifest with immutable blobs, then exercise a restore. Long-term interpretation retention remains unchanged.

   **Severity → major.** Deferring this risks irreversible loss of evidence before accuracy is understood.

22. **Secure Input, regex scrubbing, and producer privacy flags do not enforce the stated privacy promises.**

   **Claim →** §§3.2/3.4/5.4: never log raw keys, stop capture during Secure Input, scrub shell secrets with regexes, and redact events marked sensitive before LLM calls.

   **Why weak →** Apple's [Secure Event Input note](https://developer.apple.com/library/archive/technotes/tn2150/_index.html) describes protection of keyboard events, not classification of all sensitive pixels, URLs, commands, and transcripts. A token can be pasted into a command, encoded, printed by a tool, embedded in a URL, or visible in a screenshot. Stopping all collection also suppresses noncontent coverage metadata if interpreted literally. A browser extension disabled in private windows cannot reliably tell the screen collector what is displayed there. An unmarked payload defaults to normal in the proposed envelope. The same secret can survive in a blob, OCR text, L3, or an embedding after a single source field was scrubbed.

   **Proposed change →** Enforce one capture policy across companions and screen/AX collectors, before persistence for explicit exclusions. Require reliable private-window identification or conservatively suppress content where the promise cannot be enforced; preserve metadata-only gap reasons. Retain the no-raw-keys rule. Treat shell argv, URLs, and agent transcripts as potentially sensitive; parsing/redaction is defense in depth, not a guarantee from a denylist. Use server-enforced classification/egress rules with unknown/restricted states and field/blob lineage. Apply restrictions to derived summaries, embeddings, evidence, logs, and backups. Never let producer labels alone authorize LLM disclosure. Tokens need source/domain and sensitivity scopes as well as layer scopes: an L3 summary can reveal the same secret as its raw source.

   **Severity → blocker.** The current safeguards can leak precisely the data they claim to exclude.

23. **Prompt injection and self-observation can permanently contaminate memory.**

   **Claim →** §§5.2–5.4: private networking, scoped tokens, citations, and versioned entity memory establish safe interpretation.

   **Why weak →** The input is an untrusted corpus of websites, messages, documents, terminal output, and agent-generated prose. A webpage can instruct the interpreter to invent a purchase or overwrite a goal. That content can carry valid evidence IDs. [OWASP's prompt-injection guidance](https://genai.owasp.org/llmrisk/llm01-prompt-injection/) explicitly covers indirect and multimodal injection; retrieval and structured formatting do not eliminate it. More subtly, a Hyperthymesia summary shown in the browser can be captured, imported through a coding transcript, and reused as supposedly independent corroboration. Stored hallucinations then gain apparent support on every pass.

   **Proposed change →** Keep the interpreter tool-less by default and give it a bounded, scoped evidence packet; if evidence retrieval is necessary, expose only a read-only broker. Treat every source's content as data, never operating instructions. Enforce output schema, allowed evidence references, actor/source distinctions, and automated adversarial checks outside prompts. Preserve derivation lineage and mark system-generated summaries, quoted/replayed content, and cross-source duplicates. Do not count a generated summary or its recapture as independent evidence for itself. Exclude the dashboard's generated content from factual corroboration while still allowing a metadata event that it was viewed. Escape/sanitize dashboard rendering and do not let captured HTML or remote image URLs execute or fetch as trusted UI. Bind device credentials to permitted sources and revoke access at the API, not merely the network.

   **Severity → major.** A lifelong memory system needs integrity protection as much as confidentiality.

24. **The health claim confuses samples with continuous raw signals.**

   **Claim →** §1: Oura v2 provides continuous HR and RR series; WHOOP v2 exposes summaries; therefore the health schema can be fixed now.

   **Why wrong/weak →** I fetched Oura's documentation-linked [OpenAPI 1.43 specification](https://cloud.ouraring.com/v2/static/json/openapi-1.43.json). It exposes timestamped discrete heart-rate samples, sleep HR/HRV sampled arrays with an explicit interval, and average breathing rate during sleep. It does not expose a general beat-to-beat R–R endpoint. If RR means respiratory rate instead, the cited sleep field is an average, not a continuous respiratory series. The [WHOOP public API](https://developer.whoop.com/api/) supports the narrower summary-oriented characterization. Neither provides a universal second-accurate physiology stream or equates a vendor's physiological day with a local calendar day.

   **Proposed change →** Correct the capability claim and defer vendor-specific schema commitments until real responses are qualified. The generic contract should support metric identity, units, point/interval/aggregate semantics, sampling interval, missing values, source quality, algorithm version, source physiological day, and corrections. Represent HRV as HRV, never reconstructed raw R–R. Preserve original payloads so future capability additions require adapters, not core migrations. Health remains later work.

   **Severity → minor.** Fix the factual assumption now; avoid building unsupported capability around it later.

25. **Private-only networking and cloud webhooks do not connect by themselves.**

   **Claim →** §§4.3/5.4: use vendor webhooks while exposing no public ports and keeping the server exclusively on Tailscale.

   **Why weak →** An external vendor cannot deliver callbacks to an ordinary private tailnet address. Oura's [webhook subscription schema](https://cloud.ouraring.com/v2/static/json/openapi-1.43.json) requires a callback URL and includes an expiration time; delivery, validation, renewal, and resynchronization need an actual design. This is independent of event normalization.

   **Proposed change →** Use outbound API polling with durable cursors/backfill as the default for the private server. If a later connector benefits from webhooks, use a deliberately narrow public relay that authenticates vendor requests and forwards notifications over an authenticated private channel; fetch authoritative records server-side. Keep the main API and evidence store private. Treat webhook delivery as a hint and retain reconciliation polling. Do not quietly introduce a public application endpoint through a connector.

   **Severity → minor.** A straightforward architectural gap with a simple v1 solution.

26. **The build order postpones the highest-risk evidence and data-loss questions.**

   **Claim →** §8: prove curl ingestion, build many semantic connectors, build the three-stage interpreter, then add screens/AX at M5, phone at M6, retention at M7.

   **Why weak →** A successful insert proves very little about offline capture, duplicate imports, mutable records, timing, or useful meaning. M2's exit criterion mentions segments before M3 builds the compiler. M1 promises exact presence before the ambiguity is resolved. Deferring platform capture and phone-contract qualification lets the wrong assumptions harden in the schema. Deferring retention puts live data at risk. Building entity memory and goals before verifying basic episode facts makes failures harder to localize.

   **Proposed change →** Replace the sequence with these bounded gates, keeping v1's API/dashboard scope and four-hour LLM cadence:

   - **Gate A — feasibility and contracts:** a small signed capture/run-loop prototype; SCK/encoding and AX lifecycle measurements; actual T3 and Apple Bridge response fixtures; clocks, identities, revisions, coverage, and policy specified. This is a targeted spike, not all connectors.
   - **Gate B — reliable vertical slice:** one Mac focus/input feed plus browser and one agent-session adapter through outbox → ingest → interval compiler → basic day view. Replay duplicates, disconnect networking, restart, and reorder events; prove gaps and durations remain honest. Include blob lifecycle/retention as soon as blobs exist.
   - **Gate C — meaning with an oracle:** strong-model interpretation on the four-hour schedule with factual claim checks, abstention, and disjoint task intervals. Compare simpler versus staged inference using the same evidence. Query the result through the basic API.
   - **Gate D — coverage and operations:** server OCR, broader semantic connectors, properly qualified phone aggregates/intervals, late-data revision, backups/restore, privacy and injection fixtures, then optional goal hypotheses and MCP output if useful.
   - **Later:** health/location, richer retrieval, and plugin isolation only when the corresponding need is demonstrated.

   **Severity → major.** The first useful milestone should establish trustworthy evidence and replay, not merely a growing stack of components.

27. **Parts I explicitly agree with — settled for this round.**

   - All Section 0 decisions listed at the start of this review, including no routine human review and no LLM budget cap.
   - Rust for the native agent and server, with TypeScript for browser/editor companions. Pure deterministic transformations, typed IDs, checked SQL, narrow audited FFI, and crate contracts are good defaults.
   - Semantic instrumentation as the highest-value evidence source for coding and browser workflows, supplemented by OS observations and visual evidence. Do not globally force Chromium/Electron accessibility. Query already-available, bounded capabilities where useful; do not infer completeness from app category.
   - SQLite as the device outbox; Postgres for metadata, intervals, jobs, FTS, and initially vectors; content-addressed external blobs. No distributed event bus or additional analytical database is justified yet.
   - Immutable raw observations, explicit normalized layers, deterministic interval compilation, provenance links, versioned interpretations, and replay as primary product capabilities—with the temporal, revision, and expiration contracts above.
   - Separating interleaved tasks from local episodes. Carry stable entity identities between windows. Keep inferred goals distinguishable from observed facts.
   - App-level phone scope, late data, no audio in v1, and graceful degradation when an individual collector is unavailable.
   - Signed native packaging, bounded local storage, a visible pause/capture state, private networking, encrypted backups, scoped access, and treating missing capture as missing evidence.
   - API plus a basic Rust-rendered dashboard. htmx is reasonable for the first timeline; UI framework choice is not a major architectural risk here.

28. **Top five changes.**

   1. Preserve lossless observed intervals and uncertainty; stop equating focus/input silence with exact human attention or merging away short actions.
   2. Freeze a real event/connector contract: native identity, revisions/deletions, clocks, actor/session correlation, durable checkpoints, and coverage. Import Apple Bridge's actual aggregates honestly.
   3. Move OCR to the server and qualify the native capture/binding/energy design in an early signed prototype before promising the performance budget.
   4. Build evidence-checked factual episodes and disjoint task intervals first; evaluate staging, calibrate confidence, and allow unknown goals. Valid citations alone are insufficient.
   5. Ship privacy propagation, memory-poisoning defenses, atomic revisions, retention, and restore verification with the first reliable vertical slice—not after collecting a month of irreplaceable data.
