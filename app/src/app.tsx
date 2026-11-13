import { lazy, Suspense, type ReactNode } from "react"
import { createBrowserRouter, Navigate, Outlet, RouterProvider, type RouteObject } from "react-router"
import { AddConnectionPage } from "./connections/add"
import { BrowsePage } from "./connections/browse"
import { ConnectionPage } from "./connections/connection"
import { CatalogDetailPage } from "./connections/detail"
import { FirstRunPage } from "./first-run/first-run"
import { AgentPage } from "./pages/agent"
import { ConnectionsPage } from "./pages/connections"
import { RunsPage } from "./pages/runs"
import { SettingsPage } from "./pages/settings"
import { TodayPage } from "./pages/today"
import { SettingsProvider, useSettings } from "./server/settings"
import { Layout } from "./shell"

const KitPage = import.meta.env.DEV ? lazy(() => import("./pages/kit").then((m) => ({ default: m.KitPage }))) : null

const children: RouteObject[] = [
  { index: true, element: <Navigate to="/today" replace /> },
  { path: "today", element: <TodayPage /> },
  { path: "connections", element: <ConnectionsPage /> },
  { path: "connections/browse", element: <BrowsePage /> },
  { path: "connections/new", element: <AddConnectionPage /> },
  { path: "connections/catalog/:id", element: <CatalogDetailPage /> },
  { path: "connections/:id", element: <ConnectionPage /> },
  { path: "agent", element: <AgentPage /> },
  { path: "runs", element: <RunsPage /> },
  { path: "settings", element: <SettingsPage /> },
  { path: "*", element: <Navigate to="/today" replace /> },
]

if (KitPage) {
  children.push({ path: "kit", element: <Suspense><KitPage /></Suspense> })
}

// First run until a model is chosen; nothing is preselected.
function FirstRunGate({ children }: { children: ReactNode }) {
  const { data } = useSettings()
  if (data && data.settings.model === null) return <Navigate to="/setup" replace />
  return children
}

const router = createBrowserRouter(
  [
    {
      element: (
        <SettingsProvider>
          <Outlet />
        </SettingsProvider>
      ),
      children: [
        { path: "/setup", element: <FirstRunPage /> },
        { path: "/", element: <FirstRunGate><Layout /></FirstRunGate>, children },
      ],
    },
  ],
  { basename: "/app" },
)

function App() {
  return <RouterProvider router={router} />
}

export { App }
