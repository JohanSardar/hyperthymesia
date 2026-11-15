import { Check } from "lucide-react"
import type { ReactNode } from "react"
import { cx } from "./cx"

type Tone = "neutral" | "primary" | "warning" | "danger"

/** One plain-language notice in the flow of a page. */
function Callout({
  tone = "neutral",
  icon,
  title,
  children,
  actions,
}: {
  tone?: Tone
  icon?: ReactNode
  title?: ReactNode
  children?: ReactNode
  actions?: ReactNode
}) {
  return (
    <div className="callout" data-tone={tone} role={tone === "danger" ? "alert" : undefined}>
      {icon && (
        <span className="callout-icon" aria-hidden>
          {icon}
        </span>
      )}
      <div className="callout-text">
        {title && <span className="callout-title">{title}</span>}
        {children && <span className="callout-description">{children}</span>}
      </div>
      {actions && <div className="callout-actions">{actions}</div>}
    </div>
  )
}

type StepState = "done" | "current" | "upcoming"
type StepItem = { title: ReactNode; description?: ReactNode; state: StepState }

/** Numbered steps. A done step shows a check. */
function Steps({ items, className }: { items: StepItem[]; className?: string }) {
  return (
    <ol className={cx("steps", className)}>
      {items.map((item, index) => (
        <li key={index} className="steps-item" data-state={item.state} aria-current={item.state === "current" ? "step" : undefined}>
          <span className="steps-mark" aria-hidden>
            {item.state === "done" ? <Check size={12} strokeWidth={2.5} /> : index + 1}
          </span>
          <div className="steps-text">
            <span className="steps-title">{item.title}</span>
            {item.description && <span className="steps-description">{item.description}</span>}
          </div>
        </li>
      ))}
    </ol>
  )
}

/** A thin bar. Leave `value` out for work of unknown length. */
function Progress({
  value,
  label,
  hint,
  tone = "neutral",
}: {
  value?: number
  label?: ReactNode
  hint?: ReactNode
  tone?: Tone
}) {
  const known = value !== undefined
  const pct = known ? Math.min(100, Math.max(0, value)) : undefined
  return (
    <div className="progress" data-tone={tone} data-indeterminate={known ? undefined : ""}>
      {(label || hint) && (
        <div className="progress-head">
          {label && <span className="progress-label">{label}</span>}
          {hint && <span>{hint}</span>}
        </div>
      )}
      <div className="progress-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-label={typeof label === "string" ? label : undefined}>
        <div className="progress-fill" style={{ width: known ? `${pct}%` : undefined }} />
      </div>
    </div>
  )
}

/** A pill toggle for filters. */
function Chip({
  pressed,
  onPressedChange,
  count,
  disabled,
  children,
}: {
  pressed?: boolean
  onPressedChange?: (pressed: boolean) => void
  count?: number
  disabled?: boolean
  children: ReactNode
}) {
  return (
    <button
      type="button"
      className="chip"
      aria-pressed={pressed}
      data-pressed={pressed ? "" : undefined}
      data-disabled={disabled ? "" : undefined}
      disabled={disabled}
      onClick={() => onPressedChange?.(!pressed)}
    >
      {children}
      {count !== undefined && <span className="chip-count">{count}</span>}
    </button>
  )
}

function Chips({ children, label }: { children: ReactNode; label: string }) {
  return (
    <div className="chips" role="group" aria-label={label}>
      {children}
    </div>
  )
}

export { Callout, Chip, Chips, Progress, Steps }
export type { StepItem }
