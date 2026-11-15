import { Check, Play, TriangleAlert } from "lucide-react"
import { useState } from "react"
import { api, type CheckResult, type ModelChoice } from "../server/api"
import { Button, Spinner } from "../ui"
import { describeProblem, type Fix } from "./problems"
import "./agent-parts.css"

/**
 * Test: one small real request with the saved model, then a plain result. On a
 * failure it names the fix and offers it as a button.
 */
function TestPanel({
  model,
  onResult,
  onFix,
}: {
  model: ModelChoice | null
  onResult?: (result: CheckResult) => void
  onFix?: (fix: Fix) => void
}) {
  const [running, setRunning] = useState(false)
  const [result, setResult] = useState<CheckResult | null>(null)
  const [tested, setTested] = useState<string | null>(null)
  const key = model ? `${model.provider}/${model.model}/${model.effort ?? ""}` : null
  // A result belongs to the model it tested.
  const shown = tested === key ? result : null

  const run = async () => {
    setRunning(true)
    setResult(null)
    try {
      const next = await api.checkModel()
      setResult(next)
      setTested(key)
      onResult?.(next)
    } catch (cause) {
      const next = { ok: false, message: cause instanceof Error ? cause.message : "The test didn't run", problem: "other" }
      setResult(next)
      setTested(key)
      onResult?.(next)
    } finally {
      setRunning(false)
    }
  }
  const problem = shown && !shown.ok ? describeProblem(shown.problem ?? "other", model?.provider ?? null, "check") : null

  return (
    <div className="test-panel" data-state={running ? "running" : shown ? (shown.ok ? "ok" : "failed") : "idle"}>
      <div className="test-line">
        {running ? (
          <span className="test-result">
            <Spinner size={13} /> Asking {model?.model ?? "the model"} to reply…
          </span>
        ) : shown?.ok ? (
          <span className="test-result" data-tone="ok">
            <span className="signin-mark" aria-hidden>
              <Check size={12} strokeWidth={3} />
            </span>
            It works. {model?.model} answered{shown.seconds ? ` in ${shown.seconds.toFixed(1)} s` : ""}.
          </span>
        ) : problem ? (
          <span className="test-result" data-tone="failed">
            <TriangleAlert size={14} aria-hidden /> {problem.title}
          </span>
        ) : (
          <span className="test-result muted">Sends one short request. Subscriptions include it; with an API key your provider charges for one short request.</span>
        )}
        <span className="test-actions">
          {problem?.fix && problem.fix !== "rerun" && onFix && (
            <Button variant="primary" size="sm" onClick={() => onFix(problem.fix)}>
              {problem.fixLabel}
            </Button>
          )}
          <Button variant={shown?.ok || problem?.fix ? "default" : "primary"} size="sm" disabled={!model || running} onClick={run}>
            <Play size={12} aria-hidden /> {shown ? "Test again" : "Test"}
          </Button>
        </span>
      </div>
      {problem && <p className="test-detail">{problem.body}</p>}
    </div>
  )
}

export { TestPanel }
