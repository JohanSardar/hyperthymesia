# Task guides

## Verify a source build without a model account

Build and exercise the real local server against private throwaway state:

```sh
cargo build --locked -p hx-server
python3 scripts/quickstart.test.py
```

The smoke test runs migrate, state doctor/status, loopback health, authentication,
online backup and restore. It uses the stub provider, never captured data or
a paid model. A passing smoke is not proof of working cloud authentication.

## Connect and switch off a source

Follow [Connect a source](../README.md#connect-a-source). Use a read-only,
narrowly scoped login/key. Connection setup can invoke a paid model.
Inspect **What your agent set up**, then verify saved evidence/citations.
Use **Switch off** to stop requests/pulls; this does not remove credentials,
tables, saved evidence, or access through privileged host bash.

## Back up, upgrade and recover

The [installation recovery guide](install.md#upgrade-and-recovery) is the
authoritative runbook. Choose a new backup destination; backups omit source
keys/server.env, so protect those separately. The real CLI smoke above exercises
online backup and restore with synthetic metadata. Never rehearse by replacing
your live history. `status` lists retained copies; nothing is pruned automatically.

## Add a producer

Implement the [connector contract](../crates/hx-connector-sdk/README.md).
Reuse envelope identities on retry, validate schemas, checkpoint only after a
matching acknowledgement, and report unknown coverage. Do not write directly to
server SQLite. Run existing durability and fixture privacy tests:

```sh
cargo test --locked -p hx-connector-sdk
```

## Change a documented interface

Build workspace binaries, regenerate the reference, and review the diff:

```sh
python3 scripts/reference.py
python3 scripts/reference.py --check
```

The generator runs CLI help in an empty HOME/environment and exports MCP schemas
without state or a model. CI rejects generated-reference drift and broken local
Markdown links. Prose changes still need code review and a real reproduction.
