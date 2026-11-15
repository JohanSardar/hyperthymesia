import type { ReactNode } from "react"
import { Empty } from "../ui"
import { navFor } from "./nav"
import { PageHeader } from "./page-header"

/** A page that exists in the rail and router but isn't built yet. */
function Placeholder({ path, title, description, action }: { path: string; title: ReactNode; description: ReactNode; action?: ReactNode }) {
  const item = navFor(path)
  const Icon = item?.icon
  return (
    <>
      <PageHeader title={item?.label} />
      <div className="page-scroll">
        <Empty fill icon={Icon && <Icon size={22} strokeWidth={1.5} />} title={title} description={description}>
          {action}
        </Empty>
      </div>
    </>
  )
}

export { Placeholder }
