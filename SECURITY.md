# Security policy

Hyperthymesia is experimental software that can collect sensitive personal data.
The latest release receives fixes; older 0.x versions are not maintained.
No version is claimed to be audited, sandboxed or production-safe.

## Report a vulnerability privately

Open a **confidential issue** in this project on GitLab
(https://gitlab.com/johansardar/hyperthymesia/-/issues/new, tick "This issue is
confidential"). Only the reporter and maintainers can read it. Do not open a public issue with
exploit details, credentials or captured personal data. Include the affected
version, impact, minimal synthetic reproduction and suggested mitigation.
Do not send raw evidence, databases, provider login files or server.env.

The maintainer will investigate and coordinate a fix and public advisory.
There is no guaranteed response SLA or paid bug bounty. If you receive no
response, follow up in the private report rather than posting sensitive details.

## Trust boundaries

- The server is loopback-only. Use a private HTTPS proxy or SSH tunnel for remote
  access. Do not expose it to the public internet.
- State, source keys and backups are **not encrypted at rest** by the project.
  Protect the account, disk and backups. Backups omit source keys/server.env.
- The model agent can run host bash under your account. `HX_HOST_BASH=off`
  removes that tool, not source-command execution. It is not a sandbox.
- A cloud model can receive permitted evidence and source responses. Read the
  README disclosures before connecting sources or capture.
- `read_raw` tokens expose evidence and can administer authorized sources.
  Revoke a compromised token immediately; rotate affected provider/source keys.
- Source switch-off is not erasure. GC is disabled. Retained versions, failed
  upgrade copies, databases, keys and backups require deliberate protection.
- Release SHA-256 detects tampering/corruption relative to reviewed installer
  pins. It is not a code signature or a guarantee against a compromised release
  account. Ad-hoc macOS builds are not notarized.

Never post suspected credentials in a PR. If published history contains a real
credential, revoke it first. History rewriting requires a separate owner decision
and cannot make already copied data private again.
