import { Toast, ToastClose, ToastDescription, ToastRegion, ToastTitle, toast } from "@ui-sdk/react/toast"
import { X } from "lucide-react"

function Toaster() {
  return (
    <ToastRegion className="toast-region">
      {() => (
        <Toast className="toast">
          <div className="toast-text">
            <ToastTitle className="toast-title" />
            <ToastDescription className="toast-description" />
          </div>
          <ToastClose className="toast-close" aria-label="Dismiss">
            <X size={12} aria-hidden />
          </ToastClose>
        </Toast>
      )}
    </ToastRegion>
  )
}

export { Toaster, toast }
