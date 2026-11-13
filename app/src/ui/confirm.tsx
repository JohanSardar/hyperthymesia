import {
  AlertDialog,
  AlertDialogBackdrop,
  AlertDialogClose,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@ui-sdk/react/alert-dialog"
import type { ComponentProps, ReactNode } from "react"
import { buttonAttrs } from "./button"

type ConfirmProps = Omit<ComponentProps<typeof AlertDialog>, "children"> & {
  title: ReactNode
  description?: ReactNode
  confirmLabel: string
  cancelLabel?: string
  destructive?: boolean
  trigger?: ReactNode
  onConfirm: () => unknown
}

function Confirm({
  title,
  description,
  confirmLabel,
  cancelLabel = "Cancel",
  destructive,
  trigger,
  onConfirm,
  ...rest
}: ConfirmProps) {
  return (
    <AlertDialog {...rest}>
      {trigger}
      <AlertDialogBackdrop className="scrim" />
      <AlertDialogContent className="dialog" data-size="sm">
        <header className="dialog-header">
          <div className="dialog-heading">
            <AlertDialogTitle className="dialog-title">{title}</AlertDialogTitle>
            {description && <AlertDialogDescription className="dialog-description">{description}</AlertDialogDescription>}
          </div>
        </header>
        <footer className="dialog-footer">
          <AlertDialogClose {...buttonAttrs("default", "sm")}>{cancelLabel}</AlertDialogClose>
          <AlertDialogClose
            value="confirm"
            onClick={() => onConfirm()}
            {...buttonAttrs(destructive ? "destructive" : "primary", "sm")}
          >
            {confirmLabel}
          </AlertDialogClose>
        </footer>
      </AlertDialogContent>
    </AlertDialog>
  )
}

export { AlertDialogTrigger as ConfirmTrigger, Confirm }
