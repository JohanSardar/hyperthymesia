import type { ReactNode } from "react"

function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>
}

export { Kbd }
