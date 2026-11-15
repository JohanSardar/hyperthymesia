import { ChevronRight } from "lucide-react"
import type { ReactNode } from "react"
import { cx } from "./cx"

function Card({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cx("card", className)}>{children}</div>
}

function CardHeader({ title, actions }: { title: ReactNode; actions?: ReactNode }) {
  return (
    <div className="card-header">
      <span className="row-label">{title}</span>
      {actions && <div className="row-control">{actions}</div>}
    </div>
  )
}

type RowProps = {
  label: ReactNode
  description?: ReactNode
  leading?: ReactNode
  control?: ReactNode
  href?: string
  onClick?: () => void
}

function Row({ label, description, leading, control, href, onClick }: RowProps) {
  const body = (
    <>
      {leading && <div className="row-leading">{leading}</div>}
      <div className="row-text">
        <span className="row-label">{label}</span>
        {description && <span className="row-description">{description}</span>}
      </div>
      {control && <div className="row-control">{control}</div>}
      {(href || onClick) && !control && <ChevronRight aria-hidden size={14} className="row-chevron" />}
    </>
  )
  if (href) return <a className="row" data-interactive="" href={href}>{body}</a>
  if (onClick) return <button type="button" className="row" data-interactive="" onClick={onClick}>{body}</button>
  return <div className="row">{body}</div>
}

type ListRowProps = {
  title: ReactNode
  description?: ReactNode
  leading?: ReactNode
  trailing?: ReactNode
  href?: string
  onClick?: () => void
  selected?: boolean
}

function ListRow({ title, description, leading, trailing, href, onClick, selected }: ListRowProps) {
  const body = (
    <>
      {leading && <div className="list-row-leading">{leading}</div>}
      <div className="list-row-text">
        <span className="row-label">{title}</span>
        {description && <span className="list-row-description">{description}</span>}
      </div>
      {trailing && <div className="list-row-trailing">{trailing}</div>}
    </>
  )
  const attrs = { className: "list-row", "data-selected": selected ? "" : undefined }
  if (href) return <a {...attrs} href={href}>{body}</a>
  if (onClick) return <button type="button" {...attrs} onClick={onClick}>{body}</button>
  return <div {...attrs}>{body}</div>
}

function Tile({ size = 40, children, label }: { size?: 24 | 36 | 40 | 44; children: ReactNode; label?: string }) {
  return (
    <span className="tile" data-size={size} role={label ? "img" : undefined} aria-label={label} aria-hidden={label ? undefined : true}>
      {children}
    </span>
  )
}

function Badge({ tone = "neutral", children }: { tone?: "neutral" | "primary" | "success" | "warning" | "danger"; children: ReactNode }) {
  return <span className="badge" data-tone={tone}>{children}</span>
}

function StatusDot({ tone }: { tone: "idle" | "live" | "ok" | "warning" | "error" }) {
  return <span className="status-dot" data-tone={tone} aria-hidden />
}

/** An empty state: what's missing, and the next step in one sentence. `fill` centres it in the page. */
function Empty({
  title,
  description,
  icon,
  fill,
  children,
}: {
  title: ReactNode
  description?: ReactNode
  icon?: ReactNode
  fill?: boolean
  children?: ReactNode
}) {
  return (
    <div className="empty" data-fill={fill ? "" : undefined}>
      {icon && (
        <span className="empty-icon" aria-hidden>
          {icon}
        </span>
      )}
      <p className="empty-title">{title}</p>
      {description && <p className="empty-description">{description}</p>}
      {children && <div className="empty-actions">{children}</div>}
    </div>
  )
}

function Skeleton({ width, height = 12 }: { width?: number | string; height?: number }) {
  return <span className="skeleton" style={{ width, height }} aria-hidden />
}

function Spinner({ size = 14 }: { size?: number }) {
  return <span className="spinner" style={{ width: size, height: size }} role="status" aria-label="Loading" />
}

function Stat({ label, value, hint }: { label: ReactNode; value: ReactNode; hint?: ReactNode }) {
  return (
    <div className="stat">
      <span className="stat-label">{label}</span>
      <span className="stat-value">{value}</span>
      {hint && <span className="stat-hint">{hint}</span>}
    </div>
  )
}

export { Badge, Card, CardHeader, Empty, ListRow, Row, Skeleton, Spinner, Stat, StatusDot, Tile }
