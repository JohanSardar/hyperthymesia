import { Check, ExternalLink, LogIn, RotateCw } from "lucide-react"
import { useCallback, useEffect, useRef, useState } from "react"
import { provider as providerInfo, ProviderTile } from "../model/providers"
import { api, type ProviderId, type SigninStatus } from "../server/api"
import { Button, Callout, Sheet, Spinner } from "../ui"
import "./agent-parts.css"

/** Runs the provider's own login through fx on the server and watches it finish. */
function useSignin(id: ProviderId, onDone?: () => void) {
  const [status, setStatus] = useState<SigninStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const done = useRef(onDone)
  done.current = onDone

  const start = useCallback(async () => {
    setError(null)
    setStatus({ provider: id, state: "waiting", signed_in: false })
    try {
      setStatus(await api.signinStart(id))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The sign-in didn't start")
      setStatus(null)
    }
  }, [id])

  const waiting = status?.state === "waiting"
  useEffect(() => {
    if (!waiting) return
    const timer = window.setInterval(async () => {
      try {
        const next = await api.signinStatus(id)
        setStatus(next)
        if (next.state === "done") done.current?.()
      } catch {
        // Keep the last state; the next poll tries again.
      }
    }, 1000)
    return () => window.clearInterval(timer)
  }, [id, waiting])

  const cancel = useCallback(async () => {
    setStatus(null)
    await api.signinCancel(id).catch(() => {})
  }, [id])

  return { status, error, start, cancel }
}

/** The sign-in, inline: one button, then live state until fx holds the login. */
function SigninPanel({ id, onDone, compact }: { id: ProviderId; onDone?: () => void; compact?: boolean }) {
  const p = providerInfo(id)
  const { status, error, start, cancel } = useSignin(id, onDone)
  const state = status?.state ?? "idle"
  return (
    <div className="signin-panel" data-state={state} data-compact={compact ? "" : undefined}>
      {state === "done" ? (
        <p className="signin-line" data-tone="ok">
          <span className="signin-mark" aria-hidden>
            <Check size={12} strokeWidth={3} />
          </span>
          Signed in to {p.name}
        </p>
      ) : state === "waiting" ? (
        <>
          <p className="signin-line">
            <Spinner size={13} />
            {status?.url ? `Finish signing in with your ${p.account} account in the browser` : `Opening the ${p.account} sign-in page…`}
          </p>
          <span className="signin-actions">
            {status?.url && (
              <Button variant="ghost" size="xs" render={<a href={status.url} target="_blank" rel="noreferrer" />}>
                Open the page again <ExternalLink size={11} aria-hidden />
              </Button>
            )}
            <Button variant="ghost" size="xs" onClick={cancel}>
              Cancel
            </Button>
          </span>
        </>
      ) : (
        <>
          {state === "failed" && <Callout tone="warning" title="The sign-in didn't finish">Nothing changed. Try again when you're ready.</Callout>}
          {error && <Callout tone="warning" title="The sign-in didn't start">{error}</Callout>}
          <span className="signin-actions">
            <Button variant="primary" onClick={start}>
              {state === "failed" ? <RotateCw size={13} aria-hidden /> : <LogIn size={13} aria-hidden />}
              {state === "failed" ? "Try again" : `Sign in with ${p.account}`}
            </Button>
          </span>
        </>
      )}
      {!compact && state !== "done" && (
        <p className="signin-help">
          {p.name}'s own page opens in your browser. Your password goes only to {p.host}; fx keeps the login on this computer and Hyperthymesia never sees it.
        </p>
      )}
    </div>
  )
}

function SigninSheet({ id, open, onOpenChange, onDone }: { id: ProviderId | null; open: boolean; onOpenChange: (open: boolean) => void; onDone?: () => void }) {
  const p = id ? providerInfo(id) : null
  return (
    <Sheet
      open={open && Boolean(id)}
      onOpenChange={onOpenChange}
      title={
        p ? (
          <span className="model-sheet-title">
            <ProviderTile id={p.id} size={24} />
            Sign in to {p.name}
          </span>
        ) : (
          "Sign in"
        )
      }
      description={p ? `Uses your ${p.account} plan. Nothing is charged per run.` : undefined}
    >
      {id && (
        <SigninPanel
          key={`${id}-${open}`}
          id={id}
          onDone={() => {
            onDone?.()
            window.setTimeout(() => onOpenChange(false), 900)
          }}
        />
      )}
    </Sheet>
  )
}

export { SigninPanel, SigninSheet, useSignin }
