import { ChevronLeft, Plus, Trash2 } from "lucide-react"
import { ConnectionsSidebar } from "./sidebar"
import { useMemo, useState } from "react"
import { Link, useSearchParams } from "react-router"
import { ApiError } from "../server/http"
import { request } from "../server/http"
import type { ConnectBody, ConnectionTemplate } from "../server/connections"
import { PageHeader } from "../shell"
import {
  Button,
  Card,
  ChoiceCard,
  Disclosure,
  Field,
  IconButton,
  Input,
  Page,
  PageIntro,
  RadioGroup,
  Row,
  SecretInput,
  Section,
  Segmented,
  Select,
  Textarea,
  toast,
} from "../ui"
import { useConnect } from "./detail"
import { HISTORY, useDefaultHistory } from "./parts"
import "./connections.css"

type Kind = "mcp" | "api" | "cli" | "import"
type Pair = { id: number; name: string; value: string }

let nextId = 1
const pair = (name = "", value = ""): Pair => ({ id: nextId++, name, value })

const TITLES: Record<Kind, { title: string; description: string; noun: string }> = {
  mcp: { title: "Add MCP server", description: "Connect an MCP server the agent can read from", noun: "server" },
  api: { title: "Add an API", description: "Connect any read-only JSON API the agent can pull from", noun: "API" },
  cli: { title: "Add a command line tool", description: "Connect a tool already installed and signed in on this Mac", noun: "tool" },
  import: { title: "Paste a config", description: "Bring in one MCP server from an mcpServers JSON", noun: "server" },
}

function toMap(pairs: Pair[]) {
  return Object.fromEntries(pairs.filter((p) => p.name.trim()).map((p) => [p.name.trim(), p.value]))
}

/** Pulls one server out of a pasted mcpServers JSON to fill the form. */
function parseConfig(text: string): { name?: string; url?: string; headers?: Record<string, string>; command?: string; args?: string[]; env?: Record<string, string> } | string {
  let data: unknown
  try {
    data = JSON.parse(text)
  } catch {
    return "That isn't valid JSON"
  }
  let name: string | undefined
  let server = data as Record<string, unknown>
  const servers = (server?.mcpServers ?? server?.servers) as Record<string, Record<string, unknown>> | undefined
  if (servers && typeof servers === "object") {
    const names = Object.keys(servers)
    if (!names.length) return "No servers in that config"
    name = names[0]
    server = servers[name]
  }
  if (typeof server?.url === "string") return { name, url: server.url, headers: (server.headers as Record<string, string>) ?? {} }
  if (typeof server?.command === "string") return { name, command: server.command, args: (server.args as string[]) ?? [], env: (server.env as Record<string, string>) ?? {} }
  return "Couldn't find a url or a command in it"
}

function PairList({
  pairs,
  setPairs,
  namePlaceholder,
  valuePlaceholder,
  addLabel,
  secret,
}: {
  pairs: Pair[]
  setPairs: (next: Pair[]) => void
  namePlaceholder: string
  valuePlaceholder: string
  addLabel: string
  secret?: boolean
}) {
  const update = (id: number, patch: Partial<Pair>) => setPairs(pairs.map((p) => (p.id === id ? { ...p, ...patch } : p)))
  return (
    <div className="kv-list">
      {pairs.map((p) => (
        <div className="kv-row" key={p.id}>
          <Input mono value={p.name} placeholder={namePlaceholder} onChange={(e) => update(p.id, { name: e.currentTarget.value })} aria-label="Name" />
          {secret ? (
            <SecretInput value={p.value} onValueChange={(value) => update(p.id, { value })} placeholder={valuePlaceholder} />
          ) : (
            <Input mono value={p.value} placeholder={valuePlaceholder} onChange={(e) => update(p.id, { value: e.currentTarget.value })} aria-label="Value" />
          )}
          <IconButton label="Remove" onClick={() => setPairs(pairs.filter((x) => x.id !== p.id))}>
            <Trash2 size={14} aria-hidden />
          </IconButton>
        </div>
      ))}
      <Button variant="soft" onClick={() => setPairs([...pairs, pair()])}>
        <Plus size={14} aria-hidden /> {addLabel}
      </Button>
    </div>
  )
}

