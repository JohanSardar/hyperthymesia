import { Switch as SdkSwitch, SwitchThumb } from "@ui-sdk/react/switch"
import type { ComponentProps } from "react"
import { cx } from "./cx"

function Switch({ className, ...rest }: Omit<ComponentProps<typeof SdkSwitch>, "className" | "children"> & { className?: string }) {
  return (
    <SdkSwitch className={cx("switch", className)} {...rest}>
      <SwitchThumb className="switch-thumb" />
    </SdkSwitch>
  )
}

export { Switch }
