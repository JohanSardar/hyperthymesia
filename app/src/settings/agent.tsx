import { ArrowRight } from "lucide-react"
import { useState, type ReactNode } from "react"
import { Link } from "react-router"
import { ModelCheckButton } from "../model/model-check"
import { effortLabel, ModelSheet } from "../model/model-sheet"
import { provider, ProviderTile } from "../model/providers"
import { api } from "../server/api"
import { api as connections, usePolled } from "../server/connections"
import { useSettings } from "../server/settings"
import { Badge, Button, Card, Confirm, ConfirmTrigger, Select, buttonAttrs, toast } from "../ui"
import { errorText, RestoreDefaults, Setting, useSave } from "./parts"

function AgentSettingsPage(_: { actionsSlot: (node: ReactNode) => ReactNode }) {
  const { data, reload } = useSettings()
  const [open, setOpen] = useState(false)
  const model = data!.settings.model
  const keySaved = data!.gateway_key_configured
  const limit = data!.settings.spend_limit_usd ?? null
  const removeKey = async () => {
    try {
      await api.removeGatewayKey()
      await reload()
      toast.success("Key removed")
    } catch (cause) {
      toast.error("Couldn't remove the key", { description: errorText(cause) })
    }
  }
  return (
    <>
      <Card>
        <Setting
          id="agent.model"
          leading={model ? <ProviderTile id={model.provider} /> : <span className="tile-empty" aria-hidden />}
          description={
            model
              ? `${model.model} · ${provider(model.provider).name} · ${model.effort ? `${effortLabel(model.effort)} effort` : "the model's default effort"}. Changes apply to the agent's next run`
              : "No model chosen. The agent can't run until you pick one"
          }
          control={
            <Button variant={model ? "default" : "primary"} onClick={() => setOpen(true)}>
              {model ? "Change" : "Choose a model"}
            </Button>
          }
        />
        {model && <Setting id="agent.test" control={<ModelCheckButton />} />}
        {(keySaved || model?.provider === "vercel") && (
          <Setting
            id="agent.gateway"
            description={keySaved ? undefined : "Not saved. Add it from Change › Vercel AI Gateway"}
            control={
              keySaved ? (
                <Confirm
                  title="Remove the gateway key?"
                  description="Vercel AI Gateway models stop working until you add a key again. A key set in the server's environment stays."
                  confirmLabel="Remove key"
                  destructive
                  trigger={<ConfirmTrigger {...buttonAttrs("ghost", "sm")}>Remove</ConfirmTrigger>}
                  onConfirm={removeKey}
                />
              ) : (
                <Badge tone="warning">Missing</Badge>
              )
            }
          />
        )}
      </Card>
      <Card>
        <Setting
          id="agent.page"
          description={`Accounts and sign-in, the schedule, ${limit === null ? "no monthly spending limit" : `a $${limit} monthly spending limit`} and what the agent set up from each connection`}
          control={
            <Button render={<Link to="/agent" />}>
              Open <ArrowRight size={12} aria-hidden />
            </Button>
          }
        />
        <Setting
          id="agent.runs"
          control={
            <Button variant="ghost" render={<Link to="/runs" />}>
              Open <ArrowRight size={12} aria-hidden />
            </Button>
          }
        />
      </Card>
      <ModelSheet open={open} onOpenChange={setOpen} />
    </>
  )
}

const historyOptions = [0, 1, 3, 7, 14, 30]

function ConnectionsSettingsPage({ actionsSlot }: { actionsSlot: (node: ReactNode) => ReactNode }) {
  const { data } = useSettings()
  const save = useSave()
  const sources = usePolled(connections.connections)
  const days = data!.settings.connection_history_days ?? 1
  const on = sources.data?.filter((s) => s.enabled).length ?? 0
  return (
    <>
      {actionsSlot(<RestoreDefaults changes={days === 1 ? [] : ["first pull reaches back 1 day"]} onRestore={() => save({ connection_history_days: 1 }, "Defaults restored")} />)}
      <Card>
        <Setting
          id="connections.list"
          description={sources.data === undefined ? "Loading…" : sources.data.length ? `${sources.data.length} connected, ${on} on` : "Nothing connected yet"}
          control={
            <Button render={<Link to="/connections" />}>
              Open <ArrowRight size={12} aria-hidden />
            </Button>
          }
        />
        <Setting
          id="connections.history"
          control={
            <Select
              aria-label="First pull reaches back"
              width={144}
              value={String(days)}
              onValueChange={(value) => value !== null && save({ connection_history_days: Number(value) })}
              items={historyOptions.map((n) => ({ value: String(n), label: n === 0 ? "Only new entries" : n === 1 ? "1 day" : `${n} days` }))}
            />
          }
        />
      </Card>
      <Card>
        <Setting id="connections.rules" />
        <Setting id="connections.keys" />
        <Setting id="connections.local" />
        <Setting id="connections.remove" />
      </Card>
    </>
  )
}

export { AgentSettingsPage, ConnectionsSettingsPage }