function ArgList({ args, setArgs }: { args: Pair[]; setArgs: (next: Pair[]) => void }) {
  return (
    <div className="kv-list">
      {args.map((a) => (
        <div className="kv-row single" key={a.id}>
          <Input mono value={a.value} placeholder="--readonly" onChange={(e) => setArgs(args.map((x) => (x.id === a.id ? { ...x, value: e.currentTarget.value } : x)))} aria-label="Argument" />
          <IconButton label="Remove" onClick={() => setArgs(args.filter((x) => x.id !== a.id))}>
            <Trash2 size={14} aria-hidden />
          </IconButton>
        </div>
      ))}
      <Button variant="soft" onClick={() => setArgs([...args, pair()])}>
        <Plus size={14} aria-hidden /> Add argument
      </Button>
    </div>
  )
}

function AddConnectionPage() {
  const [params, setParams] = useSearchParams()
  const kind = (["mcp", "api", "cli", "import"].includes(params.get("kind") ?? "") ? params.get("kind") : "mcp") as Kind
  const connect = useConnect()

  const [name, setName] = useState("")
  const [description, setDescription] = useState("")
  const defaultHistory = useDefaultHistory()
  const [history, setHistory] = useState(defaultHistory)

  const [transport, setTransport] = useState<"http" | "stdio">("http")
  const [url, setUrl] = useState("")
  const [command, setCommand] = useState("")
  const [args, setArgs] = useState<Pair[]>([])
  const [cliHome, setCliHome] = useState("")

  const [baseUrl, setBaseUrl] = useState("")
  const [probePath, setProbePath] = useState("")
  const [method, setMethod] = useState<"GET" | "POST">("GET")
  const [body, setBody] = useState("")

  const [auth, setAuth] = useState<"token" | "none" | "oauth">("token")
  const [token, setToken] = useState("")
  const [headers, setHeaders] = useState<Pair[]>(() => [pair("Authorization", "Bearer {{key}}")])
  const [variables, setVariables] = useState<Pair[]>([])

  const [oauth, setOauth] = useState({ client_id: "", client_secret: "", authorization_url: "", token_url: "", scopes: "", origins: "" })
  const [oauthState, setOauthState] = useState<string | null>(null)
  const [redirected, setRedirected] = useState("")

  const [config, setConfig] = useState("")
  const [importName, setImportName] = useState("")

  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const meta = TITLES[kind]
  const httpLike = (kind === "mcp" && transport === "http") || kind === "api"
  const usesToken = httpLike && auth === "token"

  const valid = useMemo(() => {
    if (kind === "import") return config.trim().length > 0
    if (!name.trim()) return false
    if (kind === "api" && auth === "oauth") return Boolean(baseUrl.trim() && oauth.client_id && oauth.authorization_url && oauth.token_url && oauth.scopes)
    if (usesToken && !token.trim()) return false
    if (kind === "mcp") return transport === "http" ? /^https?:\/\//.test(url.trim()) : command.trim().length > 0
    if (kind === "api") return /^https?:\/\//.test(baseUrl.trim()) && probePath.trim().length > 0
    return command.trim().length > 0
  }, [kind, name, auth, baseUrl, oauth, usesToken, token, transport, url, command, probePath, config])

  function template(): ConnectionTemplate {
    const hdrs = usesToken ? toMap(headers) : {}
    if (kind === "mcp" && transport === "http") return { kind: "mcp", transport: "http", url: url.trim(), headers: hdrs }
    if (kind === "mcp") return { kind: "mcp", transport: "stdio", argv: [command.trim(), ...args.map((a) => a.value).filter(Boolean)], env: Object.fromEntries(variables.filter((v) => v.name.trim()).map((v) => [v.name.trim(), `\${${v.name.trim()}}`])) }
    if (kind === "cli") return { kind: "cli", argv: [command.trim(), ...args.map((a) => a.value).filter(Boolean)], env: {} }
    let parsedBody: unknown = null
    if (method === "POST" && body.trim()) parsedBody = JSON.parse(body)
    return { kind: "api", base_url: baseUrl.trim(), probe_path: probePath.trim(), method, headers: hdrs, body: parsedBody }
  }

  async function startOauth() {
    const origins = oauth.origins.split(/[\s,]+/).filter(Boolean)
    const started = await request<{ state: string; authorization_url: string }>("/v1/connections/oauth", {
      method: "POST",
      body: {
        name: name.trim(),
        description: description.trim() || undefined,
        history_days: Number(history),
        config: {
          client_id: oauth.client_id.trim(),
          client_secret: oauth.client_secret,
          authorization_url: oauth.authorization_url.trim(),
          token_url: oauth.token_url.trim(),
          redirect_uri: `${window.location.origin}/connect/oauth/callback`,
          scopes: oauth.scopes.trim(),
          origins: origins.length ? origins : [new URL(baseUrl.trim()).origin],
        },
      },
    })
    setOauthState(started.state)
    window.open(started.authorization_url, "_blank", "noopener")
    toast.info("Finish signing in in the new tab", { description: "Your connection appears here when you're done" })
  }

  async function finishOauth() {
    const done = await request<{ id: string; status: string }>("/v1/connections/oauth/finish", { method: "POST", body: { state: oauthState, redirected_url: redirected.trim() } })
    toast.success(`${name.trim()} connected`, { description: "The agent is setting it up now" })
    window.location.assign(`/app/agent?focus=${done.id}`)
  }

  async function submit() {
    if (!valid || busy) return
    setBusy(true)
    setError(null)
    try {
      if (kind === "api" && auth === "oauth") {
        await startOauth()
        setBusy(false)
        return
      }
      let payload: ConnectBody
      if (kind === "import") {
        let parsed: unknown
        try {
          parsed = JSON.parse(config)
        } catch {
          throw new Error("That isn't valid JSON")
        }
        payload = {
          import_config: parsed,
          import_name: importName.trim() || undefined,
          name: name.trim() || undefined,
          description: description.trim() || undefined,
          variables: variables.length ? toMap(variables) : undefined,
          history_days: Number(history),
        }
      } else {
        payload = {
          name: name.trim(),
          description: description.trim() || undefined,
          history_days: Number(history),
          connection: template(),
          key: usesToken ? token.trim() : undefined,
          variables: kind === "mcp" && variables.length ? toMap(variables) : undefined,
          cli_home: kind === "cli" && cliHome.trim() ? cliHome.trim() : undefined,
        }
      }
      await connect(payload, name.trim() || "Your server")
    } catch (e) {
      setError(e instanceof ApiError || e instanceof Error ? e.message : "The server couldn't be reached")
      setBusy(false)
    }
  }

  function applyImport() {
    const result = parseConfig(config)
    if (typeof result === "string") return setError(result)
    setError(null)
    setParams({ kind: "mcp" })
    if (result.name && !name) setName(result.name)
    if (result.url) {
      setTransport("http")
      setUrl(result.url)
      const h = Object.entries(result.headers ?? {})
      if (h.length) {
        setAuth("token")
        setHeaders(h.map(([k, v]) => pair(k, v)))
      } else setAuth("none")
    } else if (result.command) {
      setTransport("stdio")
      setCommand(result.command)
      setArgs((result.args ?? []).map((a) => pair("", a)))
      setVariables(Object.keys(result.env ?? {}).map((k) => pair(k, "")))
    }
    toast.success("Filled in from the config", { description: "Check the fields, add any secrets, then add it" })
  }

  const addButton = (
    <Button variant="primary" disabled={!valid || busy} onClick={submit}>
      {busy ? "Checking access…" : kind === "api" && auth === "oauth" ? "Sign in" : `Add ${meta.noun}`}
    </Button>
  )

  return (
    <>
      <ConnectionsSidebar />
      <PageHeader crumbs={[{ label: "Connections", to: "/connections" }, { label: meta.title }]} />
      <Page width="settings">
        <Link to="/connections" className="back-link">
          <ChevronLeft size={14} aria-hidden /> Connections
        </Link>
        <PageIntro title={meta.title} description={meta.description} />

        <Section title={kind === "import" ? "Config" : "Source"} actions={addButton}>
          <Card>
            {kind === "import" ? (
              <div className="connect-card-body">
                <Field label="mcpServers JSON" description="One server per connection. Use ${NAME} for secrets and fill them in under Variables.">
                  <Textarea rows={9} value={config} onChange={(e) => setConfig(e.currentTarget.value)} placeholder={'{\n  "mcpServers": {\n    "notes": { "url": "https://mcp.example.com/mcp", "headers": { "Authorization": "Bearer ${TOKEN}" } }\n  }\n}'} />
                </Field>
                <div className="field-pair">
                  <Field label="Server to import" description="Only needed when the config holds several">
                    <Input value={importName} onChange={(e) => setImportName(e.currentTarget.value)} placeholder="notes" />
                  </Field>
                  <Field label="Name" description="Optional">
                    <Input value={name} onChange={(e) => setName(e.currentTarget.value)} placeholder="My notes" />
                  </Field>
                </div>
              </div>
            ) : (
              <>
                <div className="connect-card-body">
                  <Field label="Name">
                    <Input value={name} onChange={(e) => setName(e.currentTarget.value)} placeholder={kind === "mcp" ? "Personal notes" : kind === "api" ? "My activity" : "Journal"} autoFocus />
                  </Field>
                  <Field label="What it holds" description="A sentence for the agent: what's there and where. Never put tokens here.">
                    <Input value={description} onChange={(e) => setDescription(e.currentTarget.value)} placeholder={kind === "api" ? "Recent activity. Records at /activity have id, occurred_at and title." : "Recent notes with created and updated times"} />
                  </Field>
                </div>
                <Row
                  label="Type"
                  description="How it's reached"
                  control={
                    <Select
                      width={176}
                      value={kind}
                      onValueChange={(v) => v && setParams({ kind: v })}
                      items={[
                        { value: "mcp", label: "MCP server" },
                        { value: "api", label: "API" },
                        { value: "cli", label: "Command line tool" },
                      ]}
                      aria-label="Type"
                    />
                  }
                />
              </>
            )}
            <Row label="Start from" description="How far back the first pull reaches" control={<Select width={176} items={HISTORY} value={history} onValueChange={(v) => setHistory(v ?? defaultHistory)} aria-label="Start from" />} />
          </Card>
        </Section>

        {kind !== "import" && (
          <Section title="Runtime">
            <Card>
              {kind === "mcp" && (
                <Row
                  label="Transport"
                  description={transport === "http" ? "Speaks MCP over HTTP to a listening server" : "Launches a process on this Mac and talks over stdin and stdout"}
                  control={
                    <Select
                      width={176}
                      value={transport}
                      onValueChange={(v) => v && setTransport(v as "http" | "stdio")}
                      items={[
                        { value: "http", label: "Streamable HTTP" },
                        { value: "stdio", label: "STDIO" },
                      ]}
                      aria-label="Transport"
                    />
                  }
                />
              )}
              <div className="connect-card-body">
                {kind === "mcp" && transport === "http" && (
                  <Field label="URL">
                    <Input mono value={url} onChange={(e) => setUrl(e.currentTarget.value)} placeholder="https://mcp.example.com/mcp" />
                  </Field>
                )}
                {kind === "api" && (
                  <>
                    <Field label="Base URL" description="End a folder with a slash so paths resolve inside it">
                      <Input mono value={baseUrl} onChange={(e) => setBaseUrl(e.currentTarget.value)} placeholder="https://api.example.com/v1/" />
                    </Field>
                    <Field label="Check path" description="One read-only request that proves access, relative to the base URL">
                      <Input mono value={probePath} onChange={(e) => setProbePath(e.currentTarget.value)} placeholder="me" />
                    </Field>
                    <Field label="Method">
                      <Segmented value={method} onValueChange={(v) => setMethod(v as "GET" | "POST")} items={[{ value: "GET", label: "GET" }, { value: "POST", label: "POST" }]} />
                    </Field>
                    {method === "POST" && (
                      <Field label="JSON body" description="For read-only POST APIs such as GraphQL">
                        <Textarea rows={4} value={body} onChange={(e) => setBody(e.currentTarget.value)} placeholder={'{ "query": "{ viewer { id } }" }'} />
                      </Field>
                    )}
                  </>
                )}
                {(kind === "cli" || (kind === "mcp" && transport === "stdio")) && (
                  <>
                    <Field label={kind === "cli" ? "Command" : "Command to launch"} description={kind === "cli" ? "The command and arguments are the read-only check; the agent finds other reads on the same tool" : "An absolute path is safest"}>
                      <Input mono value={command} onChange={(e) => setCommand(e.currentTarget.value)} placeholder={kind === "cli" ? "my-cli" : "/usr/local/bin/notes-mcp"} />
                    </Field>
                    <Field label="Arguments">
                      <ArgList args={args} setArgs={setArgs} />
                    </Field>
                    {kind === "cli" && (
                      <Field label="Home folder" description="Optional. Where the tool keeps its sign-in, if not your home folder">
                        <Input mono value={cliHome} onChange={(e) => setCliHome(e.currentTarget.value)} placeholder="/Users/you" />
                      </Field>
                    )}
                    <p className="field-description">Runs on this Mac. Install it and sign in first; nothing is installed or signed in for you.</p>
                  </>
                )}
              </div>
            </Card>
          </Section>
        )}

        {httpLike && (
          <Section title="Authentication">
            <RadioGroup value={auth} onValueChange={(v) => setAuth(v as typeof auth)} className="choice-stack">
              <ChoiceCard value="token" title="Token" description="A read-only token sent in a request header" />
              {kind === "api" && <ChoiceCard value="oauth" title="Sign in" description="Sign in with the service's own login through OAuth, with refresh" />}
              <ChoiceCard value="none" title="No sign-in" description="The source is open, or protected some other way" />
            </RadioGroup>
            {auth === "token" && (
              <Card>
                <div className="connect-card-body">
                  <Field label="Token" description="Stored privately on this Mac. Use {{key}} in a header to place it.">
                    <SecretInput value={token} onValueChange={setToken} placeholder="Paste a read-only token" />
                  </Field>
                  <Field label="Headers">
                    <PairList pairs={headers} setPairs={setHeaders} namePlaceholder="Authorization" valuePlaceholder="Bearer {{key}}" addLabel="Add header" />
                  </Field>
                </div>
              </Card>
            )}
            {auth === "oauth" && (
              <Card>
                <div className="connect-card-body">
                  <div className="field-pair">
                    <Field label="Client ID">
                      <Input mono value={oauth.client_id} onChange={(e) => setOauth({ ...oauth, client_id: e.currentTarget.value })} />
                    </Field>
                    <Field label="Client secret" description="Leave empty for a public PKCE client">
                      <SecretInput value={oauth.client_secret} onValueChange={(v) => setOauth({ ...oauth, client_secret: v })} />
                    </Field>
                  </div>
                  <Field label="Authorization URL">
                    <Input mono value={oauth.authorization_url} onChange={(e) => setOauth({ ...oauth, authorization_url: e.currentTarget.value })} placeholder="https://accounts.example.com/oauth/authorize" />
                  </Field>
                  <Field label="Token URL">
                    <Input mono value={oauth.token_url} onChange={(e) => setOauth({ ...oauth, token_url: e.currentTarget.value })} placeholder="https://accounts.example.com/oauth/token" />
                  </Field>
                  <div className="field-pair">
                    <Field label="Read scopes">
                      <Input mono value={oauth.scopes} onChange={(e) => setOauth({ ...oauth, scopes: e.currentTarget.value })} placeholder="read" />
                    </Field>
                    <Field label="API origins" description="Defaults to the base URL's">
                      <Input mono value={oauth.origins} onChange={(e) => setOauth({ ...oauth, origins: e.currentTarget.value })} placeholder="https://api.example.com" />
                    </Field>
                  </div>
                  <p className="field-description">
                    Register this redirect with the service: <span className="mono-line">{`${window.location.origin}/connect/oauth/callback`}</span>
                  </p>
                  {oauthState && (
                    <Field label="Landed somewhere else?" description="Paste the full address the browser ended on after you agreed">
                      <div className="kv-row single">
                        <Input mono value={redirected} onChange={(e) => setRedirected(e.currentTarget.value)} placeholder="https://…?code=…&state=…" />
                        <Button disabled={!redirected.trim()} onClick={() => finishOauth().catch((e: Error) => setError(e.message))}>
                          Finish
                        </Button>
                      </div>
                    </Field>
                  )}
                </div>
              </Card>
            )}
          </Section>
        )}

        {(kind === "mcp" || kind === "import") && (
          <Section
            title="Variables"
            description={
              kind === "mcp" && transport === "http"
                ? "Fill ${NAME} in headers. Sent with every call to the server; values are sealed on save."
                : "Passed to the process as environment variables when it launches. Values are sealed on save."
            }
          >
            <Card>
              <div className="connect-card-body">
                <PairList pairs={variables} setPairs={setVariables} namePlaceholder="SERVICE_TOKEN" valuePlaceholder="Value" addLabel="Add variable" secret />
              </div>
            </Card>
          </Section>
        )}

        {kind === "mcp" && (
          <Section>
            <Card>
              <Disclosure title="Advanced" description="Import a configuration">
                <Field label="Import a configuration" description="Paste an mcpServers JSON to fill in this form">
                  <Textarea rows={6} value={config} onChange={(e) => setConfig(e.currentTarget.value)} placeholder='{ "mcpServers": { … } }' />
                </Field>
                <div>
                  <Button disabled={!config.trim()} onClick={applyImport}>
                    Parse
                  </Button>
                </div>
              </Disclosure>
            </Card>
          </Section>
        )}

        {error && <div className="form-error">{error}</div>}
        <div className="form-footer">
          <span className="field-description">Access is checked with one read-only request. Then the agent sets up its pulls.</span>
          {addButton}
        </div>
      </Page>
    </>
  )
}

export { AddConnectionPage }
