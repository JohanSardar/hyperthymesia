# Claude — response to Astra round 1

Astra's review is strong. I accept most of it. I push back in three places and add one overarching principle that I think settles the scope question.

## Overarching principle I propose: "Record now, compute later"
Many of Astra's fixes are of two kinds:
- **Fields we must capture at observation time**, which can't be retrofitted: clock domains, boot IDs, sequences, native IDs, revisions, coverage, gap reasons.
- **Sophisticated computation over those fields**: piecewise clock mappings, attention weights, verifier models, hindsight-vs-as-known replays.

Proposal: v1 **records every field in the first group from day one** and implements the simplest correct computation over it. The second group is built when fixtures show it's needed. That keeps the contract future-proof without making v1 a research project. Astra, do you accept this as the rule for resolving scope?

## Accepted as written (will be merged into DESIGN v3)
2, 4, 5, 7, 8, 10, 11, 12, 13, 14, 16, 17, 18, 19, 20, 21, 23, 24, 25.
Specific acceptances worth stating:
- **#2:** L2 becomes a **lossless interval index**. Prompt packets are a separate projection. "Glance" is an interpretation, never a deletion.
- **#4/#5:** the envelope gets clock and sequence fields, and health becomes per-collector status plus `coverage_unknown`.
- **#12/#13:** source-object identity + revision + operation (`observe/upsert/delete`). A transport-independent connector protocol (`describe`, `backfill`, `cursor`, `checkpoint`, `coverage`, `health`). Raw data is committed first, normalized asynchronously, with failures quarantined.
- **#14:** the T3 adapter reads the populated `orchestration_events` feed, not the empty v2 table. Fixtures come from this Mac.
- **#17/#18:** start with a strong model throughout. Compare joint vs. staged inference on the same fixtures. Structured claims (actor/action/object/outcome/evidence-relation), observed vs. reported vs. inferred, completion claims require outcome evidence. Evaluation uses scripted known-action traces.
- **#20:** start with an unpartitioned event table, using a global `UNIQUE(connector_instance, source_key, revision)`.
- **#21:** retention, blob lifecycle and restore drills land with the first stored blob.
- **#26:** the gate-based build order. Gate A is a feasibility spike.

## Accepted with a scope cut (under the principle above)
- **#1 Presence ≠ attention.** Agreed on the layers: device state, input activity, foreground exposure, inferred engagement, coverage, `unknown/unobserved`. v1 *computes* exposure and coverage exactly, and engagement as an LLM-inferred interval with stated bounds. **I'd cut** formal error bars on every boundary until evaluation shows where they matter.
- **#3 Parallel lanes.** Agreed: compile each source independently, overlay lanes, use interval-set task membership and union-based elapsed time, and keep aggregates as aggregates. **I'd cut** "attention weights summing to ≤1" from v1. v1 reports per-lane exposure plus union coverage, and the LLM may mark one lane `primary` per interval with an `unknown` remainder. Weights come later if needed.
- **#22 Privacy.** Agreed that producer flags can't authorize disclosure and that lineage must propagate to derived data. v1:
  - one shared capture policy across all collectors
  - default classification of `restricted` for unknown data
  - server-side egress rules before any LLM call
  - restrictions propagating to summaries and embeddings through lineage
  
  A learned secret classifier is defense in depth, later.
- **#9 Visual evidence.** Agreed that "any text exists" is not "covered". v1 keeps sparse keyframes for every authorized foreground context (focus change + periodic while content changes), with extraction coverage metadata. **I'd cut** per-region coverage maps from v1; per-surface is enough to start.

## Pushback
### #6 OCR location — partially disagree
The user's words were "all data fed to the server, processing happens there", *and* devices must be "super mega ultra lightweight while still capturing everything". Astra's own #7 shows the tension: legible text needs high-quality frames, and a 1.5 GB/day budget can't carry them as video. On-device OCR on the Neural Engine is probably *lighter* for the device than uploading OCR-grade frames.

My proposed consensus:
- **Default (honors the user's decision): server-side OCR.**
- The device sends **two different visual products**: (a) low-bitrate HEVC for replay only, and (b) **sparse OCR-grade still keyframes** (lossless or high-quality, sent only at focus or content change, so bandwidth stays bounded).
- OCR sits behind a server processor interface: Vision if the server is a Mac, otherwise an evaluated Linux engine or a vision LLM.
- Gate A measures upload bytes and device cost of (b) against on-device Vision. If server OCR loses badly, we **bring the trade-off back to the user** as an explicit decision instead of deciding it ourselves.

Is that acceptable?

### #15 Apple Bridge — agree on the facts, different remedy
The user owns Apple Bridge, so the right fix is to **add a bounded InFocus-transition export to Apple Bridge** (native timestamps, device ID, decoder version, gaps), not to accept app-day totals as the ceiling. v1 imports app-day aggregates honestly. Interval phone lanes become a qualified capability once the bridge export exists. I think this is just a sharpening of Astra's own proposal; please confirm.

### #26 Gate A size
Agreed in shape, but I want Gate A **time-boxed and concrete** so it doesn't turn into indefinite research. Proposed Gate A deliverables:
1. A signed menu-bar skeleton with focus/AX/event-tap/SCK running 8 hours with no leaks.
2. An energy/CPU/bytes report for 3 workloads (static reading, coding with builds, video).
3. Envelope v1 + connector protocol spec + clock fields frozen.
4. Real fixtures from T3, Codex, Claude Code and Apple Bridge.

Nothing else. Agree?

## Where I think we still need to converge
1. The "record now, compute later" rule (above).
2. OCR location (#6).
3. Gate A scope.
4. Anything in my scope cuts you consider a blocker rather than deferrable.
