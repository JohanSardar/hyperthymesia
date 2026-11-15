import {
  Bell,
  CalendarDays,
  CircleAlert,
  Ellipsis,
  Info,
  MonitorSmartphone,
  Plus,
  RefreshCw,
  Search,
  Server,
  ShieldCheck,
  Trash2,
  TriangleAlert,
} from "lucide-react"
import { useState, type CSSProperties, type ReactNode } from "react"
import "./kit.css"
import { PageHeader, useAppearance } from "../shell"
import type { Theme } from "../theme"
import {
  Badge,
  Button,
  Callout,
  Card,
  CardHeader,
  Checkbox,
  Chip,
  Chips,
  ChoiceCard,
  Confirm,
  ConfirmTrigger,
  Dialog,
  DialogClose,
  DialogTrigger,
  Disclosure,
  Empty,
  Field,
  IconButton,
  Input,
  Kbd,
  ListRow,
  Menu,
  MenuContent,
  MenuItem,
  MenuSeparator,
  MenuTrigger,
  Page,
  PageIntro,
  Progress,
  Radio,
  RadioGroup,
  Row,
  SearchInput,
  SecretInput,
  Section,
  Segmented,
  Select,
  Sheet,
  Skeleton,
  Spinner,
  Stat,
  StatusDot,
  Steps,
  Switch,
  Textarea,
  Tile,
  buttonAttrs,
  toast,
} from "../ui"

type View = "both" | "dark" | "light"

/** Draws its children once per theme in view, labelled, side by side when there is room. */
function Specimen({ view, children }: { view: View; children: ReactNode }) {
  const themes: ("dark" | "light")[] = view === "both" ? ["dark", "light"] : [view]
  return (
    <div className="kit-specimen" data-count={themes.length}>
      {themes.map((theme) => (
        <div key={theme} className="theme-surface kit-surface" data-theme={theme}>
          <span className="kit-surface-label">{theme === "dark" ? "Dark" : "Light"}</span>
          {children}
        </div>
      ))}
    </div>
  )
}

function Label({ children }: { children: ReactNode }) {
  return <span className="kit-label">{children}</span>
}

function Cell({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="kit-cell">
      {children}
      <Label>{label}</Label>
    </div>
  )
}

const colorTokens = [
  ["chrome", "Rail and sidebar"],
  ["bg", "Page"],
  ["card", "Card"],
  ["popover", "Popover"],
  ["fg", "Text"],
  ["muted-1", "Secondary text"],
  ["muted-2", "Helper text"],
  ["muted-3", "Faint text"],
  ["border-1", "Hairline"],
  ["border-3", "Strong border"],
  ["solid", "Strong action"],
  ["primary", "Accent"],
  ["success", "Success"],
  ["warning", "Warning"],
  ["destructive", "Danger"],
] as const

const typeTokens = [
  ["display", "Display"],
  ["2xl", "Page title"],
  ["xl", "Heading"],
  ["lg", "Lead"],
  ["base", "Body"],
  ["sm", "Interface"],
  ["xs", "Helper"],
  ["2xs", "Caption"],
] as const

const spaceTokens = ["1", "2", "3", "4", "6", "8", "12", "16"] as const
const radiusTokens = ["sm", "md", "card", "panel", "full"] as const

function Tokens() {
  return (
    <div className="kit-stack">
      <div className="kit-swatches">
        {colorTokens.map(([name, label]) => (
          <div key={name} className="kit-swatch">
            <span className="kit-swatch-chip" style={{ background: `var(--${name})` }} />
            <span className="kit-swatch-text">
              <span>{label}</span>
              <code>--{name}</code>
            </span>
          </div>
        ))}
      </div>
      <div className="kit-type">
        {typeTokens.map(([name, label]) => (
          <div key={name} className="kit-type-row">
            <code>--text-{name}</code>
            <span style={{ fontSize: `var(--text-${name})`, lineHeight: `var(--leading-${name})`, letterSpacing: `var(--tracking-${name})` }}>{label}: what you saw at 10:42</span>
          </div>
        ))}
      </div>
      <div className="kit-scale">
        {spaceTokens.map((name) => (
          <div key={name} className="kit-scale-row">
            <code>--space-{name}</code>
            <span className="kit-scale-bar" style={{ width: `var(--space-${name})` }} />
          </div>
        ))}
      </div>
      <div className="kit-row">
        {radiusTokens.map((name) => (
          <Cell key={name} label={`--radius-${name}`}>
            <span className="kit-radius" style={{ borderRadius: `var(--radius-${name})` }} />
          </Cell>
        ))}
      </div>
    </div>
  )
}

