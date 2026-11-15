import { CalendarDays, ChartNoAxesGantt, ChevronLeft, ChevronRight, ChevronDown, Quote, Radio, Search } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import { Link, useNavigate, useParams } from "react-router"
import { useCitations, type Source } from "../day/citations"
import { CoverageBar } from "../day/coverage"
import { DayPicker, DaysSidebar } from "../day/days"
import {
  account,
  clock,
  dayName,
  dayRange,
  duration,
  fullDayName,
  isDayKey,
  macRow,
  shiftDay,
  timeRange,
  todayKey,
  total,
  totals,
  type Cite,
  type DayEpisode,
  type DayGoal,
  type DayTask,
  type Ms,
  type WindowNote,
} from "../lib/day"
import { dayApi, dayCache, useLoad, type DayResponse } from "../server/day"
import { PageHeader, useRegisterCommands } from "../shell"
import { Badge, Button, Callout, Card, Disclosure, Empty, Page, Skeleton } from "../ui"
import "../day/day.css"

function useNow(ms: number) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), ms)
    return () => window.clearInterval(timer)
  }, [ms])
  return now
}

type Open = (request: { title: string; description?: string; sources: Source[] }) => void

/** The button every line carries: it opens the stored records the line was based on. */
function CiteButton({ cites, title, when, open }: { cites: Cite[]; title: string; when?: string; open: Open }) {
  if (!cites.length) {
    return (
      <span className="cite-btn" data-empty="" title="This line cites no stored records">
        no source
      </span>
    )
  }
  return (
    <button
      type="button"
      className="cite-btn"
      data-cite-count={cites.length}
      aria-label={`Open the ${cites.length === 1 ? "record" : `${cites.length} records`} behind “${title}”`}
      onClick={(event) => {
        event.stopPropagation()
        open({ title, description: [when, cites.length === 1 ? "1 cited record" : `${cites.length} cited records`].filter(Boolean).join(" · "), sources: cites })
      }}
    >
      <Quote size={10} strokeWidth={2.25} aria-hidden />
      {cites.length}
    </button>
  )
}

function spansText(spans: Ms[]) {
  if (!spans.length) return ""
  if (spans.length === 1) return timeRange(spans[0].start, spans[0].end)
  return `${clock(spans[0].start)} – ${clock(spans[spans.length - 1].end)}, in ${spans.length} parts`
}

function EpisodeRow({ episode, open }: { episode: DayEpisode; open: Open }) {
  const inferred = episode.claims?.some((c) => c.basis === "inferred")
  const reported = episode.claims?.some((c) => c.basis === "reported_by_source")
  const when = spansText(episode.spans)
  return (
    <li className="episode" data-line="episode">
      <span className="line-time">{clock(episode.start)}</span>
      <div className="line-text">
        <span className="episode-title">
          {episode.title}
          {inferred && <Badge>inferred</Badge>}
          {reported && <Badge>reported by a source</Badge>}
          {episode.confidence < 0.6 && <Badge tone="warning">unsure</Badge>}
        </span>
        {episode.description && episode.description !== episode.title && <span className="episode-description">{episode.description}</span>}
        <span className="line-meta">
          {when} · {duration(total(episode.spans))}
        </span>
      </div>
      <CiteButton cites={episode.evidence.map((id) => ({ id, revision: episode.revision }))} title={episode.title} when={when} open={open} />
    </li>
  )
}

