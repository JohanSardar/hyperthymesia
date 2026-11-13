import {
  siApple,
  siBitbucket,
  siBuildkite,
  siCaldotcom,
  siCalendly,
  siCircleci,
  siClickup,
  siClockify,
  siCloudflare,
  siCodeberg,
  siCoda,
  siDigitalocean,
  siDiscogs,
  siDiscord,
  siFigma,
  siFitbit,
  siGithub,
  siGitlab,
  siGmail,
  siGnubash,
  siGooglecalendar,
  siGoogledrive,
  siHubspot,
  siHuggingface,
  siIntercom,
  siJira,
  siLichess,
  siLinear,
  siMiro,
  siMonzo,
  siNeon,
  siNetlify,
  siNotion,
  siPagerduty,
  siPlex,
  siPosthog,
  siPushbullet,
  siRailway,
  siRender,
  siReplicate,
  siSentry,
  siShortcut,
  siSourcehut,
  siSpotify,
  siStrava,
  siStripe,
  siSupabase,
  siTodoist,
  siToggl,
  siTypeform,
  siVercel,
  siWebflow,
  siWise,
  siZotero,
  siAirtable,
  siAsana,
  type SimpleIcon,
} from "simple-icons"
import { Code2, Plug, Server, Terminal } from "lucide-react"
import { cx } from "../ui"

const icons: Record<string, SimpleIcon> = {
  discord: siDiscord,
  linear: siLinear,
  todoist: siTodoist,
  asana: siAsana,
  clickup: siClickup,
  notion: siNotion,
  airtable: siAirtable,
  spotify: siSpotify,
  fitbit: siFitbit,
  strava: siStrava,
  toggl: siToggl,
  clockify: siClockify,
  gitlab: siGitlab,
  sentry: siSentry,
  jira: siJira,
  calcom: siCaldotcom,
  "gmail-gws": siGmail,
  "calendar-gws": siGooglecalendar,
  "drive-gws": siGoogledrive,
  "apple-bridge": siApple,
  "x-github": siGithub,
  "x-github-mcp": siGithub,
  "x-gh": siGithub,
  "x-glab": siGitlab,
  "x-codeberg": siCodeberg,
  "x-bitbucket": siBitbucket,
  "x-sourcehut": siSourcehut,
  "x-circleci": siCircleci,
  "x-buildkite": siBuildkite,
  "x-vercel": siVercel,
  "x-netlify": siNetlify,
  "x-cloudflare": siCloudflare,
  "x-digitalocean": siDigitalocean,
  "x-doctl": siDigitalocean,
  "x-render": siRender,
  "x-railway": siRailway,
  "x-supabase": siSupabase,
  "x-neon": siNeon,
  "x-huggingface": siHuggingface,
  "x-huggingface-mcp": siHuggingface,
  "x-replicate": siReplicate,
  "x-posthog": siPosthog,
  "x-pagerduty": siPagerduty,
  "x-monzo": siMonzo,
  "x-stripe": siStripe,
  "x-stripe-mcp": siStripe,
  "x-wise": siWise,
  "x-hubspot": siHubspot,
  "x-intercom": siIntercom,
  "x-shortcut": siShortcut,
  "x-coda": siCoda,
  "x-typeform": siTypeform,
  "x-calendly": siCalendly,
  "x-figma": siFigma,
  "x-miro": siMiro,
  "x-webflow": siWebflow,
  "x-zotero": siZotero,
  "x-discogs": siDiscogs,
  "x-plex": siPlex,
  "x-lichess": siLichess,
  "x-pushbullet": siPushbullet,
  "x-taskwarrior": siGnubash,
  "x-timewarrior": siGnubash,
  "x-jrnl": siGnubash,
  "x-himalaya": siGnubash,
}

const tints = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)", "var(--chart-4)", "var(--chart-5)"]

function hash(text: string) {
  let h = 0
  for (const ch of text) h = (h * 31 + ch.charCodeAt(0)) | 0
  return Math.abs(h)
}

/** Relative luminance of a hex colour, to pick a light or dark glyph. */
function light(hex: string) {
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.6
}

type LogoProps = {
  id?: string
  name: string
  kind?: "mcp" | "api" | "cli" | null
  size?: 24 | 36 | 40 | 44
  className?: string
}

/** A service's mark on its brand colour, or a tinted monogram when there is no mark. */
function Logo({ id, name, kind, size = 40, className }: LogoProps) {
  const icon = id ? icons[id] : undefined
  const glyph = Math.round(size * 0.5)
  if (icon) {
    const isDark = icon.hex === "000000" || icon.hex === "181717" || icon.hex === "100000"
    const bg = `#${icon.hex}`
    const fg = light(icon.hex) ? "#111" : "#fff"
    return (
      <span className={cx("logo", className)} data-size={size} style={{ background: isDark ? "var(--logo-ink)" : bg, color: isDark ? "var(--logo-ink-fg)" : fg }} aria-hidden>
        <svg viewBox="0 0 24 24" width={glyph} height={glyph} fill="currentColor">
          <path d={icon.path} />
        </svg>
      </span>
    )
  }
  if (!id && kind) {
    const Icon = kind === "mcp" ? Server : kind === "cli" ? Terminal : kind === "api" ? Code2 : Plug
    return (
      <span className={cx("logo", "logo-plain", className)} data-size={size} aria-hidden>
        <Icon size={Math.round(size * 0.42)} strokeWidth={1.75} />
      </span>
    )
  }
  const letter = name.replace(/[^\p{L}\p{N}]/gu, "").charAt(0).toUpperCase() || "?"
  return (
    <span className={cx("logo", "logo-letter", className)} data-size={size} style={{ ["--tint" as string]: tints[hash(name) % tints.length] }} aria-hidden>
      {letter}
    </span>
  )
}

export { Logo }