const variants = ["primary", "default", "soft", "ghost", "accent", "destructive", "link"] as const
const forced = [
  ["rest", undefined],
  ["hover", "hover"],
  ["focus", "focus"],
  ["pressed", "active"],
] as const

function Buttons() {
  return (
    <div className="kit-stack">
      <div className="kit-table" style={{ "--cols": forced.length + 2 } as CSSProperties}>
        <span />
        {forced.map(([label]) => (
          <Label key={label}>{label}</Label>
        ))}
        <Label>disabled</Label>
        <Label>pending</Label>
        {variants.map((variant) => (
          <div key={variant} className="kit-table-row">
            <Label>{variant}</Label>
            {forced.map(([label, force]) => (
              <span key={label}>
                <button type="button" {...buttonAttrs(variant, "sm")} data-force={force}>
                  {variant === "link" ? "Open docs" : "Save"}
                </button>
              </span>
            ))}
            <span>
              <button type="button" {...buttonAttrs(variant, "sm")} disabled data-disabled="">
                Save
              </button>
            </span>
            <span>
              <button type="button" {...buttonAttrs(variant, "sm")} data-pending="" aria-busy>
                Save
              </button>
            </span>
          </div>
        ))}
      </div>
      <div className="kit-row">
        <Button size="xs">Extra small</Button>
        <Button size="sm">Small</Button>
        <Button size="md">Medium</Button>
        <Button size="lg" variant="primary">
          Large
        </Button>
        <IconButton label="Refresh" variant="default">
          <RefreshCw size={14} aria-hidden />
        </IconButton>
        <IconButton label="More">
          <Ellipsis size={14} aria-hidden />
        </IconButton>
        <Button onClick={() => new Promise((resolve) => setTimeout(resolve, 1500))}>Press to see pending</Button>
      </div>
    </div>
  )
}

function Inputs() {
  return (
    <div className="kit-stack">
      <div className="kit-grid-3">
        <Field label="Rest">
          <Input placeholder="Deploy workflow" />
        </Field>
        <Field label="Hover">
          <Input placeholder="Deploy workflow" data-force="hover" />
        </Field>
        <Field label="Focus">
          <Input defaultValue="Deploy workflow" data-force="focus" />
        </Field>
        <Field label="Invalid" error="That key was rejected">
          <Input defaultValue="sk-wrong" mono />
        </Field>
        <Field label="Disabled" description="Set by the server">
          <Input defaultValue="127.0.0.1:7377" disabled mono />
        </Field>
        <Field label="Secret">
          <SecretInput placeholder="sk-…" />
        </Field>
      </div>
      <div className="kit-grid-3">
        <Field label="Search">
          <SearchInput placeholder="Search providers" />
        </Field>
        <Field label="Small search">
          <SearchInput size="sm" placeholder="Filter" />
        </Field>
        <Field label="Select">
          <Select
            aria-label="Keep entries for"
            defaultValue="90"
            width="full"
            items={[
              { value: "30", label: "30 days" },
              { value: "90", label: "90 days" },
              { value: "365", label: "1 year" },
            ]}
          />
        </Field>
        <Field label="Select, empty">
          <Select aria-label="Model" placeholder="Choose a model" width="full" items={[{ value: "a", label: "Model A" }]} />
        </Field>
        <Field label="Select, disabled">
          <Select aria-label="Region" defaultValue="eu" disabled width="full" items={[{ value: "eu", label: "Europe" }]} />
        </Field>
      </div>
      <Field label="Textarea" description="Monospace, for configuration">
        <Textarea rows={3} placeholder="Paste an mcpServers JSON, a URL, or a command line" />
      </Field>
    </div>
  )
}

