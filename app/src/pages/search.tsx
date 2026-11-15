import { CalendarDays } from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"
import { Link, useSearchParams } from "react-router"
import { useCitations } from "../day/citations"
import { Highlight, kindInfo } from "../day/record"
import { clock, dayKey, dayName, timeRange, todayKey } from "../lib/day"
import { dayApi, type EntryHit, type FindResponse, type RecordHit } from "../server/day"
import { PageHeader, PageSidebar, SidebarGroup } from "../shell"
import { Badge, Callout, Empty, Kbd, Page, SearchInput, Segmented, Skeleton } from "../ui"
import "../day/day.css"

type Scope = "all" | "entries" | "records"

const recentKey = "hx-recent-searches"
function readRecent(): string[] {
  try {
    const value = JSON.parse(localStorage.getItem(recentKey) ?? "[]")
    return Array.isArray(value) ? value.filter((v) => typeof v === "string").slice(0, 8) : []
  } catch {
    return []
  }
}
function saveRecent(q: string) {
  try {
    const next = [q, ...readRecent().filter((v) => v !== q)].slice(0, 8)
    localStorage.setItem(recentKey, JSON.stringify(next))
    return next
  } catch {
    return readRecent()
  }
}

const entryWords: Record<EntryHit["kind"], string> = { task: "Task", episode: "Episode", summary: "Summary" }

function dayLink(iso: string) {
  const key = dayKey(new Date(iso))
  return key === todayKey() ? "/today" : `/today/${key}`
}

function EntryResult({ hit, terms }: { hit: EntryHit; terms: string[] }) {
  return (
    <Link to={dayLink(hit.start)} className="result" data-result="entry">
      <span className="result-when">
        <span>{dayName(dayKey(new Date(hit.start)), "short")}</span>
        <span>{hit.kind === "summary" ? timeRange(hit.start, hit.end) : clock(hit.start)}</span>
      </span>
      <span className="result-body">
        <span className="result-title">
          <Badge>{entryWords[hit.kind]}</Badge>
          <Highlight text={hit.title} terms={terms} />
        </span>
        {hit.description && hit.description !== hit.title && (
          <span className="result-snippet">
            <Highlight text={hit.description} terms={terms} />
          </span>
        )}
        {hit.goal && (
          <span className="result-meta">
            Goal: <Highlight text={hit.goal} terms={terms} />
          </span>
        )}
      </span>
    </Link>
  )
}

function RecordResult({ hit, terms, onOpen }: { hit: RecordHit; terms: string[]; onOpen: () => void }) {
  const info = kindInfo(hit.kind ?? "")
  const Icon = info.icon
  return (
    <button type="button" className="result" data-result="record" onClick={onOpen}>
      <span className="result-when">
        <span>{dayName(dayKey(new Date(hit.at)), "short")}</span>
        <span>{clock(hit.at)}</span>
      </span>
      <span className="result-body">
        <span className="result-title">
          <Icon size={13} strokeWidth={1.75} aria-hidden />
          <span>{info.label}</span>
          {(hit.app || hit.title) && <span className="result-where">{[hit.app, hit.title].filter(Boolean).join(" · ")}</span>}
        </span>
        <span className="result-snippet">
          <Highlight text={hit.snippet} terms={terms} />
        </span>
      </span>
    </button>
  )
}

