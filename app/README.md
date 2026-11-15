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
- `src/pages/`: one file per page.
- `src/first-run/`: the first-run steps at `/setup`, shown until a model is chosen.
- `src/model/`: the model sheet (providers, key or sign-in, model list, effort), used by first run and Settings.
- `src/server/`: typed calls to the JSON API (`docs/reference/app-api.md`) and the shared settings state.

`/app/kit` (⌘K → Component kit) shows every component in each of its states, in both themes side by side. It loads only when opened.

Keys: ⌘K palette, ⌘1–7 and ⌘, pages, ⌘B sidebar, ⌘[ and ⌘] back and forward.