function Toggles() {
  const [chips, setChips] = useState({ mac: true, mail: false })
  const [choice, setChoice] = useState<string | null>("key")
  return (
    <div className="kit-stack">
      <div className="kit-row">
        <Cell label="off">
          <Switch aria-label="Off" />
        </Cell>
        <Cell label="on">
          <Switch aria-label="On" defaultChecked />
        </Cell>
        <Cell label="focus">
          <Switch aria-label="Focus" defaultChecked data-force="focus" />
        </Cell>
        <Cell label="disabled">
          <Switch aria-label="Disabled off" disabled />
        </Cell>
        <Cell label="disabled on">
          <Switch aria-label="Disabled on" disabled defaultChecked />
        </Cell>
        <span className="kit-divider" />
        <Cell label="unchecked">
          <Checkbox aria-label="Unchecked" />
        </Cell>
        <Cell label="checked">
          <Checkbox aria-label="Checked" defaultChecked />
        </Cell>
        <Cell label="mixed">
          <Checkbox aria-label="Mixed" defaultChecked="indeterminate" />
        </Cell>
        <Cell label="disabled">
          <Checkbox aria-label="Disabled" disabled defaultChecked />
        </Cell>
        <span className="kit-divider" />
        <RadioGroup aria-label="Radio" defaultValue="b" className="kit-row">
          <Cell label="off">
            <Radio value="a" />
          </Cell>
          <Cell label="on">
            <Radio value="b" />
          </Cell>
        </RadioGroup>
      </div>
      <div className="kit-row">
        <Segmented
          aria-label="Range"
          defaultValue="day"
          items={[
            { value: "day", label: "Day" },
            { value: "week", label: "Week" },
            { value: "month", label: "Month" },
          ]}
        />
        <Chips label="Sources">
          <Chip pressed={chips.mac} onPressedChange={(mac) => setChips((c) => ({ ...c, mac }))} count={1284}>
            This Mac
          </Chip>
          <Chip pressed={chips.mail} onPressedChange={(mail) => setChips((c) => ({ ...c, mail }))} count={36}>
            Mail
          </Chip>
          <Chip disabled>Calendar</Chip>
        </Chips>
      </div>
      <RadioGroup aria-label="Authentication" value={choice} onValueChange={setChoice} className="choice-stack">
        <ChoiceCard value="signin" title="Sign in" description="Use the server's own login" />
        <ChoiceCard value="key" title="API key" description="Send a key in a request header" />
      </RadioGroup>
    </div>
  )
}

function Rows() {
  return (
    <div className="kit-stack">
      <Card>
        <Row label="Record screen text" description="Reads the text of the window in front every few seconds" control={<Switch aria-label="Record screen text" defaultChecked />} />
        <Row
          label="Keep entries for"
          description="Older entries are removed on the next pass"
          control={<Select aria-label="Keep entries for" defaultValue="90" width={144} items={[{ value: "90", label: "90 days" }]} />}
        />
        <Row label="Data folder" description="Where entries are kept on this Mac" onClick={() => toast("Opened data folder")} />
      </Card>
      <Card>
        <CardHeader
          title="2 connections"
          actions={
            <Menu>
              <MenuTrigger aria-label="Add" {...buttonAttrs("default", "sm", undefined, true)}>
                <Plus size={14} aria-hidden />
              </MenuTrigger>
              <MenuContent align="end">
                <MenuItem value="catalog" icon={<Server size={14} />}>
                  From the catalog
                </MenuItem>
                <MenuSeparator />
                <MenuItem value="api" icon={<Server size={14} />}>
                  An API
                </MenuItem>
              </MenuContent>
            </Menu>
          }
        />
        <Row
          leading={<Tile size={36}>M</Tile>}
          label="This Mac"
          description="Screen text, apps and windows"
          control={
            <Badge tone="success">
              <StatusDot tone="live" /> Live
            </Badge>
          }
        />
        <Disclosure title="Advanced" description="Closed until opened">
          <p className="row-description">Nothing else to set here.</p>
        </Disclosure>
        <Disclosure title="Advanced, open" description="The panel shows its content" defaultOpen>
          <p className="row-description">An open disclosure.</p>
        </Disclosure>
      </Card>
      <div className="kit-grid-2">
        <ListRow leading={<Tile>G</Tile>} title="Rest" description="A list row" onClick={() => {}} />
        <ListRow leading={<Tile>N</Tile>} title="Selected" description="The chosen row" selected onClick={() => {}} />
        <ListRow leading={<Tile size={24}>S</Tile>} title="Small tile" description="With a trailing value" trailing="12m" />
        <ListRow leading={<Tile size={44}>L</Tile>} title="Large tile" description="Not interactive" />
      </div>
    </div>
  )
}

