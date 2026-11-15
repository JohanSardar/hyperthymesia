import { afterAll, beforeAll, expect, test } from "bun:test"

// UI regression proofs against the production bundle, with scratch API fixtures.
// Prerequisites: bun run build; agent-browser installed.
const session = "review-56da1321c3f7"
let server: ReturnType<typeof Bun.serve>
async function browser(...args: string[]) {
  const p = Bun.spawn(["agent-browser", "--session", session, ...args], { stdout: "pipe", stderr: "pipe" })
  const [out, err] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text()])
  if (await p.exited) throw new Error(out + err)
  return out.trim()
}
beforeAll(() => {
  server = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(req) {
    const path = new URL(req.url).pathname
    if (path === "/v1/settings") return Response.json({
      settings: { model: { provider: "codex", model: "fixture", effort: null }, mac_recording: true, chunk_retention_days: 90, appearance: "dark" },
      gateway_key_configured: false, storage: { state_dir: "/scratch", published_blob_bytes: 0, automatic_gc: true }, api_keys: [],
    })
    if (path.startsWith("/v1/")) return Response.json({})
    const file = Bun.file(new URL(`./dist${path.startsWith("/app/assets/") ? path.slice(4) : "/index.html"}`, import.meta.url))
    return new Response(file)
  } })
})
afterAll(async () => { await browser("close"); server.stop(true) })
async function kit() {
  await browser("open", `http://127.0.0.1:${server.port}/app/kit`)
  await browser("wait", "--text", "Every shared component in each of its states")
}

test("S1: the shared spinner border follows the border token", async () => {
  await kit()
  const actual = await browser("eval", `(() => {
    document.documentElement.style.setProperty('--border-width-strong', '6px');
    return getComputedStyle(document.querySelector('.kit-surface .spinner')).borderTopWidth;
  })()`)
  expect(actual).toBe('"6px"')
}, 30000)

test("S2: the kit demonstrates disabled radio states in both themes", async () => {
  await kit()
  const actual = await browser("eval", `[...document.querySelectorAll('.kit-surface')]
    .filter(s => s.querySelector('[role="radiogroup"][aria-label="Radio"]'))
    .map(s => s.querySelectorAll('[role="radiogroup"][aria-label="Radio"] .radio[data-disabled]').length)`)
  expect(JSON.parse(actual).map((count: number) => count > 0)).toEqual([true, true])
}, 30000)

test("S3: a new navigation after going back discards Forward", async () => {
  await kit()
  for (const name of ["Timeline", "Search", "Devices"]) {
    await browser("click", `nav.rail a[aria-label="${name}"]`)
    await browser("wait", "--fn", `location.pathname === '/app/${name.toLowerCase()}'`)
  }
  await browser("click", 'button[aria-label="Back"]')
  await browser("wait", "--fn", "location.pathname === '/app/search'")
  await browser("click", 'button[aria-label="Back"]')
  await browser("wait", "--fn", "location.pathname === '/app/timeline'")
  await browser("click", 'nav.rail a[aria-label="Devices"]')
  await browser("wait", "--fn", "location.pathname === '/app/devices'")
  const actual = await browser("eval", `document.querySelector('button[aria-label="Forward"]').disabled`)
  expect(actual).toBe("true")
}, 60000)
