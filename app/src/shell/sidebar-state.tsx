import { createContext, useContext } from "react"

type SidebarState = { open: boolean; toggle: () => void }

const SidebarContext = createContext<SidebarState>({ open: true, toggle: () => {} })

function useSidebar() {
  return useContext(SidebarContext)
}

export { SidebarContext, useSidebar }
