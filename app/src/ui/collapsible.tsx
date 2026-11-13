import { Collapsible as SdkCollapsible, CollapsiblePanel, CollapsibleTrigger } from "@ui-sdk/react/collapsible"
import { ChevronRight } from "lucide-react"
import type { ComponentProps, ReactNode } from "react"

function Disclosure({
  title,
  description,
  children,
  ...rest
}: Omit<ComponentProps<typeof SdkCollapsible>, "children" | "title"> & { title: ReactNode; description?: ReactNode; children: ReactNode }) {
  return (
    <SdkCollapsible className="disclosure" {...rest}>
      <CollapsibleTrigger className="disclosure-trigger">
        <span className="row-text">
          <span className="row-label">{title}</span>
          {description && <span className="row-description">{description}</span>}
        </span>
        <ChevronRight aria-hidden size={14} className="disclosure-chevron" />
      </CollapsibleTrigger>
      <CollapsiblePanel className="disclosure-panel">
        <div className="disclosure-inner">{children}</div>
      </CollapsiblePanel>
    </SdkCollapsible>
  )
}

export { Disclosure }
