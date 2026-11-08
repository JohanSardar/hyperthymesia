# Open-source readiness audit

Baseline: 2026-11-08, Hyperthymesia `33cbc2e`. Comparison:
[`vercel-labs/fx`](https://github.com/vercel-labs/fx) at
`3d90fe9254eac209ddf55fccc16984651e6f20a8`, shallow-cloned outside this repository.
fx is the closest Vercel project: a native agent CLI, model-provider integration,
downloadable binaries, curl installation, and an extensible local tool boundary.
It is also Hyperthymesia's pinned model runner. Next.js, SWR and AI SDK are
frameworks/libraries, not comparable installable native applications.

This is an evidence-based comparison, not a claim that fx meets every requested
standard. Its README includes artwork/badges, its root includes agent instructions,
and no root SECURITY.md, CODE_OF_CONDUCT.md or ARCHITECTURE.md was present.
The target is the requested standard where that is stricter than fx.

| Area | Where Vercel is | Where we are (before) | Gap | Fix and acceptance evidence |
| --- | --- | --- | --- | --- |
| README | Pitch, first-screen curl, features, quick start, docs and Apache license | Long safety preamble; source build and separate runner download; no public release | High friction; install not in first screen | Short pitch/install/30-second local verification/features/docs/AGPL license first; preserve full safety disclosure immediately below; execute quick start |
| Docs | Hosted task/configuration/CLI/SDK manual, `--help`, examples | Scattered guides, partial server REST reference, historical probes mixed with instructions | Command/option/API coverage and runnable examples not mechanically checked | Docs index, generated CLI/environment/route inventory, rustdoc, task guides and troubleshooting; CI drift checks and safe examples |
| File structure | `src`, `tests`, `scripts`, `examples`, SDK; also AGENTS.md and `.fx` | Cargo workspace, crate-local tests, companions, connectors, fixtures; tracked build log and agent planning/review docs | Internal notes in public tree; no obvious examples/index | Delete tracked internal logs/plans/agent files, retain live operational guides and synthetic fixtures, document workspace map; leave pre-existing untracked work untouched |
| Code quality | Zig formatter, boundary checks, unit tests, fuzz/model/benchmark jobs | Rust workspace lints and check script; TS typecheck/build not enforced; CI lock fallback | Non-reproducible companion install; missing combined checks; large server modules | Enforce locked installs, format/lint/typecheck/build/tests; audit dead/TODO code, make only test-backed changes, report remaining module debt honestly |
| Architecture | Source map in README; no root architecture document | Crate boundaries exist but DESIGN/specs mix intent with old implementation | No short current module/data/trust-boundary map | ARCHITECTURE.md derived from imports/current store, plus rustdoc navigation |
| Tests | Unit and end-to-end suites in CI | Extensive Rust integration/unit tests; installer/companion jobs; check.sh omits companions/installer | No single all-local entry point; real-account/TCC tests need owner | One verification command, deterministic synthetic quick-start test, all PR CI; separate opt-in Postgres and capture/provider tests explicitly |
| CI and releases | Multi-platform build/release pipelines and CHANGELOG | CI/release workflows exist and latest main passed; main unprotected; server/app 0.1.2 (library default 0.1.0), no release/changelog | No enforced required check, no published artifacts/SemVer history | Required aggregate CI, timeouts/read-only token; Keep a Changelog; patch release, version-tag checks, checksum release notes |
| Contributor files | CONTRIBUTING, LICENSE, issue template; security contact in README | AGPL license only; very small ignore file | No contribution/security/conduct/PR guidance; secrets not broadly ignored | Contributor/security/conduct files, issue/PR templates, build and secret ignores; enable private vulnerability reporting when permitted |
| Install experience | `curl` installs native CLI under local bin, public releases | Mature user-service rollback installer; latest-release sums, no Pages pin, no PATH update | No public one-liner, independent pin, Bash PATH test, live clean-machine proof | Pages `/docs/install` pins VERSION/SHA256; verifies release checksum manifest before delegating to same-release installer; safe lifecycle preserved; six requested cases and live clean-host verification |

## Initial evidence and priorities

- GitHub repository is already public; authenticated access is ADMIN. No visibility
  change is needed or permitted. No branch protection or Pages site existed.
- Latest main CI passed. Existing release workflow previously passed artifact-only
  runs, but no GitHub release or tag was published.
- Tree + branch/remote/tag history privacy scan: **0 unapproved, 232 approved**
  pattern matches. Approvals cover public project URLs, public author metadata,
  and a synthetic snapshot. This is not proof that arbitrary secrets do not exist.
- Manual inspection found private process/machine context in `.audit/build.tsv`
  and obsolete agent planning/spec/review documents. Delete from the current tree;
  **published history still contains them**. No history rewriting is authorized.
- Browser companion: locked install, typecheck, build, 8 tests passed. Editor:
  locked install, typecheck, build, 1 test passed.
- Local Docker daemon is unavailable. A scratch HOME is not a clean machine.
  Use disposable GitHub-hosted infrastructure for the live-release proof.

Order: CI gate → contributor/privacy/tree hygiene → architecture → test entry
point → code-quality enforcement → reference/task docs → release metadata →
installer lifecycle/Pages → live pin-only update → README stranger pass.
Each area uses its own branch and PR; no merge before its actual CI succeeds.
See [decisions](decisions.md) for choices and [validation](validation.md) for
measured results and remaining limits. Rows describe the initial state; completion
is recorded with evidence, never inferred from the presence of a file.

## Implemented checkpoint

| Area | Before | Verified change |
| --- | --- | --- |
| CI | Unprotected main and incomplete local checks | #86 requires green/current CI, including admins; #88 unifies platform-aware locked checks |
| Tree/privacy | Obsolete build, agent planning and review notes tracked | #87 removes these from the current tree; history deliberately retained |
| Contributors | License only | #89 adds contributor/security/conduct policies, issue/PR templates and comprehensive ignores |
| Architecture | Mixed historical design notes | #90 adds the current crate/data/trust-boundary map |
| Quality | Large combined helper/test modules; developer-dependent fixture setup | #91 adds enforcement, splits citation/MCP schema/test boundaries and isolates fixture keys/HOME; remaining module debt is explicit |
| Docs/README/install/release | Source-first and no public release | Prepared locally with generated references, concise install-first README, atomic PATH setup and 0.1.3 integrity-checked release notes; final area merges/public activation are not yet claimed here |

The combined candidate passes all local checks. Independent tracked-source and
all-history secret scans have no unclassified findings, but removed historical
notes remain public. See [validation](validation.md) for exact counts, scope,
failures corrected and the separate clean-host acceptance gate.
