import { createContext, useContext } from "react"

type SidebarState = {
  open: boolean
  toggle: () => void
  width: number
  setWidth: (width: number) => void
  resetWidth: () => void
  /** Where a page's own sidebar content is rendered (see PageSidebar). */
  slot: HTMLElement | null
  setSlot: (node: HTMLElement | null) => void
  /** True while a page has put content in the slot. */
  filled: boolean
  setFilled: (filled: boolean) => void
}

const SidebarContext = createContext<SidebarState>({
  open: true,
  toggle: () => {},
  width: 248,
  setWidth: () => {},
  resetWidth: () => {},
  slot: null,
  setSlot: () => {},
  filled: false,
  setFilled: () => {},
})

function useSidebar() {
  return useContext(SidebarContext)
}

export { SidebarContext, useSidebar }
