import type { ReactNode } from "react"
import { cx } from "./cx"

type Width = "settings" | "explore" | "detail" | "dashboard" | "full"

function Page({ width = "settings", className, children }: { width?: Width; className?: string; children: ReactNode }) {
  return (
    <div className="page-scroll">
      <div className={cx("page", className)} data-width={width}>
        {children}
      </div>
    </div>
  )
}

function PageIntro({ title, description, actions }: { title: ReactNode; description?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="page-intro">
      <div>
        <h2 className="h1">{title}</h2>
        {description && <p className="page-intro-description">{description}</p>}
      </div>
      {actions && <div className="page-intro-actions">{actions}</div>}
    </div>
  )
}

function Section({ title, description, actions, children }: { title?: ReactNode; description?: ReactNode; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="section">
      {(title || actions) && (
        <div className="section-head">
          <div>
            {title && <h3 className="h2">{title}</h3>}
            {description && <p className="section-description">{description}</p>}
          </div>
          {actions && <div className="section-actions">{actions}</div>}
        </div>
      )}
      {children}
    </section>
  )
}

export { Page, PageIntro, Section }
