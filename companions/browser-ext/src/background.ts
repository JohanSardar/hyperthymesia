import { DEFAULT_EXCLUSIONS, excluded, Outbox, visible, visit, event, type CompanionEvent } from "./core";
const store = chrome.storage.local;
const outbox = new Outbox(store, async e => {
  try {
    const ack = await Promise.race([
      chrome.runtime.sendNativeMessage("com.hyperthymesia.agent", e),
      new Promise<null>(resolve => setTimeout(() => resolve(null), 3000)),
    ]);
    return ack?.ok === true;
  } catch { return false; }
});
let chain = Promise.resolve();
function enqueue(work: () => Promise<void>) {
  chain = chain.then(work).catch(() => { /* Retain durable queue for the next alarm. */ });
}
async function patterns(): Promise<string[]> {
  const { excludedPatterns } = await store.get(["excludedPatterns"]);
  return Array.isArray(excludedPatterns) && excludedPatterns.every(x => typeof x === "string") ? [...DEFAULT_EXCLUSIONS, ...excludedPatterns] : DEFAULT_EXCLUSIONS;
}
interface State { current?: CompanionEvent; page: boolean; url?: string }
async function update(id: number, navigation = false, page?: boolean, expectedUrl?: string) {
  const tab = await chrome.tabs.get(id).catch(() => null);
  if (!tab || tab.incognito || !tab.url) return;
  const rules = await patterns();
  const { visits = {} } = await store.get(["visits"]);
  const s: State = visits[id] ?? {page: false};
  const now = new Date().toISOString();
  if (expectedUrl && expectedUrl !== tab.url) return;
  const pending: CompanionEvent[] = [];
  if (excluded(tab.url, rules)) {
    if (s.current && !excluded(String(s.current.payload.url), rules)) { s.current.ended_at = now; s.current.revision = now; pending.push(s.current); }
    // Excluded URL/title/text must never enter durable state.
    delete visits[id]; await outbox.addAll(pending, {visits}); return;
  }
  const routeChanged = navigation || s.url !== tab.url;
  if (routeChanged) s.page = false;
  if (page !== undefined) s.page = page;
  const focused = await chrome.windows.getLastFocused().catch(() => null);
  const idle = await chrome.idle.queryState(60);
  const isVisible = visible(!!tab.active, tab.windowId, focused?.focused ? focused.id! : -1, s.page, idle);
  if (s.current && !routeChanged && s.current.payload.visible === isVisible && s.current.payload.title === tab.title) {
    visits[id] = s; await store.set({visits}); return;
  }
  if (s.current) { s.current.ended_at = now; s.current.revision = now; pending.push(s.current); }
  // Persist the counter even after tab deletion so reused IDs do not collide.
  const { navCounter = 0 } = await store.get(["navCounter"]);
  s.current = visit(tab, navCounter + 1, isVisible, now); s.current.revision = "open"; s.url = tab.url;
  pending.push(s.current);
  visits[id] = s;
  await outbox.addAll(pending, {visits, navCounter: navCounter + 1});
}
async function refresh() { for (const tab of await chrome.tabs.query({})) if (tab.id !== undefined) await update(tab.id); }
async function drain() { await outbox.purge(await patterns()); await outbox.flush(); }
chrome.windows.onFocusChanged.addListener(() => enqueue(async () => { await refresh(); await drain(); }));
chrome.tabs.onActivated.addListener(() => enqueue(async () => { await refresh(); await drain(); }));
chrome.tabs.onUpdated.addListener((id, info) => { if (info.title || info.status === "complete") enqueue(async () => { await update(id); await drain(); }); });
for (const hook of [chrome.webNavigation.onCommitted, chrome.webNavigation.onHistoryStateUpdated]) {
  hook.addListener(d => { if (d.frameId === 0) enqueue(async () => {
    await update(d.tabId, true, undefined, d.url);
    await chrome.tabs.sendMessage(d.tabId, {type: "route"}).catch(() => {}); await drain();
  }); });
}
chrome.tabs.onRemoved.addListener(id => enqueue(async () => {
  const {visits = {}} = await store.get(["visits"]);
  const current = visits[id]?.current;
  if (current) { current.ended_at = new Date().toISOString(); current.revision = current.ended_at; }
  delete visits[id]; await outbox.addAll(current ? [current] : [], {visits}); await drain();
}));
chrome.idle.onStateChanged.addListener(() => enqueue(async () => { await refresh(); await drain(); }));
chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  if (!sender.tab || sender.frameId !== 0 || sender.tab.incognito) return;
  const id = sender.tab.id!;
  enqueue(async () => {
    const tab = await chrome.tabs.get(id).catch(() => null);
    if (!tab?.url || excluded(tab.url, await patterns()) || sender.url !== tab.url) { reply({allowed:false}); return; }
    if (msg?.type === "policy") { reply({allowed:true}); return; }
    if (msg?.type !== "snapshot" && msg?.type !== "visibility") return;
    if (typeof msg.pageVisible !== "boolean" || msg.url !== tab.url) return;
    await update(id, false, msg.pageVisible, msg.url);
    const {visits = {}} = await store.get(["visits"]);
    if (msg.type === "snapshot" && visits[id]?.current?.payload.visible === true && typeof msg.text === "string" && msg.text.length <= 16000) {
      const digest = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(msg.text))), x => x.toString(16).padStart(2,"0")).join("");
      await outbox.add(event("text.snapshot", `tab:${id}:snapshot:${crypto.randomUUID()}`, {surface:tab.url, source:"page", text:msg.text, text_sha256:digest, completeness:msg.truncated ? "truncated" : "partial", visible:true}));
    }
    reply({allowed:true}); await drain();
  });
  return true;
});
chrome.storage.onChanged.addListener((changes, area) => {
  if (area === "local" && changes.excludedPatterns) enqueue(async () => {
    const {visits = {}} = await store.get(["visits"]); const rules = await patterns();
    for (const id of Object.keys(visits)) if (excluded(visits[id].url, rules)) delete visits[id];
    await store.set({visits}); await outbox.purge(rules); await refresh(); await drain();
  });
});
chrome.alarms.onAlarm.addListener(() => enqueue(drain));
void chrome.alarms.create("retry", {periodInMinutes: 0.5});
enqueue(async () => { await refresh(); await drain(); });
