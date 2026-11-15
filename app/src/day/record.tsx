import { AppWindow, Camera, FileText, Keyboard, Moon, MousePointerClick, PauseCircle, Plug, type LucideIcon } from "lucide-react"
import { Fragment, type ReactNode } from "react"
import { clockSeconds, duration, timeRange } from "../lib/day"
import type { CiteResponse, Seen, StoredEnvelope, StoredRecord } from "../server/day"
import { Badge, Disclosure } from "../ui"

const str = (v: unknown) => (typeof v === "string" && v.trim() ? v.trim() : undefined)

type KindInfo = { icon: LucideIcon; label: string }
function kindInfo(kind: string): KindInfo {
  switch (kind) {
    case "text.snapshot":
      return { icon: FileText, label: "Screen text" }
    case "focus.span":
      return { icon: AppWindow, label: "Window in front" }
    case "ui.click":
      return { icon: MousePointerClick, label: "Click" }
    case "media.still":
    case "ocr.text":
      return { icon: Camera, label: "Text read from a screenshot" }
    case "presence.span":
      return { icon: Moon, label: "At the Mac or away" }
    case "input.bucket":
      return { icon: Keyboard, label: "Input counts" }
    case "policy.gap":
      return { icon: PauseCircle, label: "Recording gap" }
    default:
      return { icon: Plug, label: kind }
  }
}

/** Marks every case-insensitive occurrence of the search terms. */
function Highlight({ text, terms }: { text: string; terms?: string[] }) {
  const list = (terms ?? []).filter((t) => t.length > 0)
  if (!list.length) return <>{text}</>
  const escaped = list.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
  const parts = text.split(new RegExp(`(${escaped.join("|")})`, "gi"))
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <mark key={i} className="hit">
            {part}
          </mark>
        ) : (
          <Fragment key={i}>{part}</Fragment>
        ),
      )}
    </>
  )
}

const ocrHeader = "\n\nScreen text (top to bottom):\n"
const axHeader = "\nAccessibility text (may include offscreen content):\n"

/** Splits the recorder's text into what it read off the screen and what accessibility reported. */
function textSections(raw: string): { label: string | null; body: string }[] {
  // Runs of blank lines are folded to one; the words and their order are untouched.
  const text = raw.replace(/\n[ \t]*(\n[ \t]*){2,}/g, "\n\n")
  const ocrAt = text.indexOf(ocrHeader)
  if (ocrAt < 0) return [{ label: null, body: text }]
  const rest = text.slice(ocrAt + ocrHeader.length)
  const axAt = rest.indexOf(axHeader)
  const ocr = axAt < 0 ? rest : rest.slice(0, axAt)
  const ax = axAt < 0 ? "" : rest.slice(axAt + axHeader.length)
  return [
    { label: "Read off the screen", body: ocr.trim() },
    ...(ax.trim() ? [{ label: "Accessibility text (can include parts that were scrolled out of view)", body: ax.trim() }] : []),
  ]
}

function ScreenText({ envelope, terms }: { envelope: StoredEnvelope; terms?: string[] }) {
  const p = envelope.payload
  const text = str(p.text) ?? ""
  const truncated = p.completeness === "truncated"
  return (
    <div className="screen-text">
      {textSections(text).map((section, i) => (
        <div key={i} className="screen-text-section">
          {section.label && <div className="screen-text-label">{section.label}</div>}
          <pre className="screen-text-body">
            {section.body ? <Highlight text={section.body} terms={terms} /> : <span className="muted">Nothing</span>}
          </pre>
        </div>
      ))}
      {truncated && <p className="record-note">The recorder kept only the first part of this text.</p>}
    </div>
  )
}

function when(envelope: StoredEnvelope) {
  const start = envelope.time.source_start ?? envelope.time.observed_at
  const end = envelope.time.source_end
  if (!end || end === start) return clockSeconds(start)
  const ms = Date.parse(end) - Date.parse(start)
  const range = ms < 10 * 60_000 ? `${clockSeconds(start)} – ${clockSeconds(end)}` : timeRange(start, end)
  return `${range} · ${duration(ms)}`
}

function RecordHead({ envelope, id }: { envelope: StoredEnvelope; id: number }) {
  const info = kindInfo(envelope.kind)
  const Icon = info.icon
  const p = envelope.payload
  const app = str(p.app_name) ?? str(p.surface) ?? str(p.app_bundle_id)
  const title = str(p.window_title)
  return (
    <div className="record-head">
      <span className="record-kind">
        <Icon size={14} strokeWidth={1.75} aria-hidden />
        {info.label}
      </span>
      <span className="record-when">{when(envelope)}</span>
      {(app || title) && (
        <span className="record-where">
          {app}
          {app && title && " · "}
          {title}
        </span>
      )}
      <span className="record-id">Record {id}</span>
    </div>
  )
}

function Seen({ seen }: { seen: Seen | null }) {
  if (!seen) return null
  if (seen.unavailable) {
    return <p className="record-note">The AI's run for this part of the day isn't available, so what it read can't be shown.</p>
  }
  const snapshot = seen.snapshot ? pretty(seen.snapshot) : null
  if (!seen.digest_lines.length && !snapshot) {
    return seen.has_digest ? (
      <p className="record-note">The AI's input didn't list this record on a line of its own. It may sit inside a grouped line, which lists only the first and last ids.</p>
    ) : null
  }
  return (
    <section className="record-section">
      <h4 className="record-section-title">What the AI read</h4>
      <p className="record-section-note">Exactly as stored with the AI's run for this part of the day.</p>
      {seen.digest_lines.length > 0 && <pre className="seen-lines">{seen.digest_lines.join("\n")}</pre>}
      {snapshot && <pre className="seen-lines">{snapshot}</pre>}
    </section>
  )
}

