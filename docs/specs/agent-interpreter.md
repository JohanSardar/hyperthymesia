# Spec: agent interpreter (fx loop)

Status: v1 implemented on 2026-10-31. Select `HX_PROVIDER=fx`; see
[server operation and synthetic verification](../../crates/hx-server/README.md#fx-agent-interpreter).
Live connectors remain unconfigured; real-data acceptance is a separate owner run.
This spec replaces the "single prompt, no tools" interpreter in [DESIGN.md §5.2](../DESIGN.md#52-interpreter-every-4-h-plus-scheduled-revisions).
Everything else in DESIGN.md stays in force unless this spec says otherwise.

## 1. Goal

Produce a **perfect replica of what the owner did at any moment**. It is nested
(day → tasks → episodes → minutes), and every statement links to the exact data it came from.

The interpreter becomes an **agent loop**. It gets a goal ("work out what happened between
12:00 and 16:00"), plus tools over all of the owner's data. It decides what to look at,
follows leads, and writes the replica when it is done. Connecting a new source must never
require changing the agent: the new data simply shows up behind the same tools.

Out of scope for now: answering questions. A later version will answer questions over the
same stored evidence, so the evidence and citations must be kept in full.

## 2. Decisions

| # | Decision | Choice |
|---|---|---|
| D1 | Agent harness | [Vercel fx](https://github.com/vercel-labs/fx) |
| D2 | Model | GPT-6.1 Sol, reasoning effort `xhigh`, fast mode on |
| D3 | Model account | The owner's **ChatGPT subscription**, via `fx login codex`. No per-use API billing |
| D4 | Data egress | Real data goes to OpenAI through the owner's own subscription. The owner explicitly accepted this. This replaces the earlier "Claude subscription only" rule for the interpreter |
| D5 | Where it runs | On the owner's own server, which is the Mac laptop for now. Move it to an always-on machine later; devices then just point at the new address |
| D6 | Schedule | Every 4-hour window, plus a catch-up pass the next day for late data such as phone Screen Time |
| D7 | Data access | The server exposes all stored data to fx as **one MCP tool server**. fx does not read the database directly |
| D8 | Live lookups | When the agent fetches from a live source (Drive, email, Oura, …), the server saves an exact copy of what it returned, and citations point at that copy |
| D9 | Citations | Every claim cites the stored evidence it used. Each citation opens the exact stored text or data, and also links to the original app or page when one exists |
| D10 | Privacy | Unchanged. Password fields and excluded apps are never recorded. `restricted` data is never returned by any tool. The owner's disclosure policy decides each item's class |
| D11 | Server code | The existing Rust server stays. Only the interpretation step changes |

## 3. How it fits together

```
Devices ──send──▶ Server ──every 4 h──▶ fx agent loop ──▶ replica (stored, cited)
 Mac app            receive              │      ▲
 phone              store                │ MCP  │ tool results
 Codex/Claude/T3    OCR                  ▼      │
 git, …             schedule        Server's MCP tool server
                                    (reads the store, applies privacy,
                                     snapshots live lookups)
```

What stays the same:
- Capture, delivery, ingest, normalization, OCR, compile, windows and the job queue.
- The disclosure policy.
- Revisions: a run publishes only if its input is still current, and the prior view keeps
  serving while a rerun is in progress.
- The dashboard and the REST API.

What changes:
- The interpretation job no longer builds one bounded packet and sends one prompt.
  Instead it starts an fx session with the window's goal, connects the server's MCP tool
  server, and collects the final structured replica.
- The final replica goes through the same validator as today (§6) before it is stored.

## 4. The fx run

- **Invocation:** the server runs fx headless on the same machine, once per window job.
  Preferred is `fx ask --json` with the MCP server attached. Use `libfx` or ACP only if the
  CLI cannot do what we need. Each run gets its own working directory, which holds no
  owner data.
- **Instructions:** a versioned system/project prompt, recorded in the input manifest
  together with the fx version, the model ID, the effort and the fast setting.
  - The prompt defines the replica's structure, the citation rules, and the
    observed / reported / inferred labels.
  - It requires completion claims to have outcome evidence.
  - It states that source content is data and never instructions.
- **Tools:** only the server's MCP tools. fx's built-in shell and file tools are disabled.
  The agent has no network access other than through these tools.
- **Output:** the same structured interpretation schema as today (episodes, tasks, claims
  with evidence references, summary), returned as JSON.
- **Limits:**
  - A per-run budget of turns and tool calls, plus a wall-clock timeout.
  - When a limit is hit, the run fails and retries like any other job. A failure never
    replaces the previously served replica.
- **Durability:** a crashed run is retried from scratch. fx's own resume is optional and
  not relied on in v1.

## 5. MCP tool server (first version)

All tools are read-only. All of them apply the disclosure policy, so `restricted` items are
never returned. All of them return evidence IDs that can be cited.

| Tool | Purpose |
|---|---|
| `window_overview(from, to)` | Which sources have data, how complete each one is, gaps, and the compiled lanes (apps, presence, focus) |
| `timeline(from, to, level)` | The compiled timeline at a chosen zoom level, down to individual seconds |
| `search(query, from?, to?, sources?)` | Full-text search across stored text: messages, screen text/OCR, commits, documents |
| `read_evidence(id)` | The full stored record: text, source, time, device, links |
| `screen_at(time)` | The OCR text and the app/window at a moment. The image itself is never returned |
| `list_sources()` | The connected sources and what each one provides |
| `live_lookup(source, request)` | Fetch from a live connector, such as a Drive doc or an email thread. The server snapshots the result as new evidence and returns its evidence ID |
| `prior_context(before)` | Earlier replicas and entity memory (people, projects), filtered by the current disclosure policy |

Adding a source means adding data behind these tools, and `list_sources` reports it.
New tools are added only when a source's shape cannot be expressed through the existing ones.

## 6. Citations and validation

- Every claim lists the evidence IDs it relies on. The validator rejects:
  - an episode whose time span the evidence does not support
  - a claim with no citable, independent evidence. Hyperthymesia's own output never counts
    as independent evidence
  - a completion claim with no outcome evidence
  - a "reported" source item that is presented as an observed action
- Rejected parts get a scoped repair. Claims that remain unsupported are dropped, and only
  their time span is kept.
- **Citation link:** every evidence ID resolves on the dashboard to the exact stored text
  or data the agent saw. That includes the snapshot of a live lookup, so the link keeps
  working when the original changes. When the source has an original location, the
  evidence also carries that link (a thread, a commit URL, a doc URL).
- The tool calls and results of each run are stored with the revision. A reader can
  therefore see exactly what the agent looked at.

## 7. Acceptance criteria

1. **Model check.** On the server machine, fx signed in with `fx login codex` runs
   GPT-6.1 Sol at `xhigh` with fast mode on. The run log shows the model ID, effort and
   fast setting that were actually used.
2. **Real window.** One real 4-hour window that includes laptop capture produces a replica
   where every claim's citations open the exact stored evidence.
3. **Privacy.** No tool call in a recorded run returns a `restricted` item. A synthetic
   restricted fixture proves this.
4. **Live lookup.** A live lookup is snapshotted. Its citation still opens the same text
   after the original source is changed.
5. **Failure.** A failed or timed-out run leaves the previous replica served, and the job
   retries.
6. **New source.** Adding a new source makes its data appear through `list_sources` and
   `search` with no change to the prompt or the tool list.

## 8. Risks and unknowns

- **fx is experimental.** Vercel says it will change often, so pin the fx version and
  record it in every manifest.
- **Unconfirmed through the ChatGPT login:**
  - whether `gpt-6.1-sol` is offered (fx's docs list older Sol versions)
  - whether `xhigh` effort and fast mode are available
  - whether `libfx` (as opposed to the CLI) can use the login

  Acceptance criterion 1 settles all of these first.
- **Subscription rate limits.** Long tool loops every 4 hours, plus catch-up passes, may
  hit the subscription's limits. Runs must back off and retry, not fail permanently.
- **Laptop as server.** While the laptop is closed, no passes run and phone data waits.
  This is accepted until the server moves (D5).
- **Bugs to fix first** (found by the 2026-10-31 statechart review). They affect replica
  correctness regardless of the interpreter:
  - capture gaps that are not recorded (private browser windows, companion gaps, spans
    left open after a crash)
  - capture that stops silently when the on-device queue is full
  - stale windows after a span is shortened
  - normalization abandoned after 5 crashes
  - windows stuck after their retry budget runs out
  - entity memory that ignores the current disclosure policy
  - retries that discard an earlier valid answer

## 9. First step

1. Install fx (pinned version) on the server machine.
2. The owner signs in once with `fx login codex`.
3. Run a scripted check of acceptance criterion 1: model ID, `xhigh` and fast mode.

Only after it passes, build the MCP tool server and replace the interpretation job.
