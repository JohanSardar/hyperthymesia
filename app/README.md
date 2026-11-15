# Web app

The Hyperthymesia web app. hx-server serves the build from `app/dist` under `/app`.

```sh
bun install
bun run dev        # http://localhost:5173/app/
HX_SERVER=http://127.0.0.1:7377 bun run dev   # same, with API calls and sign-in sent to that hx-server
bun run typecheck
bun run build      # writes app/dist
```

Built with React 19 and the UI SDK (`@ui-sdk/react`). Before using it, read `node_modules/@ui-sdk/react/docs/llms.txt`.

- `src/styles/tokens.css`: every colour, type size, spacing step, radius, border and motion value, for dark (the default) and light. CSS never uses a raw value.
- `src/ui/`: the components every screen is built from. Import them from `src/ui`.
- `src/shell/`: the layout. The icon rail and the router list every page (`nav.ts`); the sidebar is resizable and shows a page's own content through `PageSidebar`; `PageHeader` is the bar on top of each page; `useRegisterCommands` adds a page's actions to the ⌘K palette; `Placeholder` is a page that isn't built yet.
- `src/pages/`: one file per page. Today (`/today`, past days at `/today/YYYY-MM-DD`) is the AI's account of a day, with a days list in the sidebar; the Live log is at `/today/live`. The timeline is at `/timeline` and `/timeline/YYYY-MM-DD?at=ISO`. Search (`/search?q=`) covers day-page entries and recorded screen text.
- `src/day/`: the day page's parts: coverage bar, days list and picker, and the citation sheet that opens the exact stored record behind any line (`src/lib/day.ts` holds the date and coverage maths).
- `src/first-run/`: the six first-run steps at `/setup` (intro, where data lives, permissions, AI, connections, proof), shown until setup is finished; progress is saved in `settings.setup` so it resumes where it stopped.
- `src/tour/`: the guided tour shown on Today after setup, replayable from Settings or ⌘K.
- `src/agent/`: the Agent and Runs pages' parts: provider sign-in (`signin.tsx`), the model test, plain problem wording with its fix (`problems.ts`) and run formatting.
- Agent (`/agent`) holds provider accounts, the default model and test, the schedule, the optional spending limit and connection setups. Runs (`/runs`, one run at `/runs/ID`) lists AI runs in the sidebar with a live log, cost, what each wrote, re-run and fixes; `?view=pulls` shows connection pulls.
- `src/model/`: the model sheet (providers, key or sign-in, model list, effort), used by first run and Settings.
- `src/server/`: typed calls to the JSON API (`docs/reference/app-api.md`) and the shared settings state.

`/app/kit` (⌘K → Component kit) shows every component in each of its states, in both themes side by side. It loads only when opened.

Keys: ⌘K palette, ⌘1–7 and ⌘, pages, ⌘B sidebar, ⌘[ and ⌘] back and forward. In ⌘K, typing anything offers a search of day pages and screen text. In the citation sheet, ← and → (or j and k) step through records.
