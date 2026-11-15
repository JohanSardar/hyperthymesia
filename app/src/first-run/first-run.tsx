import {
  ArrowLeft,
  ArrowRight,
  Check,
  CornerDownLeft,
  Eye,
  EyeOff,
  Keyboard,
  Laptop,
  MonitorSmartphone,
  ScanText,
  Search,
  Server,
  ShieldCheck,
  Sparkles,
  TriangleAlert,
} from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react"
import { useNavigate } from "react-router"
import { ago } from "../agent/format"
import { SigninPanel } from "../agent/signin"
import { TestPanel } from "../agent/test"
import { blurb } from "../connections/blurbs"
import { featured, useCatalog } from "../connections/catalog"
import { Logo } from "../connections/logo"
import { effortLabel, ModelSheet } from "../model/model-sheet"
import { provider, providers, ProviderTile } from "../model/providers"
import { usePolled } from "../server/connections"
import { api, type CheckResult, type GuideStep, type ProviderId, type ProviderStatus } from "../server/api"
import { useSettings } from "../server/settings"
import { Badge, Button, Callout, Card, Input, Spinner, StatusDot, Tile, Toaster, toast } from "../ui"
import "./first-run.css"

const steps = [
  { id: "intro", label: "Welcome" },
  { id: "data", label: "Your data" },
  { id: "permissions", label: "Permissions" },
  { id: "ai", label: "Your AI" },
  { id: "connections", label: "Connections" },
  { id: "proof", label: "Check" },
] as const

type StepId = (typeof steps)[number]["id"]

function FirstRunPage() {
  const { data, error, save } = useSettings()
  const navigate = useNavigate()
  const saved = data?.settings.setup?.step
  const [step, setStep] = useState<number | null>(null)
  const [direction, setDirection] = useState<"forward" | "back">("forward")

  // Resume where the person left off, for example after a notification click.
  useEffect(() => {
    if (step === null && data) {
      const index = steps.findIndex((s) => s.id === saved)
      setStep(index < 0 ? 0 : index)
    }
  }, [data, saved, step])

  const go = useCallback(
    (next: number) => {
      setDirection(next > (step ?? 0) ? "forward" : "back")
      setStep(next)
      void save({ setup: { step: steps[next].id, finished: false, tour_pending: false } }).catch(() => {})
    },
    [save, step],
  )

  const finish = useCallback(async () => {
    try {
      await save({ setup: { step: "done", finished: true, tour_pending: true } })
      navigate("/today", { replace: true })
    } catch (cause) {
      toast.error("Couldn't finish setup", { description: cause instanceof Error ? cause.message : undefined })
    }
  }, [navigate, save])

  const current = step ?? 0
  return (
    <div className="first-run">
      <header className="first-run-bar">
        <span className="wordmark">Hyperthymesia</span>
        <span className="first-run-counter" aria-label={`Step ${current + 1} of ${steps.length}`}>
          {String(current + 1).padStart(2, "0")} <span className="first-run-counter-of">/ {String(steps.length).padStart(2, "0")}</span>
        </span>
      </header>
      <main className="first-run-panel">
        <div className="first-run-progress" style={{ ["--progress" as string]: `${((current + 1) / steps.length) * 100}%` }} aria-hidden />
        <div className="first-run-scroll">
          {!data || step === null ? (
            <div className="first-run-loading">{error ? <p className="row-description">{error}</p> : <Spinner />}</div>
          ) : (
            <div key={step} className="first-run-step" data-direction={direction}>
              <Step id={steps[step].id} index={step} goTo={(id) => go(steps.findIndex((s) => s.id === id))} back={() => go(step - 1)} next={() => (step + 1 < steps.length ? go(step + 1) : void finish())} />
            </div>
          )}
        </div>
      </main>
      <Toaster />
    </div>
  )
}

type StepProps = { index: number; back: () => void; next: () => void; goTo: (id: StepId) => void }

