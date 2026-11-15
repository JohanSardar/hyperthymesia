import { RotateCcw } from "lucide-react"
import { useCallback, type ReactNode } from "react"
import type { Preferences } from "../server/api"
import { useSettings } from "../server/settings"
import { Confirm, ConfirmTrigger, Row, buttonAttrs, toast } from "../ui"
import { itemById } from "./catalog"

/**
 * One setting: its name and explanation come from the catalogue, so the page
 * and search always say the same thing. `children` go under the row.
 */
function Setting({ id, control, leading, description, onClick, children }: { id: string; control?: ReactNode; leading?: ReactNode; description?: ReactNode; onClick?: () => void; children?: ReactNode }) {
  const item = itemById.get(id)
  if (!item) throw new Error(`No setting ${id} in the catalogue`)
  return (
    <div className="setting" id={`setting-${id}`} data-setting={id}>
      <Row label={item.label} description={description ?? item.description} control={control} leading={leading} onClick={onClick} />
      {children && <div className="setting-body">{children}</div>}
    </div>
  )
}

/** Saves a change straight away and says so; puts it back if it fails. */
function useSave() {
  const { save } = useSettings()
  return useCallback(
    async (patch: Partial<Preferences>, done = "Saved") => {
      try {
        await save(patch)
        toast.success(done)
        return true
      } catch (cause) {
        toast.error("Couldn't save", { description: cause instanceof Error ? cause.message : undefined })
        return false
      }
    },
    [save],
  )
}

/** The page's Restore defaults button, with what it changes spelled out first. */
function RestoreDefaults({ changes, keeps, onRestore, disabled }: { changes: string[]; keeps?: string; onRestore: () => Promise<unknown>; disabled?: boolean }) {
  return (
    <Confirm
      title="Restore this page's defaults?"
      description={
        <>
          {changes.length ? (
            <>
              This sets: {changes.join("; ")}.
            </>
          ) : (
            "Everything here is already at its default."
          )}
          {keeps && <> {keeps}</>}
        </>
      }
      confirmLabel="Restore defaults"
      trigger={
        <ConfirmTrigger {...buttonAttrs("default", "sm", "pill")} disabled={disabled}>
          <RotateCcw size={13} aria-hidden /> Restore defaults
        </ConfirmTrigger>
      }
      onConfirm={async () => {
        // The save itself says "Defaults restored", or why it couldn't.
        try {
          await onRestore()
        } catch (cause) {
          toast.error("Couldn't restore defaults", { description: cause instanceof Error ? cause.message : undefined })
        }
      }}
    />
  )
}

function bytes(n: number | null | undefined) {
  if (n == null) return "unknown"
  const units = ["bytes", "KB", "MB", "GB", "TB"]
  let value = n
  let unit = 0
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000
    unit++
  }
  return `${unit ? value.toFixed(value < 10 ? 1 : 0) : value} ${units[unit]}`
}

function count(n: number, one: string, many = `${one}s`) {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`
}

function dateTime(iso: string | null | undefined) {
  if (!iso) return "—"
  return new Date(iso).toLocaleString(undefined, { month: "short", day: "numeric", year: "numeric", hour: "numeric", minute: "2-digit" })
}

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : "Something went wrong"
}

export { bytes, count, dateTime, errorText, RestoreDefaults, Setting, useSave }
