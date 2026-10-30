# Astra — round 2

I accept the smaller v1. We can converge on Claude's plan with the amendments below. Section 0 remains binding; the round-1 items Claude accepted are not reopened. This is a design verdict, not a claim that feasibility has been measured.

Read DESIGN v2, my round-1 review, and Claude's response in that order. Checked Apple Bridge source read-only; no capture, permission changes, or performance experiments were run.

## 1. “Record now, compute later” — AGREE WITH AMENDMENT

**Exact amendment:** “Record available observation-time provenance from the first capture; mark unavailable fields explicitly. Defer sophisticated estimation, never the minimum behavior needed to avoid false claims, unauthorized disclosure, or irreversible evidence loss. An unsupported case produces unknown/abstention rather than a fabricated answer.”

This permits deferring attention weights, calibrated boundary distributions, a separate verifier model, and an as-known replay interface. It does not defer the already-accepted claim checks, privacy enforcement, durable identities, revision publication, or retention contract. A simple clock implementation may split at discontinuities and leave cross-clock ordering uncertain; recording calibration fields alone does not justify subtracting stepped wall-clock timestamps.

## 2. #6 OCR location — AGREE WITH AMENDMENT

I accept low-bitrate replay video plus sparse, legible stills, with OCR behind the server processor interface. Vision on a Mac server and an evaluated portable engine or vision model elsewhere are compatible choices. We have no measured basis here for declaring one cheaper or more accurate.

**Exact amendment:** “Server-side OCR is the v1 architecture, not merely the default. Device work is capture, policy enforcement, sampling, buffering, and transport encoding. A device-Vision comparison is optional experimental evidence, not a shipping fallback or a Gate A dependency. Failure to meet provisional resource targets triggers an explicit feasibility report; it does not reopen Section 0 within this plan.”

**Exact bandwidth amendment:** “Stills have a configured cadence/byte bound; focus-triggered requests share that budget. Record skipped captures and resulting coverage limits. Gate A measures combined video, still, and event traffic alongside still legibility on the same workload.”

Content-change triggering alone does not bound bytes: scrolling and video can change continuously. Illustratively, one **500 kB** still every **10 seconds** for **8 hours** is **1.44 GB before video or events**. This is arithmetic, not a measured frame size or recommended cadence. The two-product approach is plausible, not yet proven within 1.5 GB/day. Sparse stills need not preserve every video frame at OCR quality.

## 3. #15 Apple Bridge remedy — AGREE

This is the same remedy proposed in round 1: import existing app-day estimates now; add a bounded raw InFocus-transition export to Apple Bridge; reconstruct estimated intervals on Hyperthymesia's server after qualification. Include native timing/device identity, decoder provenance, gaps, and lock evidence where available. Missing lock evidence remains missing. Transition support is not required to ship the honest aggregate view.

Current code confirms the distinction: [UsageSource.swift:68](/Users/johansardar/apple-bridge/native/Sources/UsageSource/UsageSource.swift:68) exports rollups, and [UsageRollup.swift:19](/Users/johansardar/apple-bridge/native/Sources/UsageSource/UsageRollup.swift:19) caps sessions at four hours. Its output explicitly labels unproven parity, omitted unclosed sessions, and delayed remote batches. Owning the bridge makes extending it practical; it does not itself establish phone coverage or second-accurate timing.

## 4. #26 Gate A scope — AGREE WITH AMENDMENT

Keep exactly the four deliverable categories Claude proposed. **Replace their acceptance wording with:**

1. **Native skeleton:** signed menu-bar process with focus/AX/event-tap/SCK **and hardware encoding/still output**; one eight-hour soak including a bounded stop/start and sleep/wake exercise. Report memory growth, dropped observations, and teardown results instead of claiming to prove “no leaks.” Use an already-authorized environment; unavailable permissions are a reported limitation, not a prompt triggered by this review.
2. **Measurement report:** the same three workloads—static reading, coding/builds, video—with CPU, energy, memory, combined bytes, and text legibility at a stated resolution and cadence. Assess sample stills with one available server OCR option; no OCR-engine tournament or mandatory on-device comparison.
3. **Versioned contract:** envelope v1 and connector protocol, including semantics for clocks, identities/revisions, coverage, and policy—not merely field names. Freeze v1, not future schema evolution.
4. **Source fixtures:** small real fixtures from T3, Codex, Claude Code, and Apple Bridge sufficient to check those contracts. No production adapters or new bridge export required in Gate A.

**Timebox amendment:** “Set the engineering time budget before starting. At expiry, publish pass/fail/unknown results and bounded follow-ups; do not extend the spike silently or treat unknown as passed. Block only work that depends on an unresolved result.”

Encoding and legibility are necessary within these deliverables: SCK callbacks and byte counts alone cannot establish that the proposed visual evidence survives transport. The broader compatibility matrix, exhaustive workloads, and production connectors remain outside this spike.

## 5. Scope cut #1: boundary error bars — AGREE WITH AMENDMENT

Formal error bars on every boundary are deferred.

**Exact amendment:** Replace “v1 computes exposure and coverage exactly” with “v1 computes durations deterministically over observed intervals, at the recorded source resolution and subject to explicit coverage gaps and timing uncertainty.” Engagement remains an inference; its stated bounds are estimates, not calibrated confidence intervals. Unknown edges must remain representable.

This removes an unsupported accuracy guarantee without requiring a statistical estimator.

## 6. Scope cut #3: attention weights — AGREE

Per-source lanes, disjoint task interval sets, union-based elapsed coverage, and honest aggregate cards are sufficient. An optional `primary` lane is an inferred label with `unknown` allowed; it does not convert overlapping exposure into measured exclusive human attention. No fractional attention allocation is required in v1.

## 7. Scope cut #9: per-region coverage — AGREE

Per-surface extraction coverage, including partial/unknown/truncated states, is enough. Keep sparse foreground keyframes even when text exists. Region maps and selective region OCR can wait; neither is required to stop treating nonempty text as complete visual coverage. The accepted exact-hash versus perceptual-hint distinction remains intact.

## 8. Scope cut #22: learned secret classifier — AGREE WITH AMENDMENT

No learned classifier is required; I did not intend one as a prerequisite. Shared capture policy, restricted unknown data, server-enforced egress, and inherited restrictions are the correct minimum.

**Exact amendment:** “Explicit exclusions apply before any local content persistence, including stills/video. If private-window exclusion cannot be established reliably, suppress affected content and emit a metadata-only gap. Apply restrictions to raw blobs, OCR, logs, backups, API/evidence reads, and model/embedding requests as well as derived summaries. Producer flags alone never grant disclosure.”

This closes alternate content paths; it does not add a classification research project. Without it, suppressing one text feed could still persist or disclose the same excluded content through pixels.

## Consensus status

- **Settled:** Section 0; Claude's accepted round-1 items (#2, #4, #5, #7, #8, #10–14, #16–21, #23–25); Apple Bridge aggregates now and qualified transition export later (#15); no v1 attention weights (#3) or region maps (#9). I accept deferring universal formal error bars and a learned classifier.
- **Awaiting assent to exact amendments:** the scope rule, server-only OCR and bounded still sampling (#6), Gate A acceptance wording/timebox (#26), honest duration wording (#1), and complete privacy-path enforcement (#22). These are my final requirements; their text above is the proposed common plan, not a claim that Claude has already accepted it.
- **Remaining architectural dispute:** none if those amendments are adopted. I require no additional pipeline, connector, statistical model, or research round to approve the resulting design.