function Step({ id, ...props }: StepProps & { id: StepId }) {
  switch (id) {
    case "intro":
      return <IntroStep {...props} />
    case "data":
      return <DataStep {...props} />
    case "permissions":
      return <PermissionsStep {...props} />
    case "ai":
      return <AiStep {...props} />
    case "connections":
      return <ConnectionsStep {...props} />
    case "proof":
      return <ProofStep {...props} />
  }
}

/** Two-tone headline, one line of help, and Back / Continue on the right. */
function StepHead({ index, muted, bright, description, actions }: { index: number; muted: string; bright: string; description: ReactNode; actions: ReactNode }) {
  return (
    <div className="first-run-head">
      <span className="first-run-eyebrow">{String(index + 1).padStart(2, "0")}</span>
      <h1 className="first-run-title">
        <span className="first-run-title-muted">{muted}</span>
        <span>{bright}</span>
      </h1>
      <div className="first-run-head-row">
        <p className="first-run-description">{description}</p>
        <div className="first-run-head-actions">{actions}</div>
      </div>
    </div>
  )
}

/** Enter continues, unless the person is typing. */
function useEnter(action: () => void, enabled: boolean) {
  const ref = useRef(action)
  ref.current = action
  useEffect(() => {
    if (!enabled) return
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      if (event.key !== "Enter" || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return
      if (target?.closest("input, textarea, button, a, [role=dialog]")) return
      event.preventDefault()
      ref.current()
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [enabled])
}

function Actions({ back, next, nextLabel = "Continue", canContinue = true, skip }: { back?: () => void; next: () => void; nextLabel?: string; canContinue?: boolean; skip?: { label: string; onClick: () => void } }) {
  useEnter(next, canContinue)
  return (
    <>
      {back && (
        <Button variant="ghost" size="md" className="pill" onClick={back}>
          <ArrowLeft size={14} aria-hidden /> Back
        </Button>
      )}
      {skip && !canContinue && (
        <Button variant="ghost" size="md" className="pill" onClick={skip.onClick}>
          {skip.label}
        </Button>
      )}
      <Button variant="primary" size="md" className="pill" disabled={!canContinue} onClick={next}>
        {nextLabel} <CornerDownLeft size={13} aria-hidden />
      </Button>
    </>
  )
}

function useSetupStatus(every = 1500) {
  return usePolled(api.setupStatus, every)
}

// MARK: 1. Intro

function IntroStep({ index, next }: StepProps) {
  return (
    <>
      <StepHead
        index={index}
        muted="Your day, remembered."
        bright="Written up by your own AI."
        description="Hyperthymesia keeps a private history of your days on your own computer."
        actions={<Actions next={next} nextLabel="Get started" />}
      />
      <ul className="intro-list">
        <IntroItem icon={<ScanText size={16} strokeWidth={1.75} />}>It reads the text on your screen every few seconds and notes which app is in front.</IntroItem>
        <IntroItem icon={<Sparkles size={16} strokeWidth={1.75} />}>Every 4 hours your AI turns that into a day page: what you worked on, and when.</IntroItem>
        <IntroItem icon={<Search size={16} strokeWidth={1.75} />}>You can search anything you've seen, and every line links to the record it came from.</IntroItem>
      </ul>
      <Card className="promise">
        <div className="promise-head">
          <ShieldCheck size={16} strokeWidth={1.75} aria-hidden />
          <span className="row-label">What it keeps, and what it never keeps</span>
        </div>
        <ul className="promise-list">
          <li>
            <Eye size={14} aria-hidden />
            <span>
              <strong>Screen text is read on this Mac.</strong> The words become text here, before anything leaves the Mac.
            </span>
          </li>
          <li>
            <EyeOff size={14} aria-hidden />
            <span>
              <strong>No screen image is ever stored.</strong> The picture exists only while its text is read, then it's gone.
            </span>
          </li>
          <li>
            <Keyboard size={14} aria-hidden />
            <span>
              <strong>Keystrokes are never recorded.</strong> It only counts how busy the keyboard and mouse were.
            </span>
          </li>
          <li>
            <Sparkles size={14} aria-hidden />
            <span>
              <strong>Only your AI provider sees what a day page is written from,</strong> through your own login with them.
            </span>
          </li>
        </ul>
      </Card>
    </>
  )
}

function IntroItem({ icon, children }: { icon: ReactNode; children: ReactNode }) {
  return (
    <li className="intro-item">
      <Tile size={36}>
        <span className="tile-glyph">{icon}</span>
      </Tile>
      <span>{children}</span>
    </li>
  )
}

// MARK: 2. Where the data lives

function DataStep({ index, back, next }: StepProps) {
  const status = useSetupStatus(2000)
  const app = status.data?.app
  const [choice, setChoice] = useState<"here" | "elsewhere">(app?.paired && !app.paired_here ? "elsewhere" : "here")
  const [link, setLink] = useState("")
  const [pairing, setPairing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const pairedElsewhere = Boolean(app?.paired && !app.paired_here)
  const result = status.data?.pairing

  useEffect(() => {
    if (!pairing) return
    if (pairedElsewhere) {
      setPairing(false)
      toast.success("Paired", { description: `Recording from this Mac now goes to ${app?.server}` })
    } else if (result?.state === "failed") {
      setPairing(false)
      setError(result.error ? "The link didn't work. It may have expired or been used already; make a new one." : "Pairing didn't finish.")
    }
  }, [pairing, pairedElsewhere, result, app?.server])

  const pair = async () => {
    setError(null)
    try {
      await api.pairElsewhere(link)
      setPairing(true)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "That didn't work")
    }
  }

  const ready = choice === "here" ? Boolean(status.data) : pairedElsewhere
  return (
    <>
      <StepHead
        index={index}
        muted="Where your data lives."
        bright="On this Mac, unless you say otherwise."
        description="Your history is kept by a small Hyperthymesia server. It's already running on this Mac. You can send this Mac's recording to one on another computer you own instead."
        actions={<Actions back={back} next={next} canContinue={ready} />}
      />
      <div className="place-grid" role="radiogroup" aria-label="Where your data lives">
        <button type="button" role="radio" aria-checked={choice === "here"} className="place-card" onClick={() => setChoice("here")}>
          <span className="place-card-top">
            <Tile size={36}>
              <span className="tile-glyph">
                <Laptop size={16} strokeWidth={1.75} />
              </span>
            </Tile>
            <span className="place-radio" aria-hidden />
          </span>
          <span className="row-label">This Mac</span>
          <span className="row-description">Everything stays here. Set up already, nothing to do.</span>
          <span className="place-status">
            {!status.data ? (
              <>
                <Spinner size={11} /> Checking…
              </>
            ) : app?.paired_here ? (
              <>
                <StatusDot tone="ok" /> Ready: the server is running and this Mac is paired with it
              </>
            ) : pairedElsewhere ? (
              <>
                <StatusDot tone="idle" /> Not in use: this Mac sends to {app?.server}
              </>
            ) : (
              <>
                <StatusDot tone="ok" /> The server is running here
              </>
            )}
          </span>
        </button>
        <button type="button" role="radio" aria-checked={choice === "elsewhere"} className="place-card" onClick={() => setChoice("elsewhere")}>
          <span className="place-card-top">
            <Tile size={36}>
              <span className="tile-glyph">
                <Server size={16} strokeWidth={1.75} />
              </span>
            </Tile>
            <span className="place-radio" aria-hidden />
          </span>
          <span className="row-label">Another computer</span>
          <span className="row-description">Pair with a Hyperthymesia server you run elsewhere, using a one-time link.</span>
          <span className="place-status">
            {pairedElsewhere ? (
              <>
                <StatusDot tone="ok" /> Paired with {app?.server}
              </>
            ) : (
              <>
                <StatusDot tone="idle" /> Not paired
              </>
            )}
          </span>
        </button>
      </div>
      {choice === "here" && status.data && !app?.installed && (
        <Callout tone="warning" icon={<TriangleAlert size={14} />} title="The Hyperthymesia Mac app isn't installed">
          The server runs, but nothing records this Mac yet. Run the installer again to add the app.
        </Callout>
      )}
      {choice === "elsewhere" && !pairedElsewhere && (
        <Card className="pair-card">
          <ol className="pair-steps">
            <li>
              <span className="pair-step-mark">1</span>
              <span>
                On the other computer, run <code className="inline-code">hx-server pair --name "{"this Mac"}"</code>. It prints a one-time link that works for 15 minutes.
              </span>
            </li>
            <li>
              <span className="pair-step-mark">2</span>
              <span className="pair-step-body">
                Paste the link here.
                <span className="pair-input">
                  <Input mono placeholder="hyperthymesia://pair?server=…&code=…" value={link} onChange={(e) => setLink(e.currentTarget.value)} aria-label="Pairing link" />
                  <Button variant="primary" disabled={!link.trim() || pairing} onClick={pair}>
                    {pairing ? <Spinner size={12} /> : null} Pair
                  </Button>
                </span>
              </span>
            </li>
          </ol>
          {error && <p className="field-error">{error}</p>}
          <p className="pair-note">From then on this Mac's recording goes to that computer, and its AI writes your day pages. This Mac keeps recording only what you allow in the next step.</p>
        </Card>
      )}
    </>
  )
}

// MARK: 3. Permissions

const permissionCopy = {
  accessibility: {
    name: "Accessibility",
    icon: Eye,
    purpose: "See which app and window are in front, and read the text apps make available. It also lets it count, never record, keys and clicks.",
  },
  screen: {
    name: "Screen Recording",
    icon: MonitorSmartphone,
    purpose: "Read the text on your screen every few seconds. The picture is dropped as soon as its text is read.",
  },
} as const

function guideWords(step: GuideStep | undefined, name: string) {
  if (!step) return null
  const detail = step.detail ?? ""
  if (step.state === "waiting_user" && detail === "macos alert") return "Click Open System Settings when macOS asks"
  if (step.state === "waiting_system" && detail === "macos asks") return "Click Allow when macOS asks"
  if (step.state === "waiting_system" && detail === "confirming") return "Checking that it works…"
  if (step.state === "waiting_user") return `Switch on Hyperthymesia in ${name}, or drag it into the list`
  if (step.state === "waiting_system") return "Confirm with Touch ID or your password"
  if (step.state === "refused" || step.state === "skipped") return "Not changed"
  return null
}

function PermissionsStep({ index, back, next }: StepProps) {
  const status = useSetupStatus(1000)
  const permissions = status.data?.permissions
  const guide = status.data?.guide
  const [asked, setAsked] = useState(false)
  const granted = { accessibility: Boolean(permissions?.accessibility), screen: Boolean(permissions?.screen) }
  const allOn = granted.accessibility && granted.screen
  const guiding = guide?.status === "running"

  const ask = async () => {
    try {
      await api.requestPermissions()
      setAsked(true)
    } catch (cause) {
      toast.error("The guide didn't open", { description: cause instanceof Error ? cause.message : undefined })
    }
  }

  return (
    <>
      <StepHead
        index={index}
        muted="Let this Mac read your screen."
        bright="Two switches, and you flip them."
        description="macOS only lets you turn these on yourself. A small guide opens beside System Settings and shows one switch at a time."
        actions={<Actions back={back} next={next} canContinue={allOn} skip={{ label: "Skip for now", onClick: next }} />}
      />
      {status.data && !permissions ? (
        <Callout tone="warning" icon={<TriangleAlert size={14} />} title={status.data.app.installed ? "The Hyperthymesia Mac app isn't running" : "The Hyperthymesia Mac app isn't installed"}>
          {status.data.app.installed ? "Open Hyperthymesia from Applications. This page updates by itself once it's running." : "Run the installer again to add the app. Nothing on this Mac is recorded without it."}
        </Callout>
      ) : (
        <>
          <div className="permission-grid">
            {(["accessibility", "screen"] as const).map((id) => {
              const copy = permissionCopy[id]
              const Icon = copy.icon
              const on = granted[id]
              const words = !on && guiding ? guideWords(guide?.steps.find((s) => s.id === id), copy.name) : null
              return (
                <Card key={id} className="permission-card" data-on={on ? "" : undefined}>
                  <div className="permission-card-top">
                    <Tile size={36}>
                      <span className="tile-glyph">
                        <Icon size={16} strokeWidth={1.75} />
                      </span>
                    </Tile>
                    {!permissions ? (
                      <Spinner size={12} />
                    ) : on ? (
                      <Badge tone="success">
                        <Check size={11} strokeWidth={3} aria-hidden /> On
                      </Badge>
                    ) : (
                      <Badge tone="neutral">Off</Badge>
                    )}
                  </div>
                  <span className="row-label">{copy.name}</span>
                  <span className="row-description">{copy.purpose}</span>
                  {!on && permissions && (
                    <div className="permission-foot">
                      {words ? (
                        <span className="permission-live">
                          <StatusDot tone="live" /> {words}
                        </span>
                      ) : (
                        <Button onClick={ask}>Turn on</Button>
                      )}
                    </div>
                  )}
                </Card>
              )
            })}
          </div>
          <p className="watching">
            {allOn ? (
              <>
                <StatusDot tone="ok" /> Both are on. Recording has started.
              </>
            ) : (
              <>
                <StatusDot tone="live" /> {asked || guiding ? "Watching for the change. This page updates by itself." : "This page updates by itself as each one is turned on."}
              </>
            )}
          </p>
          {!allOn && !guiding && (
            <div className="permission-cta">
              <Button variant="primary" size="md" className="pill" onClick={ask}>
                Open the guide <ArrowRight size={13} aria-hidden />
              </Button>
              <span className="row-description">Opens System Settings with a small panel beside it. It never types your password.</span>
            </div>
          )}
        </>
      )}
    </>
  )
}

// MARK: 4. Choose the AI

function providerState(p: ProviderStatus | undefined, keyed: boolean) {
  if (!p) return { tone: "idle" as const, text: "Checking…" }
  if (keyed) return p.signed_in ? { tone: "ok" as const, text: "Key saved" } : { tone: "idle" as const, text: "Needs an API key" }
  return p.signed_in ? { tone: "ok" as const, text: "Signed in" } : { tone: "warning" as const, text: "Needs sign-in" }
}

function AiStep({ index, back, next }: StepProps) {
  const { data, reload } = useSettings()
  const model = data?.settings.model ?? null
  const status = usePolled(api.providers, 3000)
  const list = status.data?.providers ?? []
  const [picked, setPicked] = useState<ProviderId | null>(model?.provider ?? null)
  const [sheet, setSheet] = useState(false)
  const [passed, setPassed] = useState<string | null>(null)
  const chosen = picked ? list.find((p) => p.id === picked) : undefined
  const info = picked ? provider(picked) : null
  const modelHere = model && model.provider === picked ? model : null
  const modelKey = modelHere ? `${modelHere.provider}/${modelHere.model}/${modelHere.effort ?? ""}` : null
  // A test that passed for this exact model, now or earlier in this session.
  const lastOk = chosen?.last_check?.state === "done" && chosen.last_check.model === modelHere?.model
  const ok = Boolean(modelKey && (passed === modelKey || lastOk))

  const onResult = (result: CheckResult) => {
    setPassed(result.ok ? modelKey : null)
    void status.refresh()
  }

  return (
    <>
      <StepHead
        index={index}
        muted="Choose your AI."
        bright="It writes your day pages."
        description="It runs on your own provider account, through fx on this computer. Nothing is picked for you; change it any time on the Agent page."
        actions={<Actions back={back} next={next} canContinue={ok} skip={{ label: "Skip for now", onClick: next }} />}
      />
      <div className="provider-tiles" role="radiogroup" aria-label="Provider">
        {providers.map((p) => {
          const state = providerState(
            list.find((s) => s.id === p.id),
            p.kind === "key",
          )
          return (
            <button key={p.id} type="button" role="radio" aria-checked={picked === p.id} className="provider-tile" onClick={() => setPicked(p.id)}>
              <span className="provider-tile-top">
                <ProviderTile id={p.id} />
                <span className="place-radio" aria-hidden />
              </span>
              <span className="provider-tile-text">
                <span className="row-label">{p.name}</span>
                <span className="row-description">{p.blurb}</span>
              </span>
              <span className="place-status">
                <StatusDot tone={state.tone} /> {state.text}
              </span>
            </button>
          )
        })}
      </div>
      {picked && info && (
        <Card className="ai-card">
          <div className="ai-row">
            <span className="ai-row-mark" data-done={chosen?.signed_in ? "" : undefined}>
              {chosen?.signed_in ? <Check size={11} strokeWidth={3} /> : 1}
            </span>
            <div className="ai-row-body">
              <span className="row-label">{info.kind === "key" ? "Add your API key" : `Sign in to ${info.name}`}</span>
              {info.kind === "key" ? (
                chosen?.signed_in ? (
                  <span className="row-description">A key is saved on this computer.</span>
                ) : (
                  <span className="ai-inline">
                    <span className="row-description">Paste a key from your {info.name} dashboard. It stays on this computer and is never shown again.</span>
                    <Button onClick={() => setSheet(true)}>Add key</Button>
                  </span>
                )
              ) : chosen?.signed_in ? (
                <span className="row-description">Signed in with your {info.account} account.</span>
              ) : (
                <SigninPanel id={picked} onDone={() => void status.refresh()} />
              )}
            </div>
          </div>
          <div className="ai-row" data-disabled={chosen?.signed_in ? undefined : ""}>
            <span className="ai-row-mark" data-done={modelHere ? "" : undefined}>
              {modelHere ? <Check size={11} strokeWidth={3} /> : 2}
            </span>
            <div className="ai-row-body">
              <span className="row-label">Pick a model and effort</span>
              <span className="ai-inline">
                <span className="row-description">
                  {modelHere ? `${modelHere.model} · ${modelHere.effort ? `${effortLabel(modelHere.effort)} effort` : "the model's default effort"}` : `Any model ${info.name} offers your account.`}
                </span>
                <Button disabled={!chosen?.signed_in} variant={modelHere ? "default" : "primary"} onClick={() => setSheet(true)}>
                  {modelHere ? "Change" : "Choose a model"}
                </Button>
              </span>
            </div>
          </div>
          <div className="ai-row" data-disabled={modelHere ? undefined : ""}>
            <span className="ai-row-mark" data-done={ok ? "" : undefined}>
              {ok ? <Check size={11} strokeWidth={3} /> : 3}
            </span>
            <div className="ai-row-body">
              <span className="row-label">Test it</span>
              <TestPanel model={modelHere} onResult={onResult} onFix={() => setSheet(true)} />
            </div>
          </div>
        </Card>
      )}
      <ModelSheet
        open={sheet}
        onOpenChange={setSheet}
        initialProvider={picked}
        onSaved={() => {
          void reload()
          void status.refresh()
        }}
      />
    </>
  )
}

// MARK: 5. Optional connections

function ConnectionsStep({ index, back, next }: StepProps) {
  const { catalog } = useCatalog()
  const navigate = useNavigate()
  const items = useMemo(() => (catalog ? featured(catalog).slice(0, 6) : []), [catalog])
  return (
    <>
      <StepHead
        index={index}
        muted="Bring in more, if you like."
        bright="Optional: calendar, mail, notes."
        description="Connections add context to your day pages. Your Mac's screen text is enough on its own, so skip this and add them any time from Connections."
        actions={<Actions back={back} next={next} nextLabel="Skip" />}
      />
      <div className="connect-grid">
        {items.map((entry) => (
          <button key={entry.id} type="button" className="connect-tile" onClick={() => navigate(`/connections/catalog/${entry.id}`)}>
            <Logo id={entry.id} name={entry.name} size={36} />
            <span className="provider-tile-text">
              <span className="row-label">{entry.name}</span>
              <span className="row-description">{blurb(entry)}</span>
            </span>
          </button>
        ))}
        {!catalog &&
          [0, 1, 2].map((i) => (
            <span key={i} className="connect-tile" data-loading="">
              <Spinner size={12} />
            </span>
          ))}
      </div>
      <p className="row-description">Connecting one opens its page; a bar at the top brings you back here.</p>
    </>
  )
}

// MARK: 6. Proof

function ProofStep({ index, back, next, goTo }: StepProps) {
  const proof = usePolled(api.proof, 2000)
  const status = useSetupStatus(5000)
  const [, tick] = useState(0)
  const [waited, setWaited] = useState(0)
  useEffect(() => {
    const timer = window.setInterval(() => {
      tick((n) => n + 1)
      setWaited((n) => n + 1)
    }, 1000)
    return () => window.clearInterval(timer)
  }, [])
  const items = proof.data?.items ?? []
  const app = status.data?.app
  const elsewhere = Boolean(app?.paired && !app.paired_here)
  const screenOff = status.data?.permissions && !status.data.permissions.screen
  const ok = items.length > 0

  return (
    <>
      <StepHead
        index={index}
        muted="Check that it works."
        bright={ok ? "This was on your screen moments ago." : "Looking for your screen text…"}
        description="This is exactly what Hyperthymesia read from your screen in the last two minutes, as text, on this Mac. No image was kept."
        actions={<Actions back={back} next={next} nextLabel="Open my day" canContinue={ok} skip={waited > 20 || elsewhere || screenOff ? { label: "Finish anyway", onClick: next } : undefined} />}
      />
      {elsewhere ? (
        <Callout icon={<Server size={14} />} title={`Recording goes to ${app?.server}`}>
          This Mac sends its screen text to the other computer, so it shows up there. Open Hyperthymesia on that computer to see it arrive and to choose its AI.
        </Callout>
      ) : ok ? (
        <div className="proof-list">
          {items.map((item) => (
            <Card key={item.at} className="proof-card">
              <div className="proof-head">
                <StatusDot tone="live" />
                <span className="row-label">{item.app ?? "An app"}</span>
                {item.title && <span className="proof-title">{item.title}</span>}
                <span className="proof-when">{ago(item.at)}</span>
              </div>
              <div className="proof-lines">
                {item.lines.map((line, i) => (
                  <span key={i}>{line}</span>
                ))}
              </div>
            </Card>
          ))}
        </div>
      ) : (
        <Card className="proof-waiting">
          {screenOff ? <TriangleAlert size={18} className="proof-warn" aria-hidden /> : <Spinner />}
          <span className="row-label">Waiting for the first screen text</span>
          <span className="row-description">
            {screenOff
              ? "Screen Recording is off, so there's no screen text to show. Go back to turn it on."
              : "It reads the screen every few seconds once something changes. Switch to another window or scroll a page to help it along."}
          </span>
          {screenOff && (
            <Button onClick={() => goTo("permissions")}>
              <ArrowLeft size={13} aria-hidden /> Back to permissions
            </Button>
          )}
        </Card>
      )}
    </>
  )
}

export { FirstRunPage }
