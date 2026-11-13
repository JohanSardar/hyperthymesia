# Documentation

Start with the [project README](../README.md) and [installation](install.md).
Hyperthymesia is experimental. A healthy server is not proof of complete capture
or correct model-generated claims.

## Use it

- [Installation, providers, upgrades and recovery](install.md)
- [Mac permissions and pairing](mac-pairing.md)
- [Guided setup for AI agents](setup-for-agents.md)
- [Server, authentication, API and disclosure policy](../crates/hx-server/README.md)
- [Provider limitations](provider-limitations.md)
- [Garbage-collection safety](gc-safety.md)
- [Legacy PostgreSQL migration](moving-to-sqlite.md)
- [Troubleshooting](troubleshooting.md)
- [Task guides](guides.md)

## Reference

- [Every server, mover and connector command/option](reference/cli.md)
- [Mac commands and GUI](reference/macos.md)
- [HTTP contracts](reference/api.md) and [complete routes](reference/routes.md)
- [Configuration](reference/configuration.md) and [all environment names](reference/environment.md)
- [Run-scoped MCP tool schemas](reference/mcp-tools.json)

## Extend it

- [Connector protocol](../crates/hx-connector-sdk/README.md)
- [Interval compiler](../crates/hx-compile/README.md)
- [Interpretation and citations](../crates/hx-interpret/README.md)
- [Mac capture agent](../crates/hx-agent/README.md)
- [Browser companion](../companions/browser-ext/README.md),
  [editor companion](../companions/editor-ext/README.md),
  [shell hook](../companions/shell-hook/README.md)
- [JSON schemas](../schema/)

## Maintain it

- [Release process](releases.md)
- [Historical synthetic installation evidence](install-e2e.md), not a test of
  the current release

Crate-local `src/` and `tests/` hold implementation and integration tests.
`fixtures/` holds synthetic/anonymized input; `scripts/` holds repeatable checks
and builds; `companions/` and `connectors/` are separate producer integrations.
Build output belongs in ignored `target/`, crate `dist/` or root `dist/`, never
in a release's source tree.
