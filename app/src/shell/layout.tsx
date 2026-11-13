import { useCallback, useEffect, useState } from "react"
import { Outlet, useLocation } from "react-router"
import { Toaster } from "../ui"
import { Sidebar } from "./sidebar"
import { SidebarContext } from "./sidebar-state"

const key = "hx-sidebar"

const narrow = () => window.matchMedia("(max-width: 720px)").matches

function readOpen() {
  if (narrow()) return false
  try {
    return localStorage.getItem(key) !== "closed"
  } catch {
    return true
  }
}

function Layout() {
  const [open, setOpen] = useState(readOpen)
  const toggle = useCallback(() => {
    setOpen((value) => {
      try {
        localStorage.setItem(key, value ? "closed" : "open")
      } catch {
        // Storage can be blocked; the sidebar still toggles.
      }
      return !value
    })
  }, [])

  const { pathname } = useLocation()
  useEffect(() => {
    if (narrow()) setOpen(false)
  }, [pathname])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "b" && !event.altKey && !event.shiftKey) {
        event.preventDefault()
        toggle()
      }
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [toggle])

  return (
    <SidebarContext value={{ open, toggle }}>
      <div className="shell" data-sidebar={open ? "open" : "closed"}>
        <Sidebar />
        <main className="panel">
          <Outlet />
        </main>
      </div>
      <Toaster />
    </SidebarContext>
  )
}

export { Layout }
