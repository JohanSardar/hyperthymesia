export interface CompanionEvent {
  companion: string; kind: string; source_key: string; occurred_at: string;
  ended_at?: string; revision?: string; sensitivity: "restricted";
  payload: Record<string, unknown>;
}
export const event = (kind: string, key: string, payload: Record<string, unknown>, at = new Date().toISOString()): CompanionEvent =>
  ({ companion: "browser", kind, source_key: key, occurred_at: at, sensitivity: "restricted", payload });
export const DEFAULT_EXCLUSIONS = ["*://*.chase.com/*", "*://*.bankofamerica.com/*", "*://*.wellsfargo.com/*", "*://*.citi.com/*", "*://*.capitalone.com/*", "*://*.paypal.com/*"];
export function excluded(url: string, patterns: string[]): boolean {
  try {
    const u = new URL(url);
    if (!["http:", "https:"].includes(u.protocol) || u.username || u.password) return true;
    return patterns.some(pattern => {
      const escaped = pattern.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replaceAll("*", ".*");
      // A wildcard subdomain also includes the bare domain.
      return new RegExp("^" + escaped.replace(".*\\.", "(?:.*\\.)?") + "$", "i").test(url);
    });
  } catch { return true; }
}
export const visible = (active: boolean, windowId: number, focused: number, page: boolean, idle: string) =>
  active && windowId === focused && focused !== -1 && page && idle === "active";
export interface Store { get(keys: string[]): Promise<Record<string, any>>; set(value: Record<string, any>): Promise<void> }
export class Outbox {
  constructor(private store: Store, private send: (e: CompanionEvent) => Promise<boolean>) {}
  async add(e: CompanionEvent) {
    const { outbox = [] } = await this.store.get(["outbox"]);
    if (!outbox.some((x: CompanionEvent) => x.source_key === e.source_key && x.revision === e.revision)) {
      await this.store.set({ outbox: [...outbox, e] });
    }
  }
  async purge(patterns: string[]) {
    const { outbox = [] } = await this.store.get(["outbox"]);
    await this.store.set({ outbox: outbox.filter((e: CompanionEvent) => !excluded(String(e.payload.url ?? e.payload.surface), patterns)) });
  }
  async flush() {
    const { outbox = [] } = await this.store.get(["outbox"]);
    for (const e of outbox as CompanionEvent[]) {
      if (!await this.send(e)) break;
      const current = await this.store.get(["outbox"]);
      await this.store.set({outbox: current.outbox.filter((x: CompanionEvent) => !(x.source_key === e.source_key && x.revision === e.revision))});
    }
  }
}
export function visit(tab: chrome.tabs.Tab, nav: number, isVisible: boolean, at?: string) {
  return event("web.visit", `tab:${tab.id}:nav:${nav}`, {url: tab.url!, title: tab.title ?? null, tab_id: tab.id, window_id: tab.windowId, visible: isVisible, browser: "chrome"}, at);
}
