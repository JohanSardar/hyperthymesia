# Deterministic interval compiler

`compile` is pure and retains the frozen public types in `src/lib.rs`. Inputs are
normalized envelopes; an unrecognized or undecodable payload remains evidence as
a generic point. Synthetic replay fixtures live in `fixtures/traces`.

## Deterministic rules

- Canonicalize envelope dedup keys using the smallest server evidence row ID.
  Replayed copies refer to that canonical row. For source object revisions, prefer
  producer sequence, then collection time, then opaque revision text as a stable
  tie-break. Arrival order and server receive time do not establish revision order.
  A winning deletion removes current activity; historical revisions and tombstones
  remain evidence points. Historical aggregate evidence stays in aggregate cards.
- Compile each installation/device independently. Device lane keys acquire a
  `:source:<connector_instance>` suffix when multiple installations would otherwise
  have the same lane ID. Agent lanes also include the provider.
- Retain each focus, presence, and browser transition. Open spans end at their next
  transition or last evidence from the same collector, and are uncertain. An open
  span never resumes an old label after a coverage gap. Sleep, display-off, and
  lock transitions end foreground exposure.
- Collector identities use the kind namespace (`focus`, `presence`, `web`, etc.).
  `collector.health.collector` must use that namespace (the `.span` suffix is also
  accepted). Heartbeats/evidence expire after 60 seconds without more evidence;
  explicit observed spans support their recorded duration. Coverage is per device
  and collector, identified in `reason`, and may overlap across collectors. Paused
  status persists until the next health status. Failed/stopped/permission-denied
  status is Unknown. `policy.gap` with envelope start/end suppresses all collectors
  in that installation. Unknown coverage is never relabeled idle.
- Comparable boot/domain stamps determine span lengths, anchored at the recorded
  UTC start; otherwise use UTC. Raw duration provenance is in `attrs.duration`.
  Split or clipped interval lengths are authoritative for exposure; the attribute
  describes the original source span. This compiler does not estimate calibration
  offsets or repair inconsistent UTC starts across a backward wall-clock step.
- Agent messages remain points and group independently by thread into activity
  intervals between messages no more than 60 seconds apart, without a timeout tail.
  These bounds have uncertain edges and do not claim human attention.
- Shell durations support intervals. Calendar intervals carry `planned: true` and
  remain planned context. Aggregate cards use source civil days (UTC if no source
  offset is supplied); totals never become per-second exposure. Deleted aggregate
  evidence without a current card uses a zero-valued historical card.
- Every overlapping canonical revision remains reachable through intervals,
  points, or aggregate evidence. Span evidence points outside the window are
  anchored to its clipped edge; calendar points use the planned start. Source
  timestamps remain available through the evidence envelope.
- `exposure_by_label` unions interval sets across matching lane kinds and labels,
  including overlaps across devices. Different labels may overlap; their totals
  must not be added as human engagement.

## Contract follow-ups

No frozen signature or serde shape was changed. A future schema should define an
ordered source revision/generation and an explicit collector identity on each
observation, rather than relying on sequence/time precedence and kind namespaces.
A typed policy-gap payload would replace the current extension convention.

## Verification

After `source ~/.cargo/env`, with
`CARGO_TARGET_DIR=/Users/johansardar/Hyperthymesia/.target-core`:

```
cargo test -p hx-compile
cargo clippy -p hx-compile --all-targets -- -D warnings
cargo fmt -p hx-compile -- --check
```

The seven handwritten truth files cover normal switching, a six-second interleave,
sleep, a monotonic wall-clock jump, an unknown collector gap, phone aggregates,
and a late upsert. Additional behavior tests cover clipping, policy suppression,
thread interleaving, source isolation, evidence retention, replay invariance, and
union duration bounds. An insta snapshot guards the short-switch trace.


## Open-transition performance regression

The four-hour synthetic workload contains 20,000 observations: 12,000 open
`web.visit` transitions, 4,000 open focus transitions, and 4,000 browser collector
heartbeats. Construction and evidence checks are outside the compilation timer.
The normal test suite runs it in both profiles; release builds enforce a two-second
compilation limit. A second 20,000-observation test alternates paused/running
heartbeats to exercise thousands of coverage gaps under the same limit.

Temporary cumulative timers on the original compiler isolated the hot path on an
Apple M5 Mac: at 2,000 observations, compilation took 2.470 s, including 2.279 s
finding collector horizons and 0.176 s finding sleep/lock cutoffs. Both operations
scanned the whole active window per transition and repeatedly decoded payloads.
At 20,000 observations the original took 237.760 s: 219.878 s in horizon scans
and 17.779 s in sleep/lock scans. The optimized workload took 0.110 s, and the
gap-heavy variant took 0.121 s, measured individually in release on the same Mac.
These are individual development measurements, not a statistical benchmark.
Those profiling timers are not part of production code.

The compiler now builds collector horizons once and binary-searches sorted
sleep/lock timestamps and per-collector coverage-gap unions. Open spans stop at
the first overlapping gap. Collector and device identities, coverage output,
monotonic end caps, and the public interface are unchanged. This removes the
per-transition whole-window scans; it does not claim a universal complexity
bound for unrelated workloads such as many aggregate revisions.

`tests/support/reference.rs` freezes the original scan-based implementation as a
test-only oracle. Tests compare complete timelines on every `fixtures/traces/*`
fixture and on 256 random small inputs covering open/closed spans, health/policy
gaps, sleep/lock, independent installations/devices, revisions/deletions,
duplicates, and monotonic clocks. Keep this oracle independent of the indexes.

With the task's `CARGO_TARGET_DIR` exported, reproduce the measurements with:

```sh
cargo test -p hx-compile --release mixed_four_hour_window_under_two_seconds -- --nocapture
cargo test -p hx-compile --release gap_heavy_browser_window_stays_bounded -- --nocapture
```

PR #7's compiler follow-up is documented in `crates/hx-server/README.md` and the
ignored `synthetic_browser_point_stress` in `crates/hx-server/tests/server.rs`.
That crate is unchanged here. To enable it, remove its
`#[ignore = "known quadratic open-browser-transition cost in hx-compile; run separately in release"]`
attribute, then run the release hx-server test against its synthetic Postgres
fixture. The end-to-end 60-second bound includes ingest, normalization, and
interpretation across seven windows; this crate's timings alone do not verify it.