function Status() {
  return (
    <div className="kit-stack">
      <div className="kit-row">
        <Badge>Neutral</Badge>
        <Badge tone="primary">New</Badge>
        <Badge tone="success">Live</Badge>
        <Badge tone="warning">Retrying</Badge>
        <Badge tone="danger">Failed</Badge>
        <span className="kit-divider" />
        {(["idle", "live", "ok", "warning", "error"] as const).map((tone) => (
          <Cell key={tone} label={tone}>
            <StatusDot tone={tone} />
          </Cell>
        ))}
        <span className="kit-divider" />
        <Cell label="spinner">
          <Spinner />
        </Cell>
        <Cell label="skeleton">
          <Skeleton width={96} />
        </Cell>
      </div>
      <div className="kit-grid-3">
        <Stat label="Entries today" value="1,284" hint="+212 in the last hour" />
        <Stat label="Active sources" value="3" />
        <Stat label="Last pull" value={<Skeleton width={48} height={22} />} hint="Loading" />
      </div>
      <div className="kit-grid-2">
        <Progress label="Model quota" hint="64% left" value={64} />
        <Progress label="Accent" hint="30%" value={30} tone="primary" />
        <Progress label="Running low" hint="12% left" value={12} tone="warning" />
        <Progress label="Over the limit" hint="0% left" value={100} tone="danger" />
        <Progress label="Working out what to read" />
      </div>
      <div className="kit-stack-tight">
        <Callout icon={<Info size={15} />} title="Passes run every 4 hours">
          The next one starts at 14:00.
        </Callout>
        <Callout tone="primary" icon={<ShieldCheck size={15} />} title="Recording stays on this Mac" actions={<Button size="xs">Learn more</Button>}>
          Screen text is read, never saved as an image.
        </Callout>
        <Callout tone="warning" icon={<TriangleAlert size={15} />} title="Accessibility is off">
          Turn it on in System Settings so the app can read window text.
        </Callout>
        <Callout tone="danger" icon={<CircleAlert size={15} />} title="The server stopped answering" actions={<Button size="xs">Retry</Button>}>
          Nothing new arrives until it's back.
        </Callout>
      </div>
      <Steps
        items={[
          { title: "Open System Settings", description: "Privacy & Security, then Accessibility", state: "done" },
          { title: "Turn on Hyperthymesia", description: "This page notices the moment it's on", state: "current" },
          { title: "Come back here", state: "upcoming" },
        ]}
      />
    </div>
  )
}

function Empties() {
  return (
    <div className="kit-grid-2">
      <Card>
        <Empty icon={<CalendarDays size={22} strokeWidth={1.5} />} title="Nothing recorded yet" description="Keep this Mac on and today fills in as you work." />
      </Card>
      <Card>
        <Empty icon={<MonitorSmartphone size={22} strokeWidth={1.5} />} title="No other devices yet" description="Pair another computer with a one-time link.">
          <Button variant="primary">Pair a computer</Button>
        </Empty>
      </Card>
      <Card>
        <Empty title="No runs yet" description="Passes run every 4 hours." />
      </Card>
      <Card>
        <p className="sidebar-empty">No saved searches yet</p>
      </Card>
    </div>
  )
}

function Navigation() {
  return (
    <div className="kit-stack">
      <div className="kit-row kit-row-top">
        <div className="kit-rail">
          <Cell label="rest">
            <span className="rail-item">
              <Search size={17} strokeWidth={1.6} aria-hidden />
            </span>
          </Cell>
          <Cell label="hover">
            <span className="rail-item" data-force="hover">
              <Bell size={17} strokeWidth={1.6} aria-hidden />
            </span>
          </Cell>
          <Cell label="current">
            <span className="rail-item" aria-current="page">
              <CalendarDays size={17} strokeWidth={1.6} aria-hidden />
            </span>
          </Cell>
        </div>
        <div className="kit-sidebar">
          <div className="sidebar-group">
            <div className="sidebar-group-head">
              <span className="sidebar-group-label">Sidebar rows</span>
            </div>
            <span className="sidebar-row">
              <Search size={15} strokeWidth={1.6} aria-hidden />
              <span className="sidebar-row-label">Rest</span>
              <span className="sidebar-row-meta">12</span>
            </span>
            <span className="sidebar-row" data-force="hover">
              <Bell size={15} strokeWidth={1.6} aria-hidden />
              <span className="sidebar-row-label">Hover</span>
            </span>
            <span className="sidebar-row" data-selected="">
              <CalendarDays size={15} strokeWidth={1.6} aria-hidden />
              <span className="sidebar-row-label">Current</span>
              <span className="sidebar-row-meta">⌘1</span>
            </span>
          </div>
        </div>
        <div className="kit-stack-tight">
          <div className="kit-row">
            <Kbd>⌘K</Kbd>
            <Kbd>⌘B</Kbd>
            <Kbd>esc</Kbd>
          </div>
          <div className="tooltip kit-static">
            Hide sidebar <Kbd>⌘B</Kbd>
          </div>
        </div>
      </div>
      <div className="kit-header">
        <header className="page-header">
          <div className="page-header-start">
            <nav className="page-header-title" aria-label="Breadcrumb">
              <span className="crumb">
                <a href="#kit">Connections</a>
              </span>
              <span className="crumb">
                <span className="crumb-sep" aria-hidden>
                  /
                </span>
                <span>Calendar</span>
              </span>
            </nav>
          </div>
          <div className="page-header-actions">
            <button type="button" {...buttonAttrs("ghost", "sm", undefined, true)} aria-label="Refresh">
              <RefreshCw size={14} aria-hidden />
            </button>
            <button type="button" {...buttonAttrs("primary", "sm")}>
              Pull now
            </button>
          </div>
        </header>
      </div>
    </div>
  )
}

