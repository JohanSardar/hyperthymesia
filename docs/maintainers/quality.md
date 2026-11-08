# Code-quality scope

The OSS pass removes the unreferenced digest_probe executable example and adds
EditorConfig, explicit rustfmt/clippy configuration, tooling syntax checks and
full-history privacy scanning in CI. Rust source has no TODO, FIXME, todo! or
unimplemented! placeholders at this checkpoint. Runtime prompt text is embedded
by include_str! and must not be mistaken for obsolete agent planning files.

Two large server modules now have explicit internal boundaries:

- fx delegates digest repair excerpts and evidence time support to
  `fx/citations.rs`; its existing tests live in `fx/tests.rs`.
- mcp delegates shared tool schema/discovery/argument admission to
  `mcp/schema.rs`; its existing tests live in `mcp/broker_tests.rs`.

Existing public paths and test names stay unchanged. Function bodies and
security policy are retained, not redesigned. The main fx file falls from 1460
to roughly 900 lines; mcp falls from 1606 to roughly 1100. This is not a claim
that all server modules are now small. Further separation of processing,
dashboard and source administration remains ordinary refactoring debt and needs
its own regression-backed work, rather than a broad pre-release rewrite.

The fixture-extraction test previously depended on a developer Cargo setup and
could reuse a real HOME anonymization key. It now runs under a scratch HOME with
an explicit synthetic key; the helper no longer assumes .cargo/env or a
developer-specific target directory. The original missing-environment failure
was reproduced before fixing it. No captured owner data is used to pass tests.
