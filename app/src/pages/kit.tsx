import { Ellipsis, Plus, RefreshCw, Server, Trash2 } from "lucide-react"
import { useState } from "react"
import "./kit.css"
import { PageHeader } from "../shell"
import { useTheme, type Theme } from "../theme"
import {
  Badge,
  Button,
  Card,
  CardHeader,
  Checkbox,
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
  ListRow,
  Menu,
  MenuContent,
  MenuItem,
  MenuSeparator,
  MenuTrigger,
  Page,
  PageIntro,
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
  Switch,
  Textarea,
  Tile,
  buttonAttrs,
  toast,
} from "../ui"

function KitPage() {
  const [theme, setTheme] = useTheme()
  const [auth, setAuth] = useState<string | null>("none")
  return (
    <>
      <PageHeader
        crumbs={[{ label: "Kit", to: "/kit" }, { label: "Components" }]}
        actions={
          <>
            <IconButton label="Refresh">
              <RefreshCw size={14} aria-hidden />
            </IconButton>
            <IconButton label="New">
              <Plus size={14} aria-hidden />
            </IconButton>
          </>
        }
      />
      <Page>
        <PageIntro title="Components" description="Every building block, in one place, for checking the look in both themes" />

        <Section title="Buttons">
          <div className="kit-row">
            <Button variant="primary">Primary</Button>
            <Button>Default</Button>
            <Button variant="ghost">Ghost</Button>
            <Button variant="soft">Soft</Button>
            <Button variant="link">Docs ↗</Button>
            <Button variant="destructive">Remove</Button>
            <Button disabled>Disabled</Button>
            <Button onClick={() => new Promise((resolve) => setTimeout(resolve, 1500))}>Pending on press</Button>
          </div>
          <div className="kit-row">
            <Button size="xs">Extra small</Button>
            <Button size="sm">Small</Button>
            <Button size="md">Base</Button>
            <Button size="lg" variant="primary">Large</Button>
          </div>
        </Section>

        <Section title="Appearance" description="Settings rows in a card">
          <Card>
            <Row
              label="Theme"
              description="Follow the system, or pick one"
              control={
                <Segmented
                  aria-label="Theme"
                  value={theme}
                  onValueChange={(value) => value && setTheme(value as Theme)}
                  items={[
                    { value: "system", label: "System" },
                    { value: "light", label: "Light" },
                    { value: "dark", label: "Dark" },
                  ]}
                />
              }
            />
            <Row label="Record screen text" description="Read the text of the window in front every few seconds" control={<Switch aria-label="Record screen text" defaultChecked />} />
            <Row label="Pause while locked" control={<Switch aria-label="Pause while locked" />} />
            <Row
              label="Keep entries for"
              description="Older entries are deleted on the next pass"
              control={
                <Select
                  aria-label="Keep entries for"
                  defaultValue="90"
                  width={144}
                  items={[
                    { value: "30", label: "30 days" },
                    { value: "90", label: "90 days" },
                    { value: "365", label: "1 year" },
                    { value: "forever", label: "Forever" },
                  ]}
                />
              }
            />
            <Row label="Data folder" description="Where entries are stored on this Mac" onClick={() => toast("Opened data folder")} />
          </Card>
        </Section>

        <Section title="Lists">
          <Card>
            <CardHeader
              title="2 connections"
              actions={
                <Menu>
                  <MenuTrigger aria-label="Add" {...buttonAttrs("default", "sm", undefined, true)}>
                    <Plus size={14} aria-hidden />
                  </MenuTrigger>
                  <MenuContent align="end">
                    <MenuItem value="catalog" icon={<Server size={14} />}>From catalog</MenuItem>
                    <MenuItem value="mcp" icon={<Server size={14} />}>MCP server</MenuItem>
                    <MenuSeparator />
                    <MenuItem value="api" icon={<Server size={14} />}>API</MenuItem>
                  </MenuContent>
                </Menu>
              }
            />
            <Row
              leading={<Tile size={36}>M</Tile>}
              label="This Mac"
              description="Screen text, apps and windows"
              control={
                <>
                  <Badge tone="success">
                    <StatusDot tone="live" /> Live
                  </Badge>
                  <Switch aria-label="This Mac" defaultChecked />
                </>
              }
            />
            <Row
              leading={<Tile size={36}>C</Tile>}
              label="Calendar"
              description="Last pull 12 minutes ago"
              control={
                <Menu>
                  <MenuTrigger aria-label="More" {...buttonAttrs("ghost", "sm", undefined, true)}>
                    <Ellipsis size={14} aria-hidden />
                  </MenuTrigger>
                  <MenuContent align="end">
                    <MenuItem value="pull" icon={<RefreshCw size={14} />}>Pull now</MenuItem>
                    <MenuItem value="remove" icon={<Trash2 size={14} />}>Remove</MenuItem>
                  </MenuContent>
                </Menu>
              }
            />
          </Card>
          <div className="kit-grid">
            <ListRow leading={<Tile>G</Tile>} title="Gmail" description="Mail you send and receive" onClick={() => {}} />
            <ListRow leading={<Tile>N</Tile>} title="Notion" description="Pages and databases you edit" onClick={() => {}} />
            <ListRow leading={<Tile>S</Tile>} title="Slack" description="Messages in channels you're in" onClick={() => {}} />
            <ListRow leading={<Tile>L</Tile>} title="Linear" description="Issues you create, own or comment on" onClick={() => {}} />
          </div>
        </Section>

        <Section title="Fields">
          <Card className="kit-form">
            <div className="kit-pair">
              <Field label="Name">
                <Input placeholder="DeepWiki" />
              </Field>
              <Field label="Key" description="Lowercase letters, numbers and dashes">
                <Input placeholder="deepwiki" mono />
              </Field>
            </div>
            <Field label="API key" error="That key was rejected">
              <SecretInput placeholder="sk-…" />
            </Field>
            <Field label="Search">
              <SearchInput placeholder="Search providers" />
            </Field>
            <Field label="Import a configuration">
              <Textarea rows={4} placeholder="Paste an mcpServers JSON, a URL, or a command line" />
            </Field>
            <label className="kit-check">
              <Checkbox defaultChecked /> Every model
            </label>
          </Card>
          <RadioGroup aria-label="Authentication" value={auth} onValueChange={setAuth} className="choice-stack">
            <ChoiceCard value="signin" title="Sign in" description="Use the server's own login" />
            <ChoiceCard value="key" title="API key" description="Send a key in a request header" />
            <ChoiceCard value="none" title="No sign-in" description="The server is open" />
          </RadioGroup>
          <Card>
            <Disclosure title="Advanced" description="OAuth client, import a configuration">
              <p className="row-description">Nothing else to set here.</p>
            </Disclosure>
          </Card>
        </Section>

        <Section title="Overlays">
          <div className="kit-row">
            <Dialog
              title="Create API key"
              description="Programs use this key to read your entries through /v1"
              trigger={<DialogTrigger {...buttonAttrs("default", "sm")}>Dialog</DialogTrigger>}
              size="sm"
              footer={
                <>
                  <DialogClose {...buttonAttrs("default", "sm")}>Cancel</DialogClose>
                  <DialogClose {...buttonAttrs("primary", "sm")}>Create API key</DialogClose>
                </>
              }
            >
              <Field label="Name">
                <Input placeholder="Deploy workflow" />
              </Field>
            </Dialog>
            <Sheet
              title="Add a connection"
              initialFocus='input[type="search"]'
              trigger={<DialogTrigger {...buttonAttrs("default", "sm")}>Right sheet</DialogTrigger>}
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
              <div className="kit-grid">
                {["Gmail", "Calendar", "Notion", "Slack", "Linear", "GitHub", "Spotify", "Readwise"].map((name) => (
                  <ListRow key={name} leading={<Tile size={36}>{name[0]}</Tile>} title={name} description={`${name.toLowerCase()}.com`} onClick={() => {}} />
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
            <Button onClick={() => toast("Saved", { description: "Applies on the next pass" })}>Toast</Button>
          </div>
        </Section>

        <Section title="Status">
          <div className="kit-stats">
            <Stat label="Entries today" value="1,284" hint="+212 in the last hour" />
            <Stat label="Active sources" value="3" />
            <Stat label="Last pull" value="4m" hint="Calendar" />
          </div>
          <div className="kit-row">
            <Badge>Not connected</Badge>
            <Badge tone="primary">New</Badge>
            <Badge tone="success">Live</Badge>
            <Badge tone="warning">Retrying</Badge>
            <Badge tone="danger">Failed</Badge>
            <Spinner />
            <Skeleton width={120} />
          </div>
          <Card>
            <Empty title="No runs yet" description="Passes run every 4 hours" />
          </Card>
        </Section>
      </Page>
    </>
  )
}

export { KitPage }
