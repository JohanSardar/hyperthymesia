import { ArrowLeft, Check, Copy, History, Eye, LogIn, MonitorSmartphone } from "lucide-react"
import { useEffect, useState, type ReactNode } from "react"
import { useNavigate } from "react-router"
import { effortLabel, ModelSheet } from "../model/model-sheet"
import { ModelCheckButton } from "../model/model-check"
import { provider, ProviderTile } from "../model/providers"
import { api, type Today } from "../server/api"
import { useSettings } from "../server/settings"
import { Button, Card, IconButton, Row, Spinner, StatusDot, Tile, Toaster, toast } from "../ui"
import "./first-run.css"

const steps = ["Model", "This Mac", "Done"] as const

function FirstRunPage() {
  const { data, error } = useSettings()
  const [step, setStep] = useState(0)
  const [direction, setDirection] = useState<"forward" | "back">("forward")
  const go = (next: number) => {
    setDirection(next > step ? "forward" : "back")
    setStep(next)
  }

  return (
    <div className="first-run">
      <header className="first-run-bar">
        <span className="wordmark">Hyperthymesia</span>
        <ol className="first-run-steps" aria-label="Setup steps">
          {steps.map((label, index) => (
            <li key={label} data-state={index < step ? "done" : index === step ? "current" : "next"} aria-current={index === step ? "step" : undefined}>
              <span className="first-run-step-mark">{index < step ? <Check size={10} strokeWidth={3} aria-hidden /> : index + 1}</span>
              {label}
            </li>
          ))}
        </ol>
      </header>
      <main className="first-run-panel">
        <div className="first-run-progress" style={{ ["--progress" as string]: `${((step + 1) / steps.length) * 100}%` }} aria-hidden />
        <div className="first-run-scroll">
          {!data ? (
            <div className="first-run-loading">{error ? <p className="row-description">{error}</p> : <Spinner />}</div>
          ) : (
            <div key={step} className="first-run-step" data-direction={direction}>
              {step === 0 && <ModelStep onNext={() => go(1)} />}
              {step === 1 && <MacStep onBack={() => go(0)} onNext={() => go(2)} />}
              {step === 2 && <DoneStep onBack={() => go(1)} />}
            </div>
          )}
        </div>
      </main>
      <Toaster />
    </div>
  )
}

function StepHead({ eyebrow, title, description }: { eyebrow: string; title: string; description: ReactNode }) {
  return (
    <div className="first-run-head">
      <span className="first-run-eyebrow">{eyebrow}</span>
      <h1 className="first-run-title">{title}</h1>
      <p className="first-run-description">{description}</p>
    </div>
  )
}

function ModelStep({ onNext }: { onNext: () => void }) {
  const { data } = useSettings()
  const [open, setOpen] = useState(false)
  const model = data?.settings.model ?? null
  return (
    <>
      <StepHead
        eyebrow="Step 1 of 3"
        title="Choose a model"
        description="Your agent reads what you did and writes it into your history. Pick the provider and model it runs on. You can change it any time in Settings."
      />
      <Card>
        {model ? (
          <Row
            leading={<ProviderTile id={model.provider} />}
            label={model.model}
            description={[provider(model.provider).name, model.effort ? `${effortLabel(model.effort)} effort` : "Model's default effort"].join(" · ")}
            control={<Button onClick={() => setOpen(true)}>Change</Button>}
          />
        ) : (
          <Row
            leading={<span className="tile-empty" aria-hidden />}
            label="No model chosen"
            description="Nothing is picked for you. Any provider and model works"
            control={
              <Button variant="primary" onClick={() => setOpen(true)}>
                Choose a model
              </Button>
            }
          />
        )}
        {model && (
          <Row label="Try it" description="Sends one short request with this model. Your provider may charge a little for it." control={<ModelCheckButton />} />
        )}
      </Card>
      <footer className="first-run-actions">
        <span />
        <Button variant="primary" size="md" disabled={!model} onClick={onNext}>
          Continue
        </Button>
      </footer>
      <ModelSheet open={open} onOpenChange={setOpen} />
    </>
  )
}

function useToday() {
  const [today, setToday] = useState<Today | null>(null)
  useEffect(() => {
    let live = true
    const load = () =>
      api
        .today()
        .then((value) => live && setToday(value))
        .catch(() => {})
    void load()
    const timer = window.setInterval(load, 5000)
    return () => {
      live = false
      window.clearInterval(timer)
    }
  }, [])
  return today
}

const command = "hx-server setup"

