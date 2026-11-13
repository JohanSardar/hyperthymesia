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

- `src/styles/tokens.css`: colours, type, radii, shadows and motion, for dark and light.
- `src/ui/`: the components every screen is built from. Import them from `src/ui`.
- `src/shell/`: the sidebar, the main panel and `PageHeader`.
- `src/pages/`: one file per sidebar page.
- `src/first-run/`: the first-run steps at `/setup`, shown until a model is chosen.
- `src/model/`: the model sheet (providers, key or sign-in, model list, effort), used by first run and Settings.
- `src/server/`: typed calls to the JSON API (`docs/reference/app-api.md`) and the shared settings state.

`bun run dev` also serves `/app/kit`, a page showing every component in both themes. It isn't in the build.
