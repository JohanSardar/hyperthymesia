import {
  Dialog as SdkDialog,
  DialogBackdrop,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "@ui-sdk/react/dialog"
import { X } from "lucide-react"
import type { ComponentProps, ReactNode } from "react"
import { buttonAttrs } from "./button"
import { cx } from "./cx"

type SurfaceProps = Omit<ComponentProps<typeof SdkDialog>, "children"> & {
  title: ReactNode
  description?: ReactNode
  trigger?: ReactNode
  footer?: ReactNode
  children?: ReactNode
  className?: string
  initialFocus?: ComponentProps<typeof DialogContent>["initialFocus"]
}

function Dialog({ title, description, trigger, footer, children, size = "md", className, initialFocus, ...rest }: SurfaceProps & { size?: "sm" | "md" | "lg" }) {
  return (
    <SdkDialog {...rest}>
      {trigger}
      <DialogBackdrop className="scrim" />
      <DialogContent className={cx("dialog", className)} data-size={size} initialFocus={initialFocus}>
        <header className="dialog-header">
          <div className="dialog-heading">
            <DialogTitle className="dialog-title">{title}</DialogTitle>
            {description && <DialogDescription className="dialog-description">{description}</DialogDescription>}
          </div>
          <DialogClose aria-label="Close" {...buttonAttrs("ghost", "sm", "dialog-close", true)}>
            <X size={14} aria-hidden />
          </DialogClose>
        </header>
        {children && <div className="dialog-body">{children}</div>}
        {footer && <footer className="dialog-footer">{footer}</footer>}
      </DialogContent>
    </SdkDialog>
  )
}

function Sheet({ title, description, trigger, footer, children, className, initialFocus, ...rest }: SurfaceProps) {
  return (
    <SdkDialog {...rest}>
      {trigger}
      <DialogBackdrop className="scrim" />
      <DialogContent className={cx("sheet", className)} initialFocus={initialFocus}>
        <header className="sheet-header">
          <div className="dialog-heading">
            <DialogTitle className="dialog-title">{title}</DialogTitle>
            {description && <DialogDescription className="dialog-description">{description}</DialogDescription>}
          </div>
          <DialogClose aria-label="Close" {...buttonAttrs("ghost", "sm", "dialog-close", true)}>
            <X size={14} aria-hidden />
          </DialogClose>
        </header>
        <div className="sheet-body">{children}</div>
        {footer && <footer className="sheet-footer">{footer}</footer>}
      </DialogContent>
    </SdkDialog>
  )
}

export { Dialog, DialogClose, DialogTrigger, Sheet }
