import { Check, Monitor, Moon, Sun, X } from "lucide-react"
import { useState, type ReactNode } from "react"
import type { Density, Notifications } from "../server/api"
import { useSettings } from "../server/settings"
import { bindable, comboOf, formatCombo, useBindings } from "../shell/keys"
import { useAppearance } from "../shell/appearance"
import { Button, Card, Kbd, Segmented, Select, Switch, toast } from "../ui"
import { RestoreDefaults, Setting, useSave } from "./parts"

const themes = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
] as const

function AppearancePage({ actionsSlot }: { actionsSlot: (node: ReactNode) => ReactNode }) {
  const { data } = useSettings()
  const save = useSave()
  const { theme, setTheme } = useAppearance()
  const density = data!.settings.density ?? "comfortable"
  const size = data!.settings.font_size ?? 14
  const changes = [theme !== "dark" && "dark theme", density !== "comfortable" && "comfortable density", size !== 14 && "14 px text"].filter(Boolean) as string[]
  return (
    <>
      {actionsSlot(
        <RestoreDefaults
          changes={changes}
          onRestore={async () => {
            setTheme("dark")
            await save({ appearance: "dark", density: "comfortable", font_size: 14 }, "Defaults restored")
          }}
        />,
      )}
      <Card>
        <Setting id="appearance.theme">
          <div className="theme-tiles" role="radiogroup" aria-label="Theme">
            {themes.map((t) => (
              <button key={t.value} type="button" role="radio" aria-checked={theme === t.value} className="theme-tile" data-selected={theme === t.value ? "" : undefined} onClick={() => setTheme(t.value)}>
                {t.value === "system" ? (
                  <span className="theme-preview-half" aria-hidden>
                    <span className="theme-preview" data-theme="light">
                      <span />
                      <span />
                    </span>
                    <span className="theme-preview" data-theme="dark">
                      <span />
                      <span />
                    </span>
                  </span>
                ) : (
                  <span className="theme-preview" data-theme={t.value} aria-hidden>
                    <span />
                    <span />
                    <span />
                  </span>
                )}
                <span className="theme-label">
                  <t.icon size={13} aria-hidden /> {t.label}
                </span>
              </button>
            ))}
          </div>
        </Setting>
        <Setting
          id="appearance.density"
          control={
            <Segmented
              aria-label="Density"
              value={density}
              onValueChange={(v) => v && save({ density: v as Density })}
              items={[
                { value: "compact", label: "Compact" },
                { value: "comfortable", label: "Comfortable" },
                { value: "spacious", label: "Spacious" },
              ]}
            />
          }
        />
        <Setting
          id="appearance.font-size"
          control={
            <Select
              aria-label="Text size"
              width={144}
              value={String(size)}
              onValueChange={(v) => v && save({ font_size: Number(v) })}
              items={[12, 13, 14, 15, 16, 17, 18].map((n) => ({ value: String(n), label: n === 14 ? "14 px (default)" : `${n} px` }))}
            />
          }
        />
      </Card>
    </>
  )
}

const noticeIds = ["day_ready", "failed_run", "failed_pull", "permission_lost"] as const
const allOn: Notifications = { day_ready: true, failed_run: true, failed_pull: true, permission_lost: true }

function NotificationsPage({ actionsSlot }: { actionsSlot: (node: ReactNode) => ReactNode }) {
  const { data } = useSettings()
  const save = useSave()
  const notify = { ...allOn, ...data!.settings.notifications }
  const off = noticeIds.filter((id) => !notify[id])
  return (
    <>
      {actionsSlot(<RestoreDefaults changes={off.length ? ["every notification on"] : []} onRestore={() => save({ notifications: allOn }, "Defaults restored")} />)}
      <Card>
        {noticeIds.map((id) => (
          <Setting
            key={id}
            id={`notifications.${id}`}
            control={<Switch aria-label={id} checked={notify[id]} onCheckedChange={(on) => save({ notifications: { ...notify, [id]: on } }, on ? "Notification on" : "Notification off")} />}
          />
        ))}
      </Card>
      <p className="settings-note">The Mac app shows these as macOS notifications. macOS can also silence them in System Settings › Notifications.</p>
    </>
  )
}

function ShortcutRow({ id, combo, taken, onSet, onReset, custom }: { id: string; combo: string; taken: (combo: string) => string | null; onSet: (combo: string) => void; onReset: () => void; custom: boolean }) {
  const [recording, setRecording] = useState(false)
  return (
    <Setting
      id={`shortcuts.${id}`}
      control={
        <span className="shortcut-control">
          {recording ? (
            <button
              type="button"
              className="shortcut-recorder"
              data-recording-keys=""
              autoFocus
              onBlur={() => setRecording(false)}
              onKeyDown={(e) => {
                e.preventDefault()
                e.stopPropagation()
                if (e.key === "Escape") return setRecording(false)
                const next = comboOf(e.nativeEvent)
                if (!next) return
                const clash = taken(next)
                if (clash) {
                  toast.error(`${formatCombo(next)} is already used`, { description: `By “${clash}”. Change that one first.` })
                  return
                }
                onSet(next)
                setRecording(false)
              }}
            >
              Press keys… <span className="row-description">Esc to cancel</span>
            </button>
          ) : (
            <button type="button" className="shortcut-chip" onClick={() => setRecording(true)} aria-label={`Change shortcut, now ${formatCombo(combo)}`}>
              <Kbd>{formatCombo(combo)}</Kbd>
            </button>
          )}
          {custom && !recording && (
            <Button variant="ghost" size="xs" aria-label="Reset to default" onClick={onReset}>
              <X size={12} aria-hidden />
            </Button>
          )}
        </span>
      }
    />
  )
}

function ShortcutsPage({ actionsSlot }: { actionsSlot: (node: ReactNode) => ReactNode }) {
  const { data } = useSettings()
  const save = useSave()
  const keys = useBindings()
  const custom = data!.settings.shortcuts ?? {}
  const groups = [...new Set(bindable.map((b) => b.group))]
  const taken = (id: string) => (combo: string) => {
    const other = bindable.find((b) => b.id !== id && keys[b.id] === combo)
    return other ? other.label : null
  }
  return (
    <>
      {actionsSlot(<RestoreDefaults changes={Object.keys(custom).length ? ["every shortcut back to its default"] : []} onRestore={() => save({ shortcuts: {} }, "Defaults restored")} />)}
      {groups.map((group) => (
        <Card key={group}>
          <div className="card-header">
            <span className="row-label">{group}</span>
          </div>
          {bindable
            .filter((b) => b.group === group)
            .map((b) => (
              <ShortcutRow
                key={b.id}
                id={b.id}
                combo={keys[b.id]}
                custom={b.id in custom}
                taken={taken(b.id)}
                onSet={(combo) => {
                  const next = { ...custom }
                  if (combo === b.combo) delete next[b.id]
                  else next[b.id] = combo
                  void save({ shortcuts: next }, `${b.label}: ${formatCombo(combo)}`)
                }}
                onReset={() => {
                  const { [b.id]: _gone, ...rest } = custom
                  void save({ shortcuts: rest }, "Back to the default")
                }}
              />
            ))}
        </Card>
      ))}
      <p className="settings-note">
        <Check size={12} aria-hidden /> In the citation sheet, ← and → (or j and k) step through records. Those keys belong to the sheet and can't be changed.
      </p>
    </>
  )
}

export { AppearancePage, NotificationsPage, ShortcutsPage }
