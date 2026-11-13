import type { CatalogEntry } from "../server/connections"

/**
 * More sources on top of the server's catalog. Each one connects through the same
 * `POST /v1/connections` call as "Add your own", with a ready-made template, so the server
 * probes it read-only and the agent sets up its pulls exactly like any custom connection.
 * Keys always travel in headers, never in URLs.
 */

const TOKEN = "A read-only personal access token for your account."
const CLI = (tool: string) => `Install ${tool} and sign in separately before connecting. Nothing is installed or signed in for you.`

const bearer = { Authorization: "Bearer {{key}}" }

function api(
  id: string,
  name: string,
  category: string,
  description: string,
  base_url: string,
  probe_path: string,
  headers: Record<string, string> = bearer,
  extra: { method?: "POST"; body?: unknown; requirements?: string } = {},
): CatalogEntry {
  return {
    id: `x-${id}`,
    name,
    category,
    description,
    auth: "key",
    requirements: extra.requirements ?? TOKEN,
    variables: [],
    connection: { kind: "api", base_url, probe_path, method: extra.method ?? "GET", headers, body: extra.body ?? null },
  }
}

function graphql(id: string, name: string, category: string, description: string, url: string, query: string, headers: Record<string, string> = bearer) {
  const parsed = new URL(url)
  return api(id, name, category, description, parsed.origin, parsed.pathname, headers, { method: "POST", body: { query } })
}

function mcp(id: string, name: string, category: string, description: string, url: string, requirements = TOKEN): CatalogEntry {
  return {
    id: `x-${id}`,
    name,
    category,
    description,
    auth: "key",
    requirements,
    variables: [],
    connection: { kind: "mcp", transport: "http", url, headers: bearer },
  }
}

function cli(id: string, name: string, category: string, description: string, argv: string[], tool = argv[0]): CatalogEntry {
  return {
    id: `x-${id}`,
    name,
    category,
    description,
    auth: "existing_login",
    requirements: CLI(tool),
    variables: [],
    connection: { kind: "cli", argv, env: {} },
  }
}

