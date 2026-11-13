# Provider limitations

Hyperthymesia pins fx 0.0.16. `HX_PROVIDER=fx` selects it;
`HX_FX_PROVIDER`, `HX_FX_MODEL` and `HX_FX_EFFORT` select the provider/model.
Nothing is preselected. The app lists the providers and models fx can use.

- Subscription providers use their own fx login.
- Vercel AI Gateway uses an API key saved privately through Settings, or
  `AI_GATEWAY_API_KEY` for CLI installations.
- Login files must be private regular files with one hard link. fx Keychain
  access and auto-upgrade are disabled. The installer keeps its own fx binary.
- `doctor` performs local checks, not a provider request. A passing doctor does
  not establish subscription eligibility, remaining usage, model availability,
  credential refresh, provider output compatibility or inference accuracy.
- Real Grok/Gateway logins, headless interactive login, and every provider/model
  effort combination are **not verified** by synthetic tests.
- Model calls may cost money or consume subscription usage. Installation never
  performs a login or a model request. Source connection setup and day-page runs
  can call models after configuration.
- The default runner is fx; no inference starts without a chosen model.
  `HX_PROVIDER=stub` is available for synthetic development. The legacy
  `HX_PROVIDER=claude` path requires a separately installed/authenticated Claude
  CLI; it is not bundled by the installer.

The agent's host bash is not sandboxed. `HX_HOST_BASH=off` removes its host bash
tool, but a configured source's commands still run under the server account.
Source keys and CLI logins are not isolated from that account. Source switches
stop requests/pulls, not access through a fully privileged host shell. They do
not erase credentials, evidence or backups.

See [installation](install.md) and the
[server disclosure contract](../crates/hx-server/README.md#api-and-disclosure)
before connecting sensitive sources.
