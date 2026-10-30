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
recording is explicit and does not make a live request. Author synthetic responses
in the integration test, then regenerate with `HX_WRITE_SYNTHETIC_FIXTURES=1`.
Normal tests replay committed responses only.

Requested future contract improvements: typed authorized sensitivity/lineage in
`EvidenceItem`, typed compiler/time support in `Packet`, entity assertions with
prior evidence references, and a mutable validation result instead of the frozen
immutable `check` signature. No existing public signature was changed.