function MacStep({ onBack, onNext }: { onBack: () => void; onNext: () => void }) {
  const { data } = useSettings()
  const today = useToday()
  const chosen = data?.settings.model ? provider(data.settings.model.provider) : null
  const recording = (today?.entries_today ?? 0) > 0
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(command)
      toast.success("Copied", { description: "Paste it into Terminal on this Mac" })
    } catch {
      toast.error("Couldn't copy", { description: "Select the command and copy it yourself" })
    }
  }
  return (
    <>
      <StepHead
        eyebrow="Step 2 of 3"
        title="Let this Mac record"
        description="Hyperthymesia notes which apps you use and reads the text on your screen. macOS only lets you allow that yourself, so a small guide walks you through it."
      />
      <Card>
        <Row leading={<Tile size={36}><span className="tile-glyph"><Eye size={16} strokeWidth={1.75} /></span></Tile>} label="Accessibility" description="See which app and window are in front" />
        <Row leading={<Tile size={36}><span className="tile-glyph"><MonitorSmartphone size={16} strokeWidth={1.75} /></span></Tile>} label="Screen Recording" description="Read the text on your screen, every few seconds" />
        {chosen?.kind === "signin" && (
          <Row leading={<Tile size={36}><span className="tile-glyph"><LogIn size={16} strokeWidth={1.75} /></span></Tile>} label={`Sign in to ${chosen.name}`} description={`Opens ${chosen.host} in your browser if fx isn't signed in yet`} />
        )}
      </Card>
      <Card className="command-card">
        <div className="command-card-head">
          <span className="row-label">Run this in Terminal on this Mac</span>
          <span className="row-description">A panel opens beside System Settings and shows one switch at a time. It never types your password. Safe to run again: it only shows what's left.</span>
        </div>
        <div className="command-line">
          <code>
            <span className="command-prompt">$</span> {command}
          </code>
          <IconButton label="Copy" onClick={copy}>
            <Copy size={14} aria-hidden />
          </IconButton>
        </div>
        <div className="command-status" data-live={recording ? "" : undefined}>
          {recording ? (
            <>
              <StatusDot tone="live" /> Entries are arriving. {today!.entries_today.toLocaleString()} so far today
            </>
          ) : (
            <>
              <Spinner size={12} /> Waiting for the first entries from this Mac
            </>
          )}
        </div>
      </Card>
      <footer className="first-run-actions">
        <Button variant="ghost" size="md" onClick={onBack}>
          <ArrowLeft size={14} aria-hidden /> Back
        </Button>
        <span className="first-run-actions-end">
          {!recording && (
            <Button variant="ghost" size="md" onClick={onNext}>
              Skip for now
            </Button>
          )}
          <Button variant="primary" size="md" onClick={onNext}>
            Continue
          </Button>
        </span>
      </footer>
    </>
  )
}

function DoneStep({ onBack }: { onBack: () => void }) {
  const { data } = useSettings()
  const navigate = useNavigate()
  const today = useToday()
  const model = data?.settings.model
  const recording = (today?.entries_today ?? 0) > 0
  return (
    <>
      <div className="done-mark" aria-hidden>
        <Check size={20} strokeWidth={2.5} />
      </div>
      <StepHead eyebrow="Step 3 of 3" title="You're set" description="Your history starts filling in from here. Everything below can be changed later in Settings." />
      <Card>
        {model && <Row leading={<ProviderTile id={model.provider} />} label="Model" description={`${provider(model.provider).name} · ${model.model}`} />}
        <Row
          leading={<Tile size={36}><span className="tile-glyph"><MonitorSmartphone size={16} strokeWidth={1.75} /></span></Tile>}
          label="This Mac"
          description={recording ? `Entries are arriving · ${today!.entries_today.toLocaleString()} today` : "Not sending yet. Run hx-server setup on the Mac whenever you're ready"}
          control={<StatusDot tone={recording ? "live" : "idle"} />}
        />
        <Row
          leading={<Tile size={36}><span className="tile-glyph"><History size={16} strokeWidth={1.75} /></span></Tile>}
          label="Passes"
          description="Every 4 hours the agent reads the new entries and writes them up. You'll see each pass under Runs"
        />
      </Card>
      <footer className="first-run-actions">
        <Button variant="ghost" size="md" onClick={onBack}>
          <ArrowLeft size={14} aria-hidden /> Back
        </Button>
        <Button variant="primary" size="md" onClick={() => navigate("/today")}>
          Open Today
        </Button>
      </footer>
    </>
  )
}

export { FirstRunPage }