function TaskRow({ task, open }: { task: DayTask; open: Open }) {
  const [expanded, setExpanded] = useState(false)
  const when = spansText(task.spans)
  return (
    <li className="task" data-line="task" data-open={expanded ? "" : undefined}>
      <div className="task-row">
        <span className="line-time">{clock(task.start)}</span>
        <button type="button" className="task-toggle" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
          <ChevronDown size={13} className="task-chevron" aria-hidden />
          <span className="task-title">{task.title}</span>
          {task.open && task.end >= Date.now() - 4 * 3600_000 && <Badge>still going</Badge>}
        </button>
        <span className="task-duration" title={when}>
          {duration(total(task.spans))}
        </span>
        <CiteButton cites={task.cites} title={task.title} when={when} open={open} />
      </div>
      {expanded && (
        <div className="task-detail">
          {task.description && task.description !== task.title && <p className="task-description">{task.description}</p>}
          <p className="line-meta">{when}</p>
          {task.episodes.length > 0 ? (
            <ol className="episodes">
              {task.episodes.map((e) => (
                <EpisodeRow key={e.key} episode={e} open={open} />
              ))}
            </ol>
          ) : (
            <p className="line-meta">The AI listed no episodes for this task in this day.</p>
          )}
        </div>
      )}
    </li>
  )
}

function GoalBlock({ goal, open }: { goal: DayGoal; open: Open }) {
  const title = goal.title ?? "No goal stated"
  return (
    <section className="goal" data-line="goal" data-none={goal.title ? undefined : ""}>
      <header className="goal-head">
        <h3 className="goal-title">{title}</h3>
        <span className="goal-meta">
          {duration(total(goal.spans))} · {goal.tasks.length === 1 ? "1 task" : `${goal.tasks.length} tasks`}
        </span>
        <CiteButton cites={goal.cites} title={title} open={open} />
      </header>
      <ol className="tasks">
        {goal.tasks.map((t) => (
          <TaskRow key={t.key} task={t} open={open} />
        ))}
      </ol>
    </section>
  )
}

function BriefText({ text }: { text: string }) {
  const [all, setAll] = useState(false)
  const long = text.length > 320
  return (
    <div className="brief-body">
      <p className="brief-text" data-clamped={long && !all ? "" : undefined}>
        {text}
      </p>
      {long && (
        <button type="button" className="brief-more" onClick={() => setAll(!all)}>
          {all ? "Show less" : "Show all"}
        </button>
      )}
    </div>
  )
}

function Brief({ notes, open }: { notes: WindowNote[]; open: Open }) {
  const written = notes.filter((n) => n.status === "written" && n.summary)
  if (!written.length) return null
  return (
    <section className="day-section">
      <h3 className="day-section-title">In brief</h3>
      <ol className="brief">
        {written.map((n) => (
          <li key={n.start} className="brief-line" data-line="summary">
            <span className="line-time">{timeRange(n.start, n.end)}</span>
            <BriefText text={n.summary ?? ""} />
            <CiteButton cites={n.cites} title={`In brief, ${timeRange(n.start, n.end)}`} when="Summaries cite no records themselves; these are every record the entries in this stretch cite" open={open} />
          </li>
        ))}
      </ol>
    </section>
  )
}

function NotWritten({ notes, dayKey }: { notes: WindowNote[]; dayKey: string }) {
  const pending = notes.filter((n) => n.status === "not-written")
  if (!pending.length) return null
  const list = pending.map((n) => timeRange(n.start, n.end)).join(", ")
  return (
    <Callout
      title={pending.length === 1 ? "Part of this day isn't written up yet" : "Parts of this day aren't written up yet"}
      actions={
        <Button size="sm" render={<Link to={`/timeline/${dayKey}?at=${encodeURIComponent(new Date(pending[0].start).toISOString())}`} />}>
          See what was recorded
        </Button>
      }
    >
      {list} {pending.length === 1 ? "was" : "were"} recorded, but the AI hasn't written an account of {pending.length === 1 ? "it" : "them"}. The timeline shows the raw record.
    </Callout>
  )
}

function Withheld({ notes }: { notes: WindowNote[] }) {
  const hidden = notes.filter((n) => n.status === "hidden")
  if (!hidden.length) return null
  return (
    <Callout title={hidden.length === 1 ? "One written-up stretch is withheld" : "Some written-up stretches are withheld"}>
      The AI wrote an account of {hidden.map((n) => timeRange(n.start, n.end)).join(", ")}, but it drew on data from connections this app isn't allowed to read, so the server
      withholds the whole account rather than show part of it. The timeline still shows everything the Mac recorded.
    </Callout>
  )
}

