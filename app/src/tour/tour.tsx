import { Bot, CalendarDays, GraduationCap, Plug, Settings, X, type LucideIcon } from "lucide-react"
import { useCallback, useEffect, useLayoutEffect, useState } from "react"
import { createPortal } from "react-dom"
import { useLocation, useNavigate } from "react-router"
import { useSettings } from "../server/settings"
import { useRegisterCommands } from "../shell/commands"
import { Button, IconButton } from "../ui"
import "./tour.css"

type Stop = { label: string; icon: LucideIcon; title: string; body: string }

// Points at the rail items by their labels; the rail is the one place every page shares.
const stops: Stop[] = [
  {
    label: "Today",
    icon: CalendarDays,
    title: "Your day, as it happens",
    body: "Every 4 hours your AI writes what you did here. Each line links to the exact record it came from. Past days are in the sidebar.",
  },
  {
    label: "Connections",
    icon: Plug,
    title: "Add more context",
    body: "Calendar, mail, notes and any API, MCP or command-line tool. Optional: screen text is enough on its own.",
  },
  {
    label: "Agent",
    icon: Bot,
    title: "Your AI and when it runs",
    body: "See every provider login and whether it works, change the model, Test it, and set a monthly spending limit.",
  },
  {
    label: "Settings",
    icon: Settings,
    title: "Recording, storage and access",
    body: "Pause recording, never record an app or site, and replay this tour any time.",
  },
]

/** Starts the tour again from Settings or ⌘K. */
function useReplayTour() {
  const { save } = useSettings()
  const navigate = useNavigate()
  return useCallback(async () => {
    await save({ setup: { step: "done", finished: true, tour_pending: true } })
    navigate("/today")
  }, [navigate, save])
}

/** The short guided tour after setup: four coach marks beside the rail. */
function Tour() {
  const { data, save } = useSettings()
  const location = useLocation()
  const replay = useReplayTour()
  const pending = Boolean(data?.settings.setup?.tour_pending)
  const [index, setIndex] = useState(0)
  const [anchor, setAnchor] = useState<DOMRect | null>(null)

  useRegisterCommands([{ id: "tour.replay", label: "Replay the tour", group: "Help", icon: GraduationCap, keywords: ["guide", "welcome", "help"], run: replay }])

  useEffect(() => {
    if (pending) setIndex(0)
  }, [pending])

  const stop = stops[index]
  const show = pending && location.pathname.startsWith("/today")
  useLayoutEffect(() => {
    if (!show) return
    const measure = () => {
      const target = document.querySelector<HTMLElement>(`.rail [aria-label="${stop.label}"]`)
      setAnchor(target?.getBoundingClientRect() ?? null)
    }
    measure()
    window.addEventListener("resize", measure)
    return () => window.removeEventListener("resize", measure)
  }, [show, stop.label])

  const close = useCallback(() => {
    void save({ setup: { step: "done", finished: true, tour_pending: false } }).catch(() => {})
  }, [save])

  useEffect(() => {
    if (!show) return
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") close()
      if (event.key === "ArrowRight") setIndex((i) => Math.min(i + 1, stops.length - 1))
      if (event.key === "ArrowLeft") setIndex((i) => Math.max(i - 1, 0))
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [show, close])

  if (!show) return null
  const Icon = stop.icon
  const last = index === stops.length - 1
  const top = anchor ? Math.max(12, Math.min(anchor.top - 20, window.innerHeight - 260)) : 80
  return createPortal(
    <div className="tour-layer" role="presentation">
      {anchor && <span className="tour-ring" style={{ top: anchor.top - 4, left: anchor.left - 4, width: anchor.width + 8, height: anchor.height + 8 }} aria-hidden />}
      <div key={index} className="tour-card" role="dialog" aria-modal="false" aria-labelledby="tour-title" style={{ top, left: (anchor?.right ?? 52) + 14 }}>
        <div className="tour-art" aria-hidden>
          <span className="tour-art-icon">
            <Icon size={20} strokeWidth={1.6} />
          </span>
        </div>
        <IconButton label="Close the tour" size="xs" className="tour-close" onClick={close}>
          <X size={13} aria-hidden />
        </IconButton>
        <span className="tour-step">
          {index + 1} of {stops.length} · {stop.label}
        </span>
        <h2 id="tour-title" className="tour-title">
          {stop.title}
        </h2>
        <p className="tour-body">{stop.body}</p>
        <div className="tour-foot">
          <span className="tour-dots" aria-hidden>
            {stops.map((s, i) => (
              <span key={s.label} data-active={i === index ? "" : undefined} />
            ))}
          </span>
          <span className="tour-actions">
            {index > 0 && (
              <Button variant="ghost" size="sm" onClick={() => setIndex(index - 1)}>
                Back
              </Button>
            )}
            <Button variant="primary" size="sm" autoFocus onClick={() => (last ? close() : setIndex(index + 1))}>
              {last ? "Done" : "Next"}
            </Button>
          </span>
        </div>
      </div>
    </div>,
    document.body,
  )
}

export { Tour, useReplayTour }