/** Popups drawn in place, open, so both themes show them at once. */
function OpenStates() {
  return (
    <div className="kit-row kit-row-top">
      <div className="menu kit-static">
        <div className="menu-group-label">Menu</div>
        <div className="menu-item">
          <span className="menu-item-icon">
            <RefreshCw size={14} />
          </span>
          <span className="menu-item-label">Pull now</span>
        </div>
        <div className="menu-item" data-highlighted="">
          <span className="menu-item-icon">
            <Server size={14} />
          </span>
          <span className="menu-item-label">Highlighted</span>
        </div>
        <div className="menu-item" data-disabled="">
          <span className="menu-item-icon">
            <Bell size={14} />
          </span>
          <span className="menu-item-label">Disabled</span>
        </div>
        <div className="menu-separator" />
        <div className="menu-item">
          <span className="menu-item-icon">
            <Trash2 size={14} />
          </span>
          <span className="menu-item-label">Remove</span>
        </div>
      </div>
      <div className="kit-stack-tight kit-toasts">
        {(
          [
            [undefined, "Saved", "Applies on the next pass"],
            ["success", "Calendar connected", undefined],
            ["warning", "Pull is slow", "Still trying"],
            ["error", "Couldn't save", "The server didn't answer"],
            ["loading", "Pulling Calendar", undefined],
          ] as const
        ).map(([type, title, description]) => (
          <div key={title} className="toast kit-static" data-type={type}>
            <span className="toast-mark" aria-hidden />
            <div className="toast-text">
              <span className="toast-title">{title}</span>
              {description && <span className="toast-description">{description}</span>}
            </div>
            {type === "error" && (
              <button type="button" {...buttonAttrs("default", "xs")}>
                Retry
              </button>
            )}
          </div>
        ))}
      </div>
      <div className="dialog kit-static" data-size="sm">
        <header className="dialog-header">
          <div className="dialog-heading">
            <span className="dialog-title">Remove Calendar?</span>
            <span className="dialog-description">Its entries stay. New events stop arriving.</span>
          </div>
        </header>
        <footer className="dialog-footer">
          <button type="button" {...buttonAttrs("default", "sm")}>
            Cancel
          </button>
          <button type="button" {...buttonAttrs("destructive", "sm")}>
            Remove
          </button>
        </footer>
      </div>
      <div className="palette kit-static">
        <div className="palette-search">
          <Search aria-hidden size={15} className="palette-search-icon" />
          <span className="palette-input kit-palette-query">tim</span>
          <Kbd>esc</Kbd>
        </div>
        <div className="palette-list">
          <div className="palette-group-label">Go to</div>
          <div className="palette-item" data-highlighted="">
            <span className="palette-item-icon">
              <CalendarDays size={15} strokeWidth={1.75} />
            </span>
            <span className="palette-item-label">
              <mark>Tim</mark>eline
            </span>
            <Kbd>⌘2</Kbd>
          </div>
          <div className="palette-item">
            <span className="palette-item-icon">
              <Search size={15} strokeWidth={1.75} />
            </span>
            <span className="palette-item-label">Search</span>
            <Kbd>⌘3</Kbd>
          </div>
        </div>
      </div>
    </div>
  )
}

