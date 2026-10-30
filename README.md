# Hyperthymesia

An exact, meaningful replica of every day, built from everything your devices observe.

Not "was on Chrome for 40 minutes" but "spent 22 minutes comparing standing desks on the laptop and phone, then ordered one", with every claim linked to the evidence behind it, and "unknown" where nothing was observed.

> Status: early build. The design is settled ([docs/DESIGN.md](docs/DESIGN.md)); the components are being built in parallel.

## How it works

```
Mac capture agent ─┐
Browser / shell /  ├─▶ server: raw events ─▶ normalized ─▶ timeline (lossless) ─▶ meaning (LLM, every 4 h)
editor companions  │                                                              │
Connectors ────────┘   T3 Code · Codex · Claude Code · git · Apple Bridge · …     ▼
                                                                    REST API · dashboard · (MCP)
```

- **Devices stay thin.** They capture focus, input activity, accessibility text, low-bitrate screen video and sparse stills, then upload. All processing, OCR included, happens on the server.
- **Instrument the app, not the pixels.** Agent sessions, the browser, the editor, the shell and git carry most of the meaning. Screen capture is evidence and fallback.
- **Honesty over false precision.** Device state, input, foreground exposure, inferred engagement and coverage are kept separate. Gaps are `unknown`, never "idle".
- **Modular.** The core knows one envelope format ([schema/](schema/)) and a small set of observation kinds. Every source is a connector.

## Layout

| Path | What |
|---|---|
| `crates/hx-schema` | Envelope v1, core kinds, connector protocol (JSON Schema in `schema/`) |
| `crates/hx-macos-sys`, `hx-agent`, `hx-agent-app` | macOS capture agent and menu-bar app |
| `crates/hx-store`, `hx-jobs`, `hx-server` | Server: Postgres store, job queue, API, dashboard |
| `crates/hx-compile` | Deterministic timeline compiler |
| `crates/hx-interpret`, `hx-visual` | LLM interpreter with claim checks; server OCR |
| `crates/hx-connector-sdk`, `connectors/*` | Connectors for agent sessions, git, Apple Bridge |
| `companions/*` | Browser extension, shell hook, editor extension |
| `docs/` | Design, review debate, build playbook |

## Develop

```sh
docker compose up -d          # Postgres + pgvector on localhost:55432
./scripts/check.sh            # fmt, clippy -D warnings, tests
```

Requires Rust 1.99 (pinned in `rust-toolchain.toml`), Docker, and macOS for the capture agent.

## License

Not yet chosen. The bundled pstack skills under `.claude/skills/` are MIT licensed ([PSTACK-LICENSE](.claude/skills/PSTACK-LICENSE)).