function HowMade({ day }: { day: DayResponse }) {
  if (!day.revisions.length) return null
  return (
    <Disclosure title="How this account was made" description="Each 4-hour stretch is written separately. Open one to see its model and the raw account.">
      <div className="revisions">
        {day.revisions.map((r) => {
          const i = r.interpretation
          return (
            <Disclosure
              key={r.revision_id}
              title={timeRange(i.window.start, i.window.end)}
              description={[
                r.model || i.provenance?.model,
                i.provenance?.prompt_version,
                `${i.tasks?.length ?? 0} tasks, ${i.episodes?.length ?? 0} episodes`,
                i.check_failures?.length ? `${i.check_failures.length} corrected by checks` : null,
              ]
                .filter(Boolean)
                .join(" · ")}
            >
              {r.has_agent_run && (
                <p className="line-meta">
                  <a href={`/revision/${r.revision_id}/run`} target="_blank" rel="noreferrer">
                    The AI's tool calls and exact results
                  </a>
                </p>
              )}
              {!!i.check_failures?.length && (
                <ul className="check-failures">
                  {i.check_failures.map((f, k) => (
                    <li key={k}>
                      <span className="mono">{f.item}</span>: {f.reason}
                    </li>
                  ))}
                </ul>
              )}
              <pre className="raw-json">{JSON.stringify(i, null, 2)}</pre>
            </Disclosure>
          )
        })}
      </div>
    </Disclosure>
  )
}

function Loading() {
  return (
    <div className="day-loading" aria-busy>
      <Skeleton width="100%" height={88} />
      {Array.from({ length: 4 }, (_, i) => (
        <div key={i} className="day-loading-row">
          <Skeleton width={56} />
          <Skeleton width={`${45 + ((i * 17) % 35)}%`} />
        </div>
      ))}
    </div>
  )
}

