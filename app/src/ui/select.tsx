import {
  Select as SdkSelect,
  SelectContent,
  SelectItem,
  SelectItemIndicator,
  SelectTrigger,
  SelectValue,
} from "@ui-sdk/react/select"
import { Check, ChevronDown } from "lucide-react"
import type { ReactNode } from "react"
import { cx } from "./cx"

type Option = { value: string; label: ReactNode; description?: ReactNode }

type SelectProps = {
  items: Option[]
  value?: string | null
  defaultValue?: string
  onValueChange?: (value: string | null) => void
  placeholder?: string
  name?: string
  disabled?: boolean
  required?: boolean
  width?: 128 | 144 | 176 | 224 | "full"
  className?: string
  "aria-label"?: string
}

function Select({ items, width = 176, className, "aria-label": ariaLabel, onValueChange, ...rest }: SelectProps) {
  return (
    <SdkSelect<string> {...rest} onValueChange={onValueChange ? (value) => onValueChange(value as string | null) : undefined}>
      <SelectTrigger className={cx("select-trigger", className)} data-width={width} aria-label={ariaLabel}>
        <SelectValue className="select-value" />
        <ChevronDown aria-hidden size={12} className="select-chevron" />
      </SelectTrigger>
      <SelectContent className="menu select-content" sideOffset={4}>
        {items.map((item) => (
          <SelectItem key={item.value} value={item.value} label={typeof item.label === "string" ? item.label : undefined} className="menu-item">
            <span className="menu-item-label">
              {item.label}
              {item.description && <span className="menu-item-description">{item.description}</span>}
            </span>
            <SelectItemIndicator className="menu-item-check">
              <Check size={14} />
            </SelectItemIndicator>
          </SelectItem>
        ))}
      </SelectContent>
    </SdkSelect>
  )
}

export { Select }
export type { Option as SelectOption }
