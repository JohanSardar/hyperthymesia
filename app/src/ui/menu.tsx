import {
  Menu,
  MenuContent as SdkMenuContent,
  MenuGroup,
  MenuGroupLabel as SdkMenuGroupLabel,
  MenuItem as SdkMenuItem,
  MenuSeparator as SdkMenuSeparator,
  MenuTrigger,
} from "@ui-sdk/react/menu"
import type { ComponentProps, ReactNode } from "react"
import { cx } from "./cx"

function MenuContent({
  className,
  sideOffset = 4,
  align = "start",
  ...rest
}: Omit<ComponentProps<typeof SdkMenuContent>, "className"> & { className?: string }) {
  return <SdkMenuContent className={cx("menu", className)} sideOffset={sideOffset} align={align} {...rest} />
}

function MenuItem({
  icon,
  className,
  children,
  ...rest
}: Omit<ComponentProps<typeof SdkMenuItem>, "className" | "children"> & {
  icon?: ReactNode
  className?: string
  children: ReactNode
}) {
  return (
    <SdkMenuItem className={cx("menu-item", className)} {...rest}>
      {icon && <span className="menu-item-icon">{icon}</span>}
      <span className="menu-item-label">{children}</span>
    </SdkMenuItem>
  )
}

function MenuSeparator() {
  return <SdkMenuSeparator className="menu-separator" />
}

function MenuGroupLabel({ children }: { children: ReactNode }) {
  return <SdkMenuGroupLabel className="menu-group-label">{children}</SdkMenuGroupLabel>
}

export { Menu, MenuContent, MenuGroup, MenuGroupLabel, MenuItem, MenuSeparator, MenuTrigger }