function TodayPage() {
  const params = useParams()
  const navigate = useNavigate()
  const key = isDayKey(params.day) ? params.day : todayKey()
  const isToday = key === todayKey()
  const now = useNow(60_000)
  const window = useMemo(() => {
    const { from, to } = dayRange(key)
    return { start: from.getTime(), end: to.getTime() }
  }, [key])
  const day = useLoad(`day:${key}`, () => dayApi.day(new Date(window.start), new Date(window.end)), dayCache, isToday ? 120_000 : 0)
  const data = day.data && day.data.window && Date.parse(day.data.window.start) === window.start ? day.data : undefined
  const acc = useMemo(() => (data ? account(data, window, now) : null), [data, window, now])
  const sums = useMemo(() => (data ? totals(macRow(data, window, now)) : null), [data, window, now])
  const gaps = useMemo(() => (data ? macRow(data, window, now).filter((s) => s.state === "gap").length : 0), [data, window, now])
  const { open, sheet } = useCitations()
  const go = (k: string) => navigate(k === todayKey() ? "/today" : `/today/${k}`)

  useRegisterCommands([
    { id: "day:prev", label: "Previous day", group: "Day", icon: ChevronLeft, run: () => go(shiftDay(key, -1)) },
    ...(isToday ? [] : [{ id: "day:next", label: "Next day", group: "Day", icon: ChevronRight, run: () => go(shiftDay(key, 1)) }]),
    ...(isToday ? [] : [{ id: "day:today", label: "Go to today", group: "Day", icon: CalendarDays, run: () => navigate("/today") }]),
    { id: "day:timeline", label: `Open the timeline for ${dayName(key).toLowerCase() === "today" ? "today" : dayName(key)}`, group: "Day", icon: ChartNoAxesGantt, run: () => navigate(`/timeline/${key}`) },
    ...(isToday ? [{ id: "day:live", label: "Open the live log", group: "Day", icon: Radio, keywords: ["arriving", "entries"], run: () => navigate("/today/live") }] : []),
  ])

  const hasRecords = !!data && (data.lanes.some((l) => l.spans.length) || data.points.some((p) => p.kind !== "policy.gap"))

  return (
    <>
      <PageHeader
        title={dayName(key)}
        actions={
          <>
            <DayPicker current={key} base="/today" />
            <Button size="sm" variant="ghost" render={<Link to={`/timeline/${key}`} />}>
              <ChartNoAxesGantt size={14} aria-hidden /> Timeline
            </Button>
            {isToday && (
              <Button size="sm" variant="ghost" render={<Link to="/today/live" />}>
                <Radio size={14} aria-hidden /> Live log
              </Button>
            )}
          </>
        }
      />
      <DaysSidebar current={key} base="/today" />
      <Page width="detail" className="day-page">
        <header className="day-intro">
          <h2 className="day-title">{fullDayName(key)}</h2>
          {sums && hasRecords && (
            <p className="day-subtitle">
              Recorded {duration(sums.recorded + sums.away)}
              {sums.away > 0 && <> (away for {duration(sums.away)} of it)</>}
              {sums.gap > 0 && (
                <>
                  {" "}
                  · not recorded for {duration(sums.gap)}
                  {gaps > 1 ? ` in ${gaps} stretches` : ""}
                </>
              )}
              {sums.paused > 0 && <> · paused for {duration(sums.paused)}</>}
            </p>
          )}
        </header>

        {day.error && !data ? (
          <Callout tone="danger" title="This day couldn't be loaded">
            {day.error.message}
          </Callout>
        ) : !data ? (
          <Loading />
        ) : (
          <>
            <Card className="day-coverage">
              <CoverageBar day={data} window={window} now={now} notes={acc?.notes} onPick={(at) => navigate(`/timeline/${key}?at=${encodeURIComponent(new Date(at).toISOString())}`)} />
            </Card>

            {!hasRecords ? (
              <Card>
                <Empty
                  title={isToday ? "Nothing recorded yet today" : "Nothing was recorded on this day"}
                  description={isToday ? "The Mac app records as you work. Entries show up here once they arrive." : "The Mac app sent nothing for this day, so there's no account of it."}
                />
              </Card>
            ) : (
              acc && (
                <>
                  <NotWritten notes={acc.notes} dayKey={key} />
                  <Withheld notes={acc.notes} />
                  <Brief notes={acc.notes} open={open} />
                  {acc.goals.length > 0 && (
                    <section className="day-section">
                      <h3 className="day-section-title">Goals and tasks</h3>
                      <div className="goals">
                        {acc.goals.map((g) => (
                          <GoalBlock key={g.key} goal={g} open={open} />
                        ))}
                      </div>
                    </section>
                  )}
                  {acc.loose.length > 0 && (
                    <section className="day-section">
                      <h3 className="day-section-title">Not part of a task</h3>
                      <ol className="episodes episodes-loose">
                        {acc.loose.map((e) => (
                          <EpisodeRow key={e.key} episode={e} open={open} />
                        ))}
                      </ol>
                    </section>
                  )}
                  {acc.goals.length === 0 && acc.loose.length === 0 && acc.notes.some((n) => n.status === "written") && (
                    <p className="line-meta">The AI found nothing to report in the parts of this day it wrote up.</p>
                  )}
                  <div className="day-foot">
                    <HowMade day={data} />
                    <Button size="sm" variant="ghost" render={<Link to={`/search`} />}>
                      <Search size={13} aria-hidden /> Search everything recorded
                    </Button>
                  </div>
                </>
              )
            )}
          </>
        )}
      </Page>
      {sheet}
    </>
  )
}

export { TodayPage }
