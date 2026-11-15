import { Button as SdkButton } from "@ui-sdk/react/button"
import { Tooltip, TooltipContent, TooltipTrigger } from "@ui-sdk/react/tooltip"
import type { ComponentProps, ReactNode } from "react"
import { cx } from "./cx"
import { Kbd } from "./kbd"

type Variant = "default" | "primary" | "accent" | "ghost" | "soft" | "link" | "destructive"
type Size = "xs" | "sm" | "md" | "lg"

type ButtonProps = Omit<ComponentProps<typeof SdkButton>, "className"> & {
  variant?: Variant
  size?: Size
  className?: string
}

function buttonAttrs(variant: Variant, size: Size, className?: string, icon?: boolean) {
  return {
    className: cx("btn", className),
    "data-variant": variant,
    "data-size": size,
    "data-icon": icon ? "" : undefined,
  }
}

function Button({ variant = "default", size = "sm", className, ...rest }: ButtonProps) {
  return <SdkButton {...buttonAttrs(variant, size, className)} {...rest} />
}

type IconButtonProps = Omit<ButtonProps, "children" | "aria-label"> & {
  label: string
  shortcut?: string
  side?: "top" | "bottom" | "left" | "right"
  children: ReactNode
}

function IconButton({
  label,
  shortcut,
  side = "bottom",
  variant = "ghost",
  size = "sm",
  className,
  children,
  ...rest
}: IconButtonProps) {
  return (
    <Tooltip>
      <TooltipTrigger
        aria-label={label}
        render={<SdkButton {...buttonAttrs(variant, size, className, true)} {...rest} />}
      >
        {children}
      </TooltipTrigger>
      <TooltipContent className="tooltip" side={side} sideOffset={6}>
        {label}
        {shortcut && <Kbd>{shortcut}</Kbd>}
      </TooltipContent>
    </Tooltip>
  )
}

export { Button, IconButton, buttonAttrs }
export type { ButtonProps, Variant as ButtonVariant }