function SearchPage() {
  const [params, setParams] = useSearchParams()
  const q = params.get("q") ?? ""
  const [text, setText] = useState(q)
  const [scope, setScope] = useState<Scope>("all")
  const [result, setResult] = useState<{ q: string; data?: FindResponse; error?: string }>()
  const [recent, setRecent] = useState(readRecent)
  const inputRef = useRef<HTMLInputElement>(null)
  const { open, sheet } = useCitations()

  useEffect(() => setText(q), [q])
  useEffect(() => inputRef.current?.focus(), [])

  // Type to search: the address follows the box after a short pause.
  useEffect(() => {
    const value = text.trim()
    if (value === q) return
    const timer = window.setTimeout(() => setParams(value ? { q: value } : {}, { replace: true }), 300)
    return () => window.clearTimeout(timer)
  }, [text, q, setParams])

  useEffect(() => {
    if (!q) {
      setResult(undefined)
      return
    }
    let alive = true
    setResult({ q })
    dayApi.find(q).then(
      (data) => {
        if (!alive) return
        setResult({ q, data })
        if (data.entries.length || data.records.length) setRecent(saveRecent(q))
      },
      (error: Error) => alive && setResult({ q, error: error.message }),
    )
    return () => {
      alive = false
    }
  }, [q])

  const terms = useMemo(
    () =>
      q
        .toLowerCase()
        .split(/\s+/)
        .map((t) => t.replace(/^"|"$/g, ""))
        .filter(Boolean),
    [q],
  )
  const data = result?.q === q ? result.data : undefined
  const entries = scope === "records" ? [] : (data?.entries ?? [])
  const records = scope === "entries" ? [] : (data?.records ?? [])

  return (
    <>
      <PageHeader title="Search" />
      <PageSidebar>
        <SidebarGroup label="Recent searches">
          {recent.length === 0 ? (
            <p className="sidebar-empty">Searches you run show up here</p>
          ) : (
            recent.map((r) => (
              <Link key={r} to={`/search?q=${encodeURIComponent(r)}`} className="sidebar-row" aria-current={r === q ? "page" : undefined}>
                <span className="sidebar-row-label">{r}</span>
              </Link>
            ))
          )}
        </SidebarGroup>
      </PageSidebar>
      <Page width="explore" className="search-page">
        <form
          className="search-box"
          role="search"
          onSubmit={(event) => {
            event.preventDefault()
            setParams(text.trim() ? { q: text.trim() } : {})
          }}
        >
          <SearchInput ref={inputRef} value={text} onChange={(e) => setText(e.currentTarget.value)} placeholder="Search your day pages and everything on screen" aria-label="Search" />
        </form>
        {q && (
          <div className="search-scope">
            <Segmented
              value={scope}
              onValueChange={(v) => setScope(v as Scope)}
              items={[
                { value: "all", label: "Everything" },
                { value: "entries", label: `Day pages${data ? ` ${data.entries.length}` : ""}` },
                { value: "records", label: `Screen text and records${data ? ` ${data.records.length}` : ""}` },
              ]}
            />
          </div>
        )}
        {!q ? (
          <Empty
            title="Find something you remember"
            description={
              <>
                Searches the AI's day pages and the full text of everything the Mac recorded on screen. <Kbd>⌘K</Kbd> searches from anywhere.
              </>
            }
          />
        ) : result?.error ? (
          <Callout tone="danger" title="Search failed">
            {result.error}
          </Callout>
        ) : !data ? (
          <div className="results">
            {Array.from({ length: 5 }, (_, i) => (
              <div key={i} className="result">
                <Skeleton width={64} />
                <Skeleton width={`${50 + ((i * 13) % 40)}%`} />
              </div>
            ))}
          </div>
        ) : !data.entries.length && !data.records.length ? (
          <Empty title={`Nothing matches “${q}”`} description="Try fewer or different words. Search matches whole words in recorded text." />
        ) : (
          <>
            {entries.length > 0 && (
              <section className="day-section">
                <h3 className="day-section-title">
                  In your day pages <span className="day-section-count">{entries.length}</span>
                </h3>
                <div className="results">
                  {entries.map((hit) => (
                    <EntryResult key={`${hit.revision_id}:${hit.kind}:${hit.id ?? ""}`} hit={hit} terms={terms} />
                  ))}
                </div>
              </section>
            )}
            {records.length > 0 && (
              <section className="day-section">
                <h3 className="day-section-title">
                  In what was recorded <span className="day-section-count">{records.length === 100 ? "newest 100" : records.length}</span>
                </h3>
                <div className="results">
                  {records.map((hit, i) => (
                    <RecordResult
                      key={hit.id}
                      hit={hit}
                      terms={terms}
                      onOpen={() =>
                        open({
                          title: `Results for “${q}”`,
                          description: "Matches are marked. Use ← and → to step through the results.",
                          sources: records.map((r) => ({ id: r.id })),
                          start: i,
                          terms,
                        })
                      }
                    />
                  ))}
                </div>
                {records.length === 100 && <p className="line-meta">Showing the newest 100 matches. Add a word to narrow it down.</p>}
              </section>
            )}
            {scope !== "records" && data.entries.length === 0 && data.records.length > 0 && (
              <p className="line-meta">
                <CalendarDays size={12} aria-hidden /> No day page mentions this; it was only on screen.
              </p>
            )}
          </>
        )}
      </Page>
      {sheet}
    </>
  )
}

export { SearchPage }
