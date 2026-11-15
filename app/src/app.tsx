import { lazy, Suspense, type ReactNode } from "react"
import { createBrowserRouter, Navigate, Outlet, RouterProvider, type RouteObject } from "react-router"
import { AddConnectionPage } from "./connections/add"
import { BrowsePage } from "./connections/browse"
import { ConnectionPage } from "./connections/connection"
import { CatalogDetailPage } from "./connections/detail"
import { FirstRunPage } from "./first-run/first-run"
import { AgentPage } from "./pages/agent"
import { ConnectionsPage } from "./pages/connections"
import { DevicesPage } from "./pages/devices"
import { RunsPage } from "./pages/runs"
import { SearchPage } from "./pages/search"
import { SettingsPage } from "./pages/settings"
import { TimelinePage } from "./pages/timeline"
import { TodayPage } from "./pages/today"
import { SettingsProvider, useSettings } from "./server/settings"
import { Layout } from "./shell"

// The kit is loaded only when opened, so it costs the other pages nothing.
const KitPage = lazy(() => import("./pages/kit").then((m) => ({ default: m.KitPage })))

const children: RouteObject[] = [
  { index: true, element: <Navigate to="/today" replace /> },
  { path: "today", element: <TodayPage /> },
  { path: "timeline", element: <TimelinePage /> },
  { path: "search", element: <SearchPage /> },
  { path: "connections", element: <ConnectionsPage /> },
  { path: "connections/browse", element: <BrowsePage /> },
  { path: "connections/new", element: <AddConnectionPage /> },
  { path: "connections/catalog/:id", element: <CatalogDetailPage /> },
  { path: "connections/:id", element: <ConnectionPage /> },
  { path: "agent", element: <AgentPage /> },
  { path: "runs", element: <RunsPage /> },
  { path: "devices", element: <DevicesPage /> },
  { path: "settings", element: <SettingsPage /> },
  { path: "kit", element: <Suspense><KitPage /></Suspense> },
  { path: "*", element: <Navigate to="/today" replace /> },
]

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
