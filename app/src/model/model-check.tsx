import { useState } from "react"
import { api } from "../server/api"
import { Badge, Button, toast } from "../ui"
import "./model.css"

// One small paid request with the saved model; never shows provider output.
function ModelCheckButton() {
  const [result, setResult] = useState<{ ok: boolean; message: string } | null>(null)
  const run = async () => {
    setResult(null)
    try {
      const next = await api.checkModel()
      setResult(next)
      if (next.ok) toast.success("The model answered")
      else toast.error("The model didn't answer", { description: next.message })
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : "The check didn't run"
      setResult({ ok: false, message })
      toast.error("The check didn't run", { description: message })
    }
  }
  return (
    <>
      {result && (
        <span className="check-result" title={result.message}>
          <Badge tone={result.ok ? "success" : "danger"}>{result.ok ? "It answered" : "No answer"}</Badge>
        </span>
      )}
      <Button onClick={run}>Send a test</Button>
    </>
  )
}

export { ModelCheckButton }