const extraCatalog: CatalogEntry[] = [
  // Code
  api("github", "GitHub", "Code", "Read your events, pushes, pull requests and issues through REST. /user identifies you; use /users/{login}/events with bounded dates.", "https://api.github.com/", "user", { ...bearer, Accept: "application/vnd.github+json" }),
  mcp("github-mcp", "GitHub MCP", "Code", "Read your repositories, pull requests and issues through GitHub's remote MCP server. Use read tools only, with bounded recent ranges.", "https://api.githubcopilot.com/mcp/"),
  api("codeberg", "Codeberg", "Code", "Read your activity feed, repositories and pull requests through the Forgejo API. /user identifies you; use bounded activity feeds.", "https://codeberg.org/api/v1/", "user", { Authorization: "token {{key}}" }),
  api("bitbucket", "Bitbucket", "Code", "Read your repositories, pull requests and commits through API 2.0. /user identifies you; bound queries by updated_on.", "https://api.bitbucket.org/2.0/", "user"),
  graphql("sourcehut", "SourceHut", "Code", "Read your profile and repositories through the meta.sr.ht GraphQL API. Use bounded queries.", "https://meta.sr.ht/query", "{ me { canonicalName } }"),
  api("circleci", "CircleCI", "Code", "Read your pipelines and workflow runs. /me identifies you; use bounded recent pipelines.", "https://circleci.com/api/v2/", "me", { "Circle-Token": "{{key}}" }),
  api("buildkite", "Buildkite", "Code", "Read builds you triggered and their states. /user identifies you; use created_from for bounded ranges.", "https://api.buildkite.com/v2/", "user"),
  cli("gh", "GitHub CLI", "Code", "Read through the installed, signed-in gh CLI. gh api user identifies you; read recent events with gh api /users/{login}/events.", ["gh", "api", "user"]),
  cli("glab", "GitLab CLI", "Code", "Read through the installed, signed-in glab CLI. glab api user identifies you; read recent events with glab api events with after/before dates.", ["glab", "api", "user"]),

  // Cloud
  api("vercel", "Vercel", "Cloud", "Read your projects and deployments. /v2/user identifies you; list deployments with bounded since/until timestamps.", "https://api.vercel.com/", "v2/user"),
  api("netlify", "Netlify", "Cloud", "Read your sites and deploys. /user identifies you; read recent deploys per site.", "https://api.netlify.com/api/v1/", "user"),
  api("cloudflare", "Cloudflare", "Cloud", "Read your zones, Workers and audit logs. /user/tokens/verify checks the token; use bounded audit log dates.", "https://api.cloudflare.com/client/v4/", "user/tokens/verify"),
  api("heroku", "Heroku", "Cloud", "Read your apps, releases and builds. /account identifies you; read recent releases per app.", "https://api.heroku.com/", "account", { ...bearer, Accept: "application/vnd.heroku+json; version=3" }),
  api("digitalocean", "DigitalOcean", "Cloud", "Read your droplets, apps and account actions. /account identifies you; read recent actions.", "https://api.digitalocean.com/v2/", "account"),
  api("render", "Render", "Cloud", "Read your services and deploys. /owners lists your workspaces; read recent deploys per service.", "https://api.render.com/v1/", "owners?limit=1"),
  graphql("railway", "Railway", "Cloud", "Read your projects and deployments through the public GraphQL API. Use bounded recent deployments.", "https://backboard.railway.com/graphql/v2", "{ me { id name } }"),
  api("supabase", "Supabase", "Cloud", "Read your projects and their status through the Management API. /projects lists them.", "https://api.supabase.com/v1/", "projects"),
  api("neon", "Neon", "Cloud", "Read your projects, branches and operations. /users/me identifies you; read recent operations per project.", "https://console.neon.tech/api/v2/", "users/me"),
  api("huggingface", "Hugging Face", "Cloud", "Read your models, datasets, Spaces and likes. /whoami-v2 identifies you; list your repos sorted by lastModified.", "https://huggingface.co/api/", "whoami-v2"),
  mcp("huggingface-mcp", "Hugging Face MCP", "Cloud", "Read your Hub activity through Hugging Face's remote MCP server. Use read tools only.", "https://huggingface.co/mcp"),
  api("replicate", "Replicate", "Cloud", "Read your predictions and trainings. /account identifies you; list recent predictions.", "https://api.replicate.com/v1/", "account"),
  api("posthog", "PostHog", "Cloud", "Read your projects, insights and recent events with a personal API key. /users/@me/ identifies you.", "https://us.posthog.com/api/", "users/@me/"),
  api("pagerduty", "PagerDuty", "Cloud", "Read incidents and on-call shifts assigned to you. /users/me identifies you; use bounded since/until.", "https://api.pagerduty.com/", "users/me", { Authorization: "Token token={{key}}", Accept: "application/vnd.pagerduty+json;version=2" }),
  api("betterstack", "Better Stack", "Cloud", "Read your monitors and incidents from Uptime. List incidents with bounded from/to dates.", "https://uptime.betterstack.com/api/v2/", "monitors"),
  cli("doctl", "doctl", "Cloud", "Read through the installed, signed-in doctl CLI. doctl account get identifies you; read recent actions with JSON output.", ["doctl", "account", "get", "--output", "json"]),

  // Money
  api("ynab", "YNAB", "Money", "Read budgets and transactions. /user identifies you; read transactions with since_date and server_knowledge deltas.", "https://api.ynab.com/v1/", "user"),
  api("lunchmoney", "Lunch Money", "Money", "Read your transactions and budgets. /me identifies you; list transactions with start_date and end_date.", "https://dev.lunchmoney.app/v1/", "me"),
  api("splitwise", "Splitwise", "Money", "Read shared expenses and groups. get_current_user identifies you; list expenses with dated_after.", "https://secure.splitwise.com/api/v3.0/", "get_current_user"),
  api("monzo", "Monzo", "Money", "Read accounts and transactions. ping/whoami checks the token; list transactions with since and before.", "https://api.monzo.com/", "ping/whoami"),
  api("up", "Up", "Money", "Read accounts and transactions. util/ping checks the token; list transactions with filter[since].", "https://api.up.com.au/api/v1/", "util/ping"),
  api("stripe", "Stripe", "Money", "Read payments, payouts and balance activity with a restricted read-only key. /account identifies the account; use created[gte] bounds.", "https://api.stripe.com/v1/", "account", bearer, { requirements: "A restricted, read-only Stripe key." }),
  mcp("stripe-mcp", "Stripe MCP", "Money", "Read your Stripe account through Stripe's remote MCP server with a restricted key. Use read tools only.", "https://mcp.stripe.com", "A restricted, read-only Stripe key."),
  api("wise", "Wise", "Money", "Read your profiles, balances and transfers. /v1/me identifies you; list transfers with createdDateStart.", "https://api.transferwise.com/", "v1/me"),

  // Work
  api("hubspot", "HubSpot", "Work", "Read contacts, deals and engagements with a private app token. Use search with lastmodifieddate filters.", "https://api.hubapi.com/", "account-info/v3/details"),
  api("attio", "Attio", "Work", "Read records, notes and tasks. /self identifies the token; query records sorted by updated time.", "https://api.attio.com/v2/", "self"),
  api("intercom", "Intercom", "Work", "Read conversations you take part in. /me identifies you; search conversations by updated_at.", "https://api.intercom.io/", "me"),
  api("front", "Front", "Work", "Read conversations and comments. /me identifies you; list conversations sorted by date.", "https://api2.frontapp.com/", "me"),
  graphql("monday", "monday.com", "Work", "Read boards, items and updates through GraphQL. me identifies you; read recent updates.", "https://api.monday.com/v2", "{ me { id name } }", { Authorization: "{{key}}" }),
  api("shortcut", "Shortcut", "Work", "Read stories and iterations assigned to you. /member identifies you; search stories by updated date.", "https://api.app.shortcut.com/api/v3/", "member", { "Shortcut-Token": "{{key}}" }),
  api("coda", "Coda", "Work", "Read docs and table rows you can access. /whoami identifies you; list docs and rows sorted by update time.", "https://coda.io/apis/v1/", "whoami"),
  api("typeform", "Typeform", "Work", "Read your forms and responses. /me identifies you; read responses with since and until.", "https://api.typeform.com/", "me"),
  graphql("fireflies", "Fireflies", "Work", "Read your meeting transcripts and summaries through GraphQL. user identifies you; list transcripts with bounded dates.", "https://api.fireflies.ai/graphql", "{ user { user_id name } }"),

  // Calendar
  api("calendly", "Calendly", "Calendar", "Read your scheduled events. /users/me gives your user URI; list scheduled_events with min_start_time and max_start_time.", "https://api.calendly.com/", "users/me"),
  api("savvycal", "SavvyCal", "Calendar", "Read your scheduled events. /me identifies you; list events in a bounded period.", "https://api.savvycal.com/v1/", "me"),

  // Design
  api("figma", "Figma", "Design", "Read files and comments you can access. /me identifies you; read recent files per project and comments by created_at.", "https://api.figma.com/v1/", "me", { "X-Figma-Token": "{{key}}" }),
  api("miro", "Miro", "Design", "Read boards you can access, sorted by last modified.", "https://api.miro.com/v2/", "boards?limit=1"),
  api("webflow", "Webflow", "Design", "Read sites and CMS items. token/authorized_by identifies you; read items by lastUpdated.", "https://api.webflow.com/v2/", "token/authorized_by"),
  api("arena", "Are.na", "Design", "Read your channels and blocks. /me identifies you; read recently updated channels.", "https://api.are.na/v2/", "me"),

  // Notes and reading
  api("zotero", "Zotero", "Reading", "Read your library items and annotations. keys/current identifies the key and user ID; use since with library versions.", "https://api.zotero.org/", "keys/current", { "Zotero-API-Key": "{{key}}" }),
  graphql("hardcover", "Hardcover", "Reading", "Read books you are reading and have finished through GraphQL. me identifies you; read user_books by updated_at.", "https://api.hardcover.app/v1/graphql", "{ me { id username } }"),
  api("capacities", "Capacities", "Notes", "Read your spaces and recently changed objects. /spaces lists them.", "https://api.capacities.io/", "spaces"),

  // Media
  api("discogs", "Discogs", "Music", "Read your collection and wantlist. oauth/identity gives your username; read collection releases by date added.", "https://api.discogs.com/", "oauth/identity", { Authorization: "Discogs token={{key}}" }),
  api("plex", "Plex", "Media", "Read your watch history. /user identifies you; read history from your server with bounded viewedAt.", "https://plex.tv/api/v2/", "user", { "X-Plex-Token": "{{key}}", Accept: "application/json" }),
  api("lichess", "Lichess", "Media", "Read your games and puzzle activity. /account identifies you; export your games with since and until.", "https://lichess.org/api/", "account"),

  // Activity
  api("exist", "Exist", "Activity", "Read your daily attributes such as mood, steps and productive time. accounts/profile/ identifies you; read attributes with date bounds.", "https://exist.io/api/2/", "accounts/profile/"),
  api("timing", "Timing", "Activity", "Read your time entries and app usage from Timing. List time-entries with start_date_min and start_date_max.", "https://web.timingapp.com/api/v1/", "projects"),
  api("pushbullet", "Pushbullet", "Activity", "Read your pushes and mirrored notifications. users/me identifies you; list pushes with modified_after.", "https://api.pushbullet.com/v2/", "users/me", { "Access-Token": "{{key}}" }),
  cli("taskwarrior", "Taskwarrior", "Tasks", "Read tasks through the installed task CLI. task export prints JSON with entry, modified and end times; filter by modified.after.", ["task", "export"], "Taskwarrior"),
  cli("timewarrior", "Timewarrior", "Activity", "Read tracked time through the installed timew CLI. timew export prints JSON intervals with start and end; bound it with a date range.", ["timew", "export"], "Timewarrior"),
  cli("jrnl", "jrnl", "Notes", "Read journal entries through the installed jrnl CLI. --format json prints dated entries; bound reads with -from.", ["jrnl", "-n", "1", "--format", "json"]),
  cli("himalaya", "Himalaya mail", "Mail", "Read email envelopes through the installed, configured himalaya CLI. envelope list prints JSON with dates and subjects.", ["himalaya", "envelope", "list", "--output", "json"], "himalaya"),
]

export { extraCatalog }
