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
