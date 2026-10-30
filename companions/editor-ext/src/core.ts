import {createConnection} from "node:net";
import {homedir} from "node:os";
import {join} from "node:path";
import {randomUUID} from "node:crypto";
export function editorEvent(kind: "editor.focus" | "editor.debug", payload: Record<string, unknown>) {
  return {companion:"editor", kind, source_key:`editor:${randomUUID()}`, occurred_at:new Date().toISOString(), sensitivity:"restricted", payload};
}
export const defaultSocket = () => join(homedir(), "Library", "Application Support", "Hyperthymesia", "agent.sock");
export function deliver(value: ReturnType<typeof editorEvent>, path = defaultSocket()): Promise<boolean> {
  return new Promise(resolve => {
    const client = createConnection({path});
    let settled = false;
    const done = (ok: boolean) => { if (settled) return; settled = true; clearTimeout(deadline); client.destroy(); resolve(ok); };
    const deadline = setTimeout(() => done(false), 250);
    client.on("error", () => done(false));
    client.on("connect", () => client.end(JSON.stringify(value) + "\n", () => done(true)));
    client.on("close", () => done(false));
  });
}
