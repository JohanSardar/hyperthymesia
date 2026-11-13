import { Field as SdkField, FieldDescription, FieldError, FieldLabel } from "@ui-sdk/react/field"
import { Input as SdkInput, InputGroup } from "@ui-sdk/react/input"
import {
  PasswordInput as SdkPasswordInput,
  PasswordInputControl,
  PasswordInputToggle,
} from "@ui-sdk/react/password-input"
import { Textarea as SdkTextarea } from "@ui-sdk/react/textarea"
import { Eye, EyeOff, Search } from "lucide-react"
import type { ComponentProps, ReactNode } from "react"
import { cx } from "./cx"

type FieldProps = Omit<ComponentProps<typeof SdkField>, "className" | "children"> & {
  label?: ReactNode
  description?: ReactNode
  error?: ReactNode
  className?: string
  children: ReactNode
}

function Field({ label, description, error, className, children, ...rest }: FieldProps) {
  return (
    <SdkField className={cx("field", className)} invalid={error ? true : undefined} {...rest}>
      {label && <FieldLabel className="field-label">{label}</FieldLabel>}
      {children}
      {description && <FieldDescription className="field-description">{description}</FieldDescription>}
      {error && <FieldError className="field-error">{error}</FieldError>}
    </SdkField>
  )
}

type InputProps = Omit<ComponentProps<typeof SdkInput>, "className"> & {
  className?: string
  mono?: boolean
}

function Input({ className, mono, ...rest }: InputProps) {
  return <SdkInput className={cx("input", mono && "mono", className)} {...rest} />
}

function SearchInput({ className, size = "md", ...rest }: Omit<InputProps, "size"> & { size?: "sm" | "md" }) {
  return (
    <InputGroup className={cx("input-search", className)} data-size={size}>
      <Search aria-hidden size={14} className="input-search-icon" />
      <SdkInput type="search" className="input" {...rest} />
    </InputGroup>
  )
}

function SecretInput({
  placeholder,
  className,
  ...rest
}: Omit<ComponentProps<typeof SdkPasswordInput>, "className"> & { placeholder?: string; className?: string }) {
  return (
    <SdkPasswordInput className={cx("input-secret", className)} {...rest}>
      <PasswordInputControl className="input mono" placeholder={placeholder} />
      <PasswordInputToggle className="input-secret-toggle" aria-label="Show">
        {(state) => (state.visible ? <EyeOff size={14} aria-hidden /> : <Eye size={14} aria-hidden />)}
      </PasswordInputToggle>
    </SdkPasswordInput>
  )
}

function Textarea({ className, ...rest }: Omit<ComponentProps<typeof SdkTextarea>, "className"> & { className?: string }) {
  return <SdkTextarea className={cx("textarea", className)} {...rest} />
}

export { Field, Input, SearchInput, SecretInput, Textarea }
