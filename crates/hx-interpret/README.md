# Interpreter integration

Use `EvidenceItem::classified` with the **server-authorized** sensitivity and
lineage flag before `build_packet`. Restricted returns `None`; Sensitive text is
fully redacted. Unknown/directly constructed evidence is excluded unless its kind
carries the constructor's classification prefix. This prefix is an internal
adapter, not authorization: never accept a model/producer-supplied prefix as an
egress grant. Mark all dashboard recaptures and derived Hyperthymesia output as
`self_generated`, including when the source kind does not name Hyperthymesia.
Entity memory and prior task inputs must also be server-egress-approved: their
frozen types carry no sensitivity or lineage metadata.

`interpret` owns provenance, retries at most twice, and runs deterministic checks.
After the last check failure it returns the repaired interpretation with audit
records; persistent JSON parse failures return an error. `check` preserves its
frozen immutable signature and reports repairs on a clone. Call
`check_and_repair` when validating externally loaded interpretations for use.
Repairs also replace free prose so removed claims cannot survive in summaries.

The validator proves reference membership, interval support and independent
corroboration presence. Explicit instruction-bearing evidence is ineligible to
support action claims. This is a conservative heuristic, not a general semantic
truth verifier; live model precision and joint-vs-staged evaluation remain
separate evaluation work. Existing task ids/aliases are supplied to the model;
semantic task identity across windows cannot be proven deterministically.

`entity_assertions` emits JSON assertion lines for claim objects prefixed with
`project:`, `repo:`, or `person:`. Supply checked interpretations only. Prior tasks
with matching stable ids extend the first-seen bounds; the evidence ids refer to
the checked current claims (prior task timing itself has task-level provenance).

`ClaudeCliProvider` uses tool-free, bare, nonpersistent Claude print mode. Tests
never invoke it. `RecordedProvider` only reads explicit digest-named fixtures;
recording is explicit and does not make a live request. Integration fixtures live
inside this crate’s `fixtures/` directory. Author synthetic responses
in the integration test, then regenerate with `HX_WRITE_SYNTHETIC_FIXTURES=1`.
Normal tests replay committed responses only.

Requested future contract improvements: typed authorized sensitivity/lineage in
`EvidenceItem`, typed compiler/time support in `Packet`, entity assertions with
prior evidence references, and a mutable validation result instead of the frozen
immutable `check` signature. No existing public signature was changed.

Packet selection (`interpret/v2`) uses the target `timeline.window`; callers can
compile the preceding 30 minutes but must set the target window before building
the packet. The server adapter supplies JSON with connector, subject, resource,
and payload, so threads/sessions/repos remain distinct even on shared lanes.
Without that metadata, selection uses lane attributes and point source keys.
Sensitive group identities are hashed and their content remains redacted.

`build_packet_with_budget` accepts `PacketBudget { max_chars, max_item_chars }`;
`build_packet` defaults to 400,000 total serialized UTF-8 bytes (a conservative
character limit, roughly 100k tokens for ordinary text) and 2,048 text bytes per
item. JSON escaping and all packet fields count toward the bound. Limits below
4,096 are raised to 4,096. No token estimator or environment variable is needed
for deterministic replay. Budget settings and the complete immutable inputs,
including omitted evidence, are part of `input_digest`.

Selection reserves 75% for evidence and up to 20% for timeline text; memory/tasks
use the remaining space and are removed first if serialization exceeds the cap.
Groups receive evidence in round-robin order: first and last messages, user
requests from unrepresented time quarters, then breadth-first temporal strata. First-round text is shortened to
share the budget fairly. Lead-in gets at most 5% of total bytes and 5% of selected
items, and never supplies target-window time support. If even the minimal item
metadata for all groups cannot fit, complete representation is impossible;
selection remains bounded and deterministic. Increase the configured budget for
such windows. Long individual content is truncated; raw observations remain
unchanged.

Timeline `group_digest` lines report in-window source counts/spans and retained
citation ids. Counts include omitted observations; spans describe source
extents, never continuous engagement. No semantic content is synthesized from
omitted messages. Detailed labels require all cited evidence to be present and
approved. Support JSON and timeline lines are kept whole, with support boundaries
sampled across time when necessary. Digests precede detailed noisy rows, and all
listed citation ids are retained packet evidence. Existing citation, instruction,
self-generated-content and interval checks still apply. Instruction-only evidence
is marked before truncation, so shortening text cannot turn it into independent
support for action claims.

`tests/selection.rs` generates a four-hour synthetic workload with 30 threads ×
40 long messages, 3,000 noisy events, and a 500-item lead-in burst. It checks
thread endpoints/prompts, all four hours, context share, byte bounds, digest
citations, deterministic replay, and a smaller explicit budget. No live model or
personal observations are used.
