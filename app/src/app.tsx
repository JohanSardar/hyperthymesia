import { lazy, Suspense } from "react"
import { createHashRouter, Navigate, RouterProvider, type RouteObject } from "react-router"
import { AgentPage } from "./pages/agent"
import { ConnectionsPage } from "./pages/connections"
import { RunsPage } from "./pages/runs"
import { SettingsPage } from "./pages/settings"
import { TodayPage } from "./pages/today"
import { Layout } from "./shell"

const KitPage = import.meta.env.DEV ? lazy(() => import("./pages/kit").then((m) => ({ default: m.KitPage }))) : null

const children: RouteObject[] = [
  { index: true, element: <Navigate to="/today" replace /> },
  { path: "today", element: <TodayPage /> },
  { path: "connections", element: <ConnectionsPage /> },
  { path: "agent", element: <AgentPage /> },
  { path: "runs", element: <RunsPage /> },
  { path: "settings", element: <SettingsPage /> },
  { path: "*", element: <Navigate to="/today" replace /> },
]

if (KitPage) {
  children.push({ path: "kit", element: <Suspense><KitPage /></Suspense> })
}

const router = createHashRouter([{ path: "/", element: <Layout />, children }])

function App() {
  return <RouterProvider router={router} />
}

export { App }
