import { Checkbox as SdkCheckbox, CheckboxIndicator } from "@ui-sdk/react/checkbox"
import { Radio as SdkRadio, RadioGroup as SdkRadioGroup, RadioIndicator } from "@ui-sdk/react/radio"
import {
  SegmentedControl as SdkSegmented,
  SegmentedControlIndicator,
  SegmentedControlItem,
} from "@ui-sdk/react/segmented-control"
import { Check, Minus } from "lucide-react"
import type { ComponentProps, ReactNode } from "react"
import { cx } from "./cx"

function Checkbox({ className, ...rest }: Omit<ComponentProps<typeof SdkCheckbox>, "className" | "children"> & { className?: string }) {
  return (
    <SdkCheckbox className={cx("checkbox", className)} {...rest}>
      <CheckboxIndicator className="checkbox-indicator">
        {(state) => (state.indeterminate ? <Minus size={11} strokeWidth={3} /> : <Check size={11} strokeWidth={3} />)}
      </CheckboxIndicator>
    </SdkCheckbox>
  )
}

function RadioGroup({ className, ...rest }: Omit<ComponentProps<typeof SdkRadioGroup>, "className"> & { className?: string }) {
  return <SdkRadioGroup className={cx("radio-group", className)} {...rest} />
}

function Radio({ className, ...rest }: Omit<ComponentProps<typeof SdkRadio>, "className" | "children"> & { className?: string }) {
  return (
    <SdkRadio className={cx("radio", className)} {...rest}>
      <RadioIndicator className="radio-indicator" />
    </SdkRadio>
  )
}

type ChoiceCardProps = {
  value: string
  title: ReactNode
  description?: ReactNode
  children?: ReactNode
}

function ChoiceCard({ value, title, description, children }: ChoiceCardProps) {
  return (
    <label className="choice-card">
      <span className="choice-card-text">
        <span className="row-label">{title}</span>
        {description && <span className="row-description">{description}</span>}
        {children}
      </span>
      <Radio value={value} />
    </label>
  )
}

type Option = { value: string; label: ReactNode }

function Segmented({
  items,
  className,
  ...rest
}: Omit<ComponentProps<typeof SdkSegmented>, "className" | "children" | "items"> & { items: Option[]; className?: string }) {
  return (
    <SdkSegmented className={cx("segmented", className)} {...rest}>
      <SegmentedControlIndicator className="segmented-indicator" />
      {items.map((item) => (
        <SegmentedControlItem key={item.value} value={item.value} className="segmented-item">
          {item.label}
        </SegmentedControlItem>
      ))}
    </SdkSegmented>
  )
}

export { Checkbox, ChoiceCard, Radio, RadioGroup, Segmented }
