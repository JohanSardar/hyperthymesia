import { ChevronLeft, ChevronRight, ExternalLink } from "lucide-react"
import { useCallback, useEffect, useMemo, useState, type KeyboardEvent, type ReactNode } from "react"
import { Link } from "react-router"
import { clockSeconds, dayKey } from "../lib/day"
import { dayApi, type CiteResponse } from "../server/day"
import { Button, IconButton, Sheet, Skeleton } from "../ui"
import { kindInfo, RecordView } from "./record"

type Source = { id: number; revision?: number }
type Request = { title: ReactNode; description?: ReactNode; sources: Source[]; start?: number; terms?: string[] }

const cache = new Map<string, CiteResponse>()
const keyOf = (s: Source) => `${s.revision ?? 0}:${s.id}`

function useCite(source: Source | undefined) {
  const [state, setState] = useState<{ key: string; data?: CiteResponse; error?: string }>()
  useEffect(() => {
    if (!source) return
    const key = keyOf(source)
    const cached = cache.get(key)
    if (cached) {
      setState({ key, data: cached })
      return
    }
    let alive = true
    setState({ key })
    dayApi.cite(source.id, source.revision).then(
      (data) => {
        cache.set(key, data)
        if (alive) setState({ key, data })
      },
      (error: Error) => alive && setState({ key, error: error.message }),
    )
    return () => {
      alive = false
    }
  }, [source])
  return source && state?.key === keyOf(source) ? state : { key: "" }
}

/** Opens the records a line cites. Returns `open` and the sheet to render once on the page. */
function useCitations() {
  const [request, setRequest] = useState<Request | null>(null)
  const [index, setIndex] = useState(0)
  const open = useCallback((next: Request) => {
    setRequest(next)
    setIndex(next.start ?? 0)
  }, [])
  const sheet = (
    <CitationSheet
      request={request}
      index={index}
      setIndex={setIndex}
      onClose={() => setRequest(null)}
    />
  )
  return { open, sheet }
}

function CitationSheet({ request, index, setIndex, onClose }: { request: Request | null; index: number; setIndex: (n: number) => void; onClose: () => void }) {
  const sources = useMemo(() => request?.sources ?? [], [request])
  const current = sources[index]
  const state = useCite(current)
  const count = sources.length
  const go = (step: number) => count && setIndex((index + step + count) % count)
  const onKey = (event: KeyboardEvent) => {
    if ((event.target as HTMLElement).closest("input, textarea")) return
    if (event.key === "ArrowRight" || event.key === "j") go(1)
    else if (event.key === "ArrowLeft" || event.key === "k") go(-1)
    else return
    event.preventDefault()
  }
  const at = state.data ? (state.data.record.envelope.time.source_start ?? state.data.record.envelope.time.observed_at) : null
  return (
    <Sheet
      open={!!request}
      onOpenChange={(next) => !next && onClose()}
      title={request?.title ?? ""}
      description={request?.description}
      className="cite-sheet"
    >
      <div className="cite" onKeyDown={onKey}>
        {count > 1 && (
          <div className="cite-nav">
            <IconButton label="Previous record" shortcut="←" onClick={() => go(-1)}>
              <ChevronLeft size={15} aria-hidden />
            </IconButton>
            <span className="cite-count">
              Record {index + 1} of {count}
            </span>
            <IconButton label="Next record" shortcut="→" onClick={() => go(1)}>
              <ChevronRight size={15} aria-hidden />
            </IconButton>
            <CiteStrip sources={sources} index={index} setIndex={setIndex} />
          </div>
        )}
        {count === 0 ? (
          <p className="record-note">This line cites no stored records, so there is nothing to open.</p>
        ) : state.error ? (
          <p className="record-note" role="alert">
            Record {current?.id} couldn't be opened: {state.error}
          </p>
        ) : !state.data ? (
          <div className="record">
            <Skeleton width="40%" />
            <Skeleton width="90%" height={120} />
          </div>
        ) : (
          <RecordView
            data={state.data}
            terms={request?.terms}
            extra={
              at && (
                <div className="cite-actions">
                  <Button
                    size="sm"
                    variant="ghost"
                    render={<Link to={`/timeline/${dayKey(new Date(at))}?at=${encodeURIComponent(at)}`} onClick={onClose} />}
                  >
                    <ExternalLink size={13} aria-hidden /> See it on the timeline
                  </Button>
                </div>
              )
            }
          />
        )}
      </div>
    </Sheet>
  )
}

/** The cited records as small chips, so a person can jump to any of them. */
function CiteStrip({ sources, index, setIndex }: { sources: Source[]; index: number; setIndex: (n: number) => void }) {
  if (sources.length > 40) return null
  return (
    <div className="cite-strip" role="tablist" aria-label="Cited records">
      {sources.map((s, i) => {
        const cached = cache.get(keyOf(s))
        const info = cached ? kindInfo(cached.record.envelope.kind) : null
        const Icon = info?.icon
        return (
          <button
            key={keyOf(s)}
            type="button"
            role="tab"
            aria-selected={i === index}
            className="cite-chip"
            onClick={() => setIndex(i)}
            title={cached ? `${info?.label} · ${clockSeconds(cached.record.envelope.time.source_start ?? cached.record.envelope.time.observed_at)}` : `Record ${s.id}`}
          >
            {Icon ? <Icon size={11} aria-hidden /> : null}
            {i + 1}
          </button>
        )
      })}
    </div>
  )
}

export { useCitations }
export type { Source }
