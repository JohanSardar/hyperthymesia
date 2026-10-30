import {describe, test, expect} from "bun:test";
import Ajv from "ajv/dist/2020";
import addFormats from "ajv-formats";
import schema from "../../schema/companion-event.v1.json";
import kinds from "../../schema/kinds.v1.json";
import {DEFAULT_EXCLUSIONS, excluded, visible, visit, event, Outbox} from "./src/core";
import {editorEvent, deliver} from "../editor-ext/src/core";
import {createServer} from "node:net";
import {mkdtempSync, rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
const ajv = new Ajv({strict:false, formats:{uint32:{type:"number",validate:(x:number)=>Number.isInteger(x)&&x>=0&&x<=4294967295},uint64:{type:"number",validate:(x:number)=>Number.isSafeInteger(x)&&x>=0}}}); addFormats(ajv);
const valid = ajv.compile(schema);
const validKind = ajv.compile(kinds);
const fixture = {id:7, windowId:4, active:true, url:"https://example.test/docs", title:"Synthetic page", index:0, highlighted:true, pinned:false, incognito:false, selected:true, discarded:false, autoDiscardable:true, groupId:-1};
function memory() {
  let value: Record<string, any> = {};
  return { get:async (_keys: string[]) => structuredClone(value), set:async (v: Record<string,any>) => { value = {...value, ...structuredClone(v)}; } };
}
test("all event builders conform to the companion schema and core payload schema", async () => {
  const web = visit(fixture, 1, true);
  const snapshot = event("text.snapshot", "snapshot:1", {surface:fixture.url,source:"page",text:"Synthetic",text_sha256:"0".repeat(64),completeness:"partial",visible:true});
  const shell = await Bun.$`python3 -c ${'import runpy,json; m=runpy.run_path("companions/shell-hook/emit.py"); print(json.dumps(m["build_event"]({"source_key":"shell:1:1","occurred_at":"2026-01-01T00:00:00Z","ended_at":"2026-01-01T00:00:01Z","command":"echo synthetic","cwd":"/tmp/example","exit_code":0,"duration_ms":1000})))'}`.cwd("../..").json();
  for (const e of [web,snapshot,shell,editorEvent("editor.focus", {workspace:"/tmp/example", file_path:"/tmp/example/main.ts",language:"typescript",git_branch:"main"}),editorEvent("editor.debug",{action:"started",session_id:"synthetic",debug_type:"node"})]) {
    expect(valid(e), JSON.stringify(valid.errors)).toBe(true);
    if (["web.visit","text.snapshot","shell.command"].includes(e.kind)) expect(validKind({kind:e.kind,payload:e.payload}),JSON.stringify(validKind.errors)).toBe(true);
  }
});
test("visibility requires active tab, focused window, visible page, active idle state", () => {
  expect(visible(true,4,4,true,"active")).toBe(true);
  for (const args of [[true,4,-1,true,"active"],[true,4,5,true,"active"],[false,4,4,true,"active"],[true,4,4,false,"active"],[true,4,4,true,"idle"],[true,4,4,true,"locked"]] as const) expect(visible(...args)).toBe(false);
});
test("banking bare domains, subdomains, configured globs and credentials are excluded", () => {
  for (const url of ["https://chase.com/", "https://secure.chase.com/login", "https://bankofamerica.com/a", "https://name:secret@example.test/"]) expect(excluded(url, DEFAULT_EXCLUSIONS)).toBe(true);
  expect(excluded("https://example.test/private/a", ["*://example.test/private/*"])).toBe(true);
  expect(excluded("https://example.test/public", DEFAULT_EXCLUSIONS)).toBe(false);
  expect(excluded("file:///tmp/example", [])).toBe(true);
});
test("outbox persists through restart, retries, dedups pending identity and removes only acked items", async () => {
  const store=memory(), a=visit(fixture,1,true), b=visit(fixture,2,false);
  const first=new Outbox(store,async () => false);
  await first.add(a); await first.add(a); await first.add(b); await first.flush();
  expect((await store.get([])).outbox).toHaveLength(2);
  const received: string[]=[];
  const restarted=new Outbox(store,async e => {received.push(e.source_key); return true;});
  await restarted.flush(); await restarted.flush();
  expect(received).toEqual([a.source_key,b.source_key]); expect((await store.get([])).outbox).toEqual([]);
});
test("policy purges queued events when exclusions change", async () => {
  const store=memory(), box=new Outbox(store,async () => true);
  await box.add(visit(fixture,1,true)); await box.purge(["*://example.test/*"]);
  expect((await store.get([])).outbox).toEqual([]);
});
test("editor delivers one NDJSON line and fails silently when agent is absent", async () => {
  const dir=mkdtempSync(join(tmpdir(),"hx-editor-")), path=join(dir,"s");
  let data="";
  const server=createServer(c => c.on("data",x => {data+=x;}));
  try {
    await new Promise<void>(resolve=>server.listen(path,resolve));
    const e=editorEvent("editor.focus",{file_path:"/tmp/synthetic.ts"});
    expect(await deliver(e,path)).toBe(true);
    await new Promise(resolve=>setTimeout(resolve,20));
    expect(JSON.parse(data)).toEqual(e); expect(data.endsWith("\n")).toBe(true);
    expect(await deliver(e,join(dir,"missing"))).toBe(false);
  } finally { await new Promise<void>(resolve=>server.close(()=>resolve())); rmSync(dir,{recursive:true,force:true}); }
});
