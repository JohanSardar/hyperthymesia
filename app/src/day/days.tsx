import { Popover, PopoverContent, PopoverTitle, PopoverTrigger } from "@ui-sdk/react/popover"
import { CalendarDays, ChevronLeft, ChevronRight } from "lucide-react"
import { useMemo, useState } from "react"
import { NavLink, useNavigate } from "react-router"
import { dayName, dayRange, parseDay, shiftDay, todayKey } from "../lib/day"
import { dayApi, useLoad, type DaysResponse } from "../server/day"
import { PageSidebar, SidebarEmpty, SidebarGroup } from "../shell"
import { buttonAttrs, IconButton, Skeleton } from "../ui"

const daysCache = new Map<string, DaysResponse>()

function useDays() {
  return useLoad("days", dayApi.days, daysCache, 60_000)
}

type DayInfo = { records: number; screenText: number; written: boolean; pending: boolean }

/** What each local day holds: records, and whether the AI wrote any of it up. */
function useDayIndex() {
  const days = useDays()
  const index = useMemo(() => {
    const map = new Map<string, DayInfo>()
    if (!days.data) return map
    for (const d of days.data.days) map.set(d.day, { records: d.records, screenText: d.screen_text, written: false, pending: false })
    for (const [key, info] of map) {
      const { from, to } = dayRange(key)
      for (const w of days.data.windows) {
        if (Date.parse(w.start) < to.getTime() && Date.parse(w.end) > from.getTime()) {
          if (w.canonical_revision !== null) info.written = true
          else if (Date.parse(w.end) < Date.now()) info.pending = true
        }
      }
    }
    return map
  }, [days.data])
  return { index, loading: days.loading && !days.data, error: days.error }
}

type ListItem = { kind: "day"; key: string; info: DayInfo } | { kind: "gap"; from: string; to: string; count: number }

/** The days list in the sidebar. Runs of days with nothing recorded show as one muted line. */
function DaysSidebar({ current, base }: { current: string; base: "/today" | "/timeline" }) {
  const { index, loading, error } = useDayIndex()
  const items = useMemo(() => {
    const keys = [...index.keys()].sort().reverse()
    const today = todayKey()
    const out: ListItem[] = []
    const empty: DayInfo = { records: 0, screenText: 0, written: false, pending: false }
    // Today is always listed, recorded or not.
    if (!index.has(today)) out.push({ kind: "day", key: today, info: empty })
    let cursor = shiftDay(today, index.has(today) ? 0 : -1)
    const gap = (from: string, to: string) => {
      let count = 0
      for (let k = to; k >= from; k = shiftDay(k, -1)) count++
      if (count > 0) out.push({ kind: "gap", from, to, count })
    }
    for (const key of keys) {
      if (key > today) continue
      // Days after `key` up to `cursor` had nothing recorded.
      if (key < cursor) gap(shiftDay(key, 1), cursor)
      out.push({ kind: "day", key, info: index.get(key)! })
      cursor = shiftDay(key, -1)
    }
    return out
  }, [index])

  return (
    <PageSidebar>
      <SidebarGroup label="Days">
        {loading ? (
          Array.from({ length: 6 }, (_, i) => (
            <div key={i} className="sidebar-row">
              <Skeleton width={`${50 + ((i * 13) % 30)}%`} />
            </div>
          ))
        ) : error ? (
          <SidebarEmpty>Couldn't load your days</SidebarEmpty>
        ) : items.length === 0 ? (
          <SidebarEmpty>Nothing recorded yet</SidebarEmpty>
        ) : (
          items.map((item) =>
            item.kind === "day" ? (
              <NavLink key={item.key} to={item.key === todayKey() && base === "/today" ? "/today" : `${base}/${item.key}`} end className="sidebar-row day-row" aria-current={item.key === current ? "page" : undefined}>
                <span className="sidebar-row-label">{dayName(item.key, "short")}</span>
                <span className="sidebar-row-meta">
                  {item.info.records === 0 ? "nothing yet" : item.info.written ? (item.info.pending ? "partly written up" : "") : "not written up"}
                </span>
              </NavLink>
            ) : (
              <NavLink
                key={`gap-${item.from}`}
                to={`${base}/${item.to}`}
                className="sidebar-row day-gap"
                aria-current={current >= item.from && current <= item.to ? "page" : undefined}
              >
                <span className="sidebar-row-label">
                  {item.count === 1 ? dayName(item.to, "short") : `${rangeFormat.format(parseDay(item.from))} – ${rangeFormat.format(parseDay(item.to))}`}
                </span>
                <span className="sidebar-row-meta">nothing recorded</span>
              </NavLink>
            ),
          )
        )}
      </SidebarGroup>
    </PageSidebar>
  )
}

const rangeFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" })
const monthFormat = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" })
const weekdayFormat = new Intl.DateTimeFormat(undefined, { weekday: "narrow" })

/** Back, forward, and a month calendar that marks the days with records. */
function DayPicker({ current, base }: { current: string; base: "/today" | "/timeline" }) {
  const navigate = useNavigate()
  const { index } = useDayIndex()
  const [open, setOpen] = useState(false)
  const [month, setMonth] = useState(() => parseDay(current))
  const today = todayKey()
  const go = (key: string) => {
    setOpen(false)
    navigate(key === today && base === "/today" ? "/today" : `${base}/${key}`)
  }
  const first = new Date(month.getFullYear(), month.getMonth(), 1)
  // Weeks start on Monday.
  const lead = (first.getDay() + 6) % 7
  const daysInMonth = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate()
  const cells: (string | null)[] = [...Array.from({ length: lead }, () => null)]
  for (let d = 1; d <= daysInMonth; d++) {
    const date = new Date(month.getFullYear(), month.getMonth(), d)
    cells.push(`${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(d).padStart(2, "0")}`)
  }
  const weekdays = Array.from({ length: 7 }, (_, i) => weekdayFormat.format(new Date(2024, 0, 1 + i)))
  return (
    <div className="day-picker">
      <IconButton label="Previous day" onClick={() => go(shiftDay(current, -1))}>
        <ChevronLeft size={15} aria-hidden />
      </IconButton>
      <Popover
        open={open}
        onOpenChange={(next) => {
          setOpen(next)
          if (next) setMonth(parseDay(current))
        }}
      >
        <PopoverTrigger {...buttonAttrs("ghost", "sm", "day-picker-trigger")}>
          <CalendarDays size={14} aria-hidden />
          {dayName(current, "short")}
        </PopoverTrigger>
        <PopoverContent className="calendar" sideOffset={6} align="end">
          <PopoverTitle className="sr-only">Pick a day</PopoverTitle>
          <div className="calendar-head">
            <IconButton label="Previous month" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))}>
              <ChevronLeft size={14} aria-hidden />
            </IconButton>
            <span className="calendar-month">{monthFormat.format(month)}</span>
            <IconButton
              label="Next month"
              disabled={new Date(month.getFullYear(), month.getMonth() + 1, 1) > new Date()}
              onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))}
            >
              <ChevronRight size={14} aria-hidden />
            </IconButton>
          </div>
          <div className="calendar-grid" role="grid">
            {weekdays.map((w, i) => (
              <span key={i} className="calendar-weekday" aria-hidden>
                {w}
              </span>
            ))}
            {cells.map((key, i) =>
              key ? (
                <button
                  key={key}
                  type="button"
                  className="calendar-day"
                  data-has={index.has(key) ? "" : undefined}
                  data-current={key === current ? "" : undefined}
                  data-today={key === today ? "" : undefined}
                  disabled={key > today}
                  onClick={() => go(key)}
                  aria-label={`${dayName(key)}${index.has(key) ? "" : ", nothing recorded"}`}
                >
                  {Number(key.slice(8))}
                </button>
              ) : (
                <span key={`lead-${i}`} />
              ),
            )}
          </div>
          <p className="calendar-foot">
            <span className="calendar-dot" aria-hidden /> Days the Mac recorded
          </p>
        </PopoverContent>
      </Popover>
      <IconButton label="Next day" disabled={current >= today} onClick={() => go(shiftDay(current, 1))}>
        <ChevronRight size={15} aria-hidden />
      </IconButton>
    </div>
  )
}

export { DayPicker, DaysSidebar, useDayIndex }
