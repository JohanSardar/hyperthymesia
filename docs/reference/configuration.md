# Configuration

The server requires `--state-dir` or `HX_STATE_DIR` for state operations.
Installed service configuration lives in private `server.env` under the remembered
state directory. The service sources it; a foreground CLI does not.
Never attach that file to an issue. Restart the service after changing it.

| Setting | Default / meaning |
| --- | --- |
| `HX_APP_DIST` | Optional runtime web build directory; otherwise debug uses `app/dist` when present and release uses the embedded web build |
| `HX_STATE_DIR` | No implicit state directory; CLI `--state-dir` takes precedence |
| `HX_PROVIDER` | `fx`; explicit `stub` for synthetic development or legacy `claude` |
| `HX_FX_PROVIDER` | No default; `vercel`, `codex`, `grok` (`gateway` is a CLI alias) |
| `HX_FX_MODEL` | No default; choose a model your selected provider supports |
| `HX_FX_EFFORT` | No default; optional, depends on the selected model |
| `HX_FX_BINARY` | `$HOME/.local/bin/fx` in source runs; installer sets its private fx 0.0.16 |
| `HX_FX_CREDENTIAL` | Provider login file override; normally under `~/.fx/` |
| `HX_FX_TIMEOUT_S` | 1200; positive seconds, invalid/nonpositive values use default |
| `HX_FX_MAX_STEPS` | 120; positive integer, invalid/nonpositive values use default |
| `AI_GATEWAY_API_KEY` | Gateway credential; store privately, never in source control |
| `HX_HOST_BASH` | Enabled unless exactly `off`; not a sandbox switch for source commands |
| `HX_DISCLOSURE` | Built-in laptop policy; `off` or JSON rules file overrides it |
| `HX_EGRESS_SENSITIVE` | Full eligible text unless `redact`; restricted input remains withheld |
| `HX_MODEL` | `sonnet` for the legacy Claude CLI, unrelated to fx selection |
| `RUST_LOG` | tracing filter; avoid verbose logs around private data |

See [all production variable names and source definitions](environment.md),
[every CLI option](cli.md), [provider limits](../provider-limitations.md), and
[server disclosure rules](../../crates/hx-server/README.md#server-owner-disclosure-policy).
Internal installer/recovery variables in the inventory are implementation
interfaces, not a way to bypass ownership, checksums or migration checks.
`FX_AUTO_UPGRADE=0`, `FX_DISABLE_KEYCHAIN=1` and `FX_NO_OPEN_BROWSER=1` are fixed
in managed model execution; do not assume a globally installed fx is used.

The app persists settings in `core.sqlite`; saved app settings override these
provider/model environment values. See [app API](app-api.md).