function pretty(text: string) {
  try {
    return JSON.stringify(JSON.parse(text), null, 2)
  } catch {
    return text
  }
}

function SpanText({ data, terms }: { data: CiteResponse; terms?: string[] }) {
  const list = data.screen_text
  return (
    <section className="record-section">
      <h4 className="record-section-title">Screen text recorded during it</h4>
      {list.length === 0 ? (
        <p className="record-note">No screen text was recorded while this was in front. Screen text pauses on the lock screen, in excluded apps and during password entry.</p>
      ) : (
        <div className="span-texts">
          {list.map((item, i) => {
            const p = item.envelope.payload
            const isOcr = item.envelope.kind !== "text.snapshot"
            const body = isOcr ? ocrText(item.envelope) : (str(p.text) ?? "")
            return (
              <Disclosure
                key={item.id}
                defaultOpen={i === 0}
                title={`${clockSeconds(item.envelope.time.source_start ?? item.envelope.time.observed_at)}${isOcr ? " · from a screenshot" : ""}`}
                description={`${firstLine(body)} · record ${item.id}`}
              >
                {isOcr ? (
                  <pre className="screen-text-body">
                    <Highlight text={body} terms={terms} />
                  </pre>
                ) : (
                  <ScreenText envelope={item.envelope} terms={terms} />
                )}
              </Disclosure>
            )
          })}
          {data.screen_text_total >= 60 && <p className="record-note">Showing the first 60 text records from this span. The timeline shows every one.</p>}
        </div>
      )}
    </section>
  )
}

function firstLine(text: string) {
  const line = text.split("\n").find((l) => l.trim()) ?? ""
  return line.length > 90 ? `${line.slice(0, 89)}…` : line || "Empty"
}

function ocrText(envelope: StoredEnvelope) {
  const ocr = envelope.payload.ocr as { lines?: { text?: string }[] } | undefined
  return (ocr?.lines ?? []).map((l) => l.text ?? "").join("\n")
}

function Body({ data, terms }: { data: CiteResponse; terms?: string[] }) {
  const record: StoredRecord = data.record
  const e = record.envelope
  const p = e.payload
  switch (e.kind) {
    case "text.snapshot":
      return <ScreenText envelope={e} terms={terms} />
    case "focus.span":
      return (
        <>
          <p className="record-line">
            <strong>{str(p.app_name) ?? str(p.app_bundle_id) ?? "An app"}</strong> was in front
            {str(p.window_title) ? (
              <>
                , showing <strong>“{str(p.window_title)}”</strong>
              </>
            ) : null}
            .
          </p>
          <SpanText data={data} terms={terms} />
        </>
      )
    case "media.still": {
      const derived = record.derived_evidence ?? []
      return (
        <>
          <p className="record-note">Only the text read from this screenshot is shown here.</p>
          {derived.length === 0 ? (
            <p className="record-note">No text was read from it.</p>
          ) : (
            derived.map((d) => (
              <pre key={d.id} className="screen-text-body">
                <Highlight text={ocrText(d.envelope)} terms={terms} />
              </pre>
            ))
          )}
        </>
      )
    }
    case "ocr.text":
      return (
        <pre className="screen-text-body">
          <Highlight text={ocrText(e)} terms={terms} />
        </pre>
      )
    case "ui.click":
      return (
        <p className="record-line">
          Clicked {str(p.label) ? <strong>“{str(p.label)}”</strong> : "an element"}
          {str(p.role) ? ` (${String(p.role).replace(/^AX/, "")})` : ""}
          {str(p.app_bundle_id) ? ` in ${p.app_bundle_id}` : ""}.
        </p>
      )
    case "presence.span":
      return (
        <>
          <p className="record-line">
            The Mac was <strong>{String(p.state ?? "active").replace(/_/g, " ")}</strong>.
          </p>
          <SpanText data={data} terms={terms} />
        </>
      )
    case "input.bucket":
      return (
        <p className="record-line">
          {Number(p.keys ?? 0)} keys, {Number(p.clicks ?? 0)} clicks and {Number(p.scroll_events ?? 0)} scrolls. Only counts are stored, never which keys.
        </p>
      )
    case "policy.gap":
      return <p className="record-line">Recording stopped here: {String(p.reason ?? "unknown reason").replace(/_/g, " ")}.</p>
    default:
      return <KeyValues payload={p} />
  }
}

function KeyValues({ payload }: { payload: Record<string, unknown> }) {
  const rows = Object.entries(payload).filter(([, v]) => v !== null && v !== "" && typeof v !== "object")
  return (
    <dl className="kv">
      {rows.map(([k, v]) => (
        <div key={k}>
          <dt>{k.replace(/_/g, " ")}</dt>
          <dd>{String(v)}</dd>
        </div>
      ))}
    </dl>
  )
}

/** One stored record, shown as a person reads it, with what the AI read for it and the raw record. */
function RecordView({ data, terms, extra }: { data: CiteResponse; terms?: string[]; extra?: ReactNode }) {
  const e = data.record.envelope
  return (
    <article className="record">
      <RecordHead envelope={e} id={data.record.id} />
      {data.record.sensitivity !== "normal" && (
        <div>
          <Badge tone="warning">{data.record.sensitivity === "restricted" ? "Private: stays on your server" : data.record.sensitivity}</Badge>
        </div>
      )}
      <Body data={data} terms={terms} />
      <Seen seen={data.seen} />
      {extra}
      <Disclosure title="Stored record" description="The exact record as the server keeps it">
        <pre className="raw-json">{JSON.stringify(data.record, null, 2)}</pre>
      </Disclosure>
    </article>
  )
}

export { Highlight, RecordView, kindInfo }
