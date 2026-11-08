# Troubleshooting

| Symptom | What to do |
| --- | --- |
| `hx-server: command not found` | Open a new shell or export `PATH="$HOME/.local/bin:$PATH"`. Check the managed symlink. Do not overwrite an unrelated command. |
| Missing state option | Use the same `--state-dir` or `HX_STATE_DIR` every time. Managed installs remember it in `.state-dir` under the program prefix. |
| State doctor fails after a manual migrate/restore | It also checks private server.env/server.log. Follow the source/setup or restore runbook; never loosen file modes to hide the error. |
| Provider doctor fails but health is `ok` | Service/storage readiness and provider readiness are different. Load server.env, log in explicitly and check model/provider settings. Installation makes no login or paid request. |
| Linux user bus/linger missing | An administrator must provision `loginctl enable-linger USER`, then start a new login session. The installer never runs sudo. |
| No macOS GUI domain | Log in to a macOS desktop session. A LaunchAgent is not a system boot daemon. |
| Unmanaged path / exit 3 | Keep the existing path. Inspect it before deliberately moving it; there is no force flag or automatic legacy adoption. Symlinked/non-regular shell startup files are also refused. |
| Checksum mismatch | Stop. Nothing should be executed from that download. Retry the reviewed Pages installer; do not disable checksum verification or substitute a latest-release URL. |
| Upgrade refused for active lease | Wait for paid work to finish, then rerun. Do not delete locks or kill a model just to force an upgrade. |
| Missing registered core / pending recovery | Reconnect the state volume and follow [recovery](install.md#upgrade-and-recovery). Keep failed state, backups, keys and pending records. Never create replacement empty history accidentally. |
| Pair link expired or used (410) | Mint another link. A 503 can be retried with the same link. Re-pairing a device requires revoking its old server token. |
| Dashboard lacks evidence | Check token prefixes/scopes and current disclosure policy. Pairing tokens ingest; they do not grant dashboard reads. |
| Mac capture absent | Open the app and grant permissions deliberately. Missing capture is unknown; ad-hoc rebuilds can reset grants. See [pairing](mac-pairing.md). |
| Disk use grows | GC is disabled. Retained upgrades/backups/failed copies are intentional. Do not delete evidence or recovery files blindly. |

Report a minimal synthetic reproduction with version/OS and redacted error text.
Do not attach tokens, state databases, source keys, server.env, screenshots or
raw activity. Vulnerabilities use the private [security policy](../SECURITY.md),
not public issues.
