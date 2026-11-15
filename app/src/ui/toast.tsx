import { Toast, ToastAction, ToastClose, ToastDescription, ToastRegion, ToastTitle, toast } from "@ui-sdk/react/toast"
import { X } from "lucide-react"
import { buttonAttrs } from "./button"

function Toaster() {
  return (
    <ToastRegion className="toast-region">
      {() => (
        <Toast className="toast">
          <span className="toast-mark" aria-hidden />
          <div className="toast-text">
            <ToastTitle className="toast-title" />
            <ToastDescription className="toast-description" />
          </div>
          <ToastAction {...buttonAttrs("default", "xs", "toast-action")} />
          <ToastClose className="toast-close" aria-label="Dismiss">
            <X size={12} aria-hidden />
          </ToastClose>
        </Toast>
      )}
    </ToastRegion>
  )
}

export { Toaster, toast }
