import { lazy, Suspense, type ReactNode } from "react"
import { createBrowserRouter, Link, Navigate, Outlet, RouterProvider, useLocation, type RouteObject } from "react-router"
import { AddConnectionPage } from "./connections/add"
import { BrowsePage } from "./connections/browse"
import { ConnectionPage } from "./connections/connection"
import { CatalogDetailPage } from "./connections/detail"
import { FirstRunPage } from "./first-run/first-run"
import { AgentPage } from "./pages/agent"
import { ConnectionsPage } from "./pages/connections"
import { DevicesPage } from "./pages/devices"
import { LivePage } from "./pages/live"
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
  { path: "today/live", element: <LivePage /> },
  { path: "today/:day", element: <TodayPage /> },
  { path: "timeline", element: <TimelinePage /> },
  { path: "timeline/:day", element: <TimelinePage /> },
  { path: "search", element: <SearchPage /> },
  { path: "connections", element: <ConnectionsPage /> },
  { path: "connections/browse", element: <BrowsePage /> },
  { path: "connections/new", element: <AddConnectionPage /> },
  { path: "connections/catalog/:id", element: <CatalogDetailPage /> },
  { path: "connections/:id", element: <ConnectionPage /> },
  { path: "agent", element: <AgentPage /> },
  { path: "runs", element: <RunsPage /> },
  { path: "runs/:id", element: <RunsPage /> },
  { path: "devices", element: <DevicesPage /> },
  { path: "devices/:device", element: <DevicesPage /> },
  { path: "settings", element: <SettingsPage /> },
  { path: "settings/:page", element: <SettingsPage /> },
  { path: "kit", element: <Suspense><KitPage /></Suspense> },
  { path: "*", element: <Navigate to="/today" replace /> },
]

// First run until setup finishes. Installations from before in-app setup
// count as set up once a model is chosen; nothing is preselected.
function FirstRunGate({ children }: { children: ReactNode }) {
  const { data } = useSettings()
  const location = useLocation()
  const setup = data?.settings.setup
  const unfinished = data ? (setup ? !setup.finished : data.settings.model === null) : false
  if (!unfinished) return children
  // The optional connections step may open a connection's page and come back.
  if (setup?.step === "connections" && location.pathname.startsWith("/connections")) {
    return (
      <div className="setup-return-frame">
        <div className="setup-return">
          <span>Setting up Hyperthymesia</span>
          <Link to="/setup" className="btn" data-variant="primary" data-size="xs">
            Back to setup
          </Link>
        </div>
        <div className="setup-return-body">{children}</div>
      </div>
    )
  }
  return <Navigate to="/setup" replace />
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