function LiveOverlays() {
  return (
    <div className="kit-row">
      <Dialog
        title="Create API key"
        description="Programs use this key to read your entries"
        trigger={<DialogTrigger {...buttonAttrs("default", "sm")}>Dialog</DialogTrigger>}
        size="sm"
        footer={
          <>
            <DialogClose {...buttonAttrs("default", "sm")}>Cancel</DialogClose>
            <DialogClose {...buttonAttrs("primary", "sm")}>Create key</DialogClose>
          </>
        }
      >
        <Field label="Name">
          <Input placeholder="Deploy workflow" />
        </Field>
      </Dialog>
      <Sheet
        title="Add a connection"
        trigger={<DialogTrigger {...buttonAttrs("default", "sm")}>Sheet</DialogTrigger>}
        footer={
          <>
            <DialogClose {...buttonAttrs("default", "sm")}>Cancel</DialogClose>
            <Button variant="primary" disabled>
              Connect
            </Button>
          </>
        }
      >
        <SearchInput placeholder="Search sources" />
        <div className="kit-grid-2">
          {["Mail", "Calendar", "Notes", "Tasks"].map((name) => (
            <ListRow key={name} leading={<Tile size={36}>{name[0]}</Tile>} title={name} description="A connection" onClick={() => {}} />
          ))}
        </div>
      </Sheet>
      <Confirm
        title="Remove Calendar?"
        description="Its entries stay. New events stop arriving."
        confirmLabel="Remove"
        destructive
        trigger={<ConfirmTrigger {...buttonAttrs("destructive", "sm")}>Confirm</ConfirmTrigger>}
        onConfirm={() => toast.success("Calendar removed")}
      />
      <Menu>
        <MenuTrigger {...buttonAttrs("default", "sm")}>Menu</MenuTrigger>
        <MenuContent>
          <MenuItem value="pull" icon={<RefreshCw size={14} />}>
            Pull now
          </MenuItem>
          <MenuItem value="remove" icon={<Trash2 size={14} />}>
            Remove
          </MenuItem>
        </MenuContent>
      </Menu>
      <Button onClick={() => toast("Saved", { description: "Applies on the next pass" })}>Toast</Button>
      <Button onClick={() => toast.success("Calendar connected")}>Success toast</Button>
      <Button onClick={() => toast.error("Couldn't save", { description: "The server didn't answer", action: { label: "Retry", onAction: () => {} } })}>Error toast</Button>
    </div>
  )
}

function KitPage() {
  const { theme, setTheme } = useAppearance()
  const [view, setView] = useState<View>("both")
  const sections: [string, string, ReactNode][] = [
    ["Tokens", "Colour, type, spacing and radius. Nothing in the app uses a raw value.", <Tokens />],
    ["Buttons", "Every variant in every state. The strong action is the solid pill; the accent is rare.", <Buttons />],
    ["Fields", "Rest, hover, focus, invalid and disabled.", <Inputs />],
    ["Toggles and choices", "Switches, checkboxes, radios, segmented controls, chips and choice cards.", <Toggles />],
    ["Rows, cards and lists", "Settings rows carry their explanation inline.", <Rows />],
    ["Status and feedback", "Badges, dots, loading, stats, progress, callouts and steps.", <Status />],
    ["Empty states", "What's missing, and the next step in one sentence.", <Empties />],
    ["Navigation", "Rail, sidebar rows, keys, tooltip and the page header.", <Navigation />],
    ["Open popups", "Menu, toasts, dialog and palette, drawn open.", <OpenStates />],
  ]
  return (
    <>
      <PageHeader
        crumbs={[{ label: "Kit" }]}
        actions={
          <Segmented
            aria-label="Show"
            value={view}
            onValueChange={(value) => value && setView(value as View)}
            items={[
              { value: "both", label: "Both themes" },
              { value: "dark", label: "Dark" },
              { value: "light", label: "Light" },
            ]}
          />
        }
      />
      <Page width="full" className="kit-page">
        <div className="kit-inner">
          <PageIntro
            title="Components"
            description="Every shared component in each of its states. Later pages are built only from these."
            actions={
              <Segmented
                aria-label="App theme"
                value={theme}
                onValueChange={(value) => value && setTheme(value as Theme)}
                items={[
                  { value: "dark", label: "Dark" },
                  { value: "light", label: "Light" },
                  { value: "system", label: "System" },
                ]}
              />
            }
          />
          {sections.map(([title, description, body]) => (
            <Section key={title} title={title} description={description}>
              <Specimen view={view}>{body}</Specimen>
            </Section>
          ))}
          <Section title="Live popups" description="These open for real and follow the app theme, set above.">
            <LiveOverlays />
          </Section>
        </div>
      </Page>
    </>
  )
}

export { KitPage }
