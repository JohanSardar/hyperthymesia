import { Network, Sparkle, SquareTerminal, type LucideIcon } from "lucide-react"
import type { ModelChoice, ProviderId } from "../server/api"
import { Tile } from "../ui"

type Provider = {
  id: ProviderId
  name: string
  host: string
  kind: "key" | "signin"
  blurb: string
  icon: LucideIcon
  help: string
  account?: string
}

// Only what fx can use. The server's /v1/models list is the source of truth;
// this adds the words and glyphs for each.
const providers: Provider[] = [
  {
    id: "codex",
    name: "Codex",
    host: "openai.com",
    kind: "signin",
    blurb: "Your ChatGPT plan, signed in through fx on this Mac",
    icon: SquareTerminal,
    help: "https://developers.openai.com/codex",
    account: "ChatGPT",
  },
  {
    id: "grok",
    name: "Grok",
    host: "x.ai",
    kind: "signin",
    blurb: "Your SuperGrok plan, signed in through fx on this Mac",
    icon: Sparkle,
    help: "https://x.ai/grok",
    account: "Grok",
  },
  {
    id: "vercel",
    name: "Vercel AI Gateway",
    host: "ai-gateway.vercel.sh",
    kind: "key",
    blurb: "One key that reaches models from almost every lab",
    icon: Network,
    help: "https://vercel.com/docs/ai-gateway",
  },
]

function provider(id: ProviderId) {
  return providers.find((p) => p.id === id)!
}

function ProviderTile({ id, size = 36 }: { id: ProviderId; size?: 24 | 36 | 40 | 44 }) {
  const Icon = provider(id).icon
  return (
    <Tile size={size}>
      <span className="tile-glyph">
        <Icon size={size >= 36 ? 18 : 13} strokeWidth={1.75} aria-hidden />
      </span>
    </Tile>
  )
}

function modelLabel(choice: ModelChoice) {
  return [provider(choice.provider).name, choice.model, choice.effort && `${choice.effort} effort`].filter(Boolean).join(" · ")
}

export { modelLabel, provider, providers, ProviderTile }
export type { Provider }
