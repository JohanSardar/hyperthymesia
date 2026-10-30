import {test, expect} from "bun:test";
import {createServer} from "node:net";
import {mkdtempSync, rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
const hook = join(import.meta.dir,"hyperthymesia.zsh");
const emitter = join(import.meta.dir,"emit.py");
test("secret denylist redacts assignments, flags, headers, credentials and recognizable tokens", async () => {
  const inputs=["SERVICE_KEY=synthetic tool", "export api_token='synthetic value'", "tool --password 'synthetic value'", "curl -H 'Authorization: Bearer synthetic' https://example.test", "curl https://name:synthetic@example.test", "echo ghp_synthetic", "tool --client-secret=synthetic", "tool password=synthetic"];
  for (const input of inputs) {
    const p=Bun.spawn(["python3","-c","import runpy,sys; print(runpy.run_path(sys.argv[1])['redact'](sys.stdin.read()))",emitter],{stdin:new Blob([input]),stdout:"pipe"});
    expect(await new Response(p.stdout).text()).toContain("[REDACTED:"); expect(await p.exited).toBe(0);
  }
  const p=Bun.spawn(["python3","-c","import runpy,sys; print(runpy.run_path(sys.argv[1])['redact']('git status'))",emitter],{stdout:"pipe"});
  expect((await new Response(p.stdout).text()).trim()).toBe("git status");
});
test("zsh hook sends valid redacted NDJSON, preserves exit status and duration", async () => {
  const dir=mkdtempSync(join(tmpdir(),"hx-hook-")), path=join(dir,"s");
  let resolveLine: (s:string)=>void;
  const line=new Promise<string>(resolve=>{resolveLine=resolve;});
  const server=createServer(c => {let data=""; c.on("data",x=>{data+=x; if(data.includes("\n")) resolveLine(data);});});
  try {
    await new Promise<void>(resolve=>server.listen(path,resolve));
    const p=Bun.spawn(["zsh","-f","-c",'source "$1"; _hx_preexec "API_KEY=synthetic tool"; sleep 0.03; false; _hx_precmd',"test",hook],{env:{...process.env,HYPERTHYMESIA_SOCKET:path},stdout:"pipe",stderr:"pipe"});
    expect(await p.exited).toBe(0);
    const text=await Promise.race([line,new Promise<never>((_,reject)=>setTimeout(()=>reject(new Error("no line from hook")),3000))]);
    const e=JSON.parse(text);
    expect(e.kind).toBe("shell.command"); expect(e.payload.command).toContain("REDACTED"); expect(e.payload.exit_code).toBe(1); expect(e.payload.duration_ms).toBeGreaterThanOrEqual(20); expect(text.split("\n")).toHaveLength(2);
  } finally {await new Promise<void>(resolve=>server.close(()=>resolve())); rmSync(dir,{recursive:true,force:true});}
});
test("missing socket never blocks the prompt", async () => {
  const p=Bun.spawn(["zsh","-f","-c",'source "$1"; _hx_preexec "echo synthetic"; _hx_precmd; print -r -- "$EPOCHREALTIME"',"test",hook],{env:{...process.env,HYPERTHYMESIA_SOCKET:"/tmp/hx-does-not-exist/socket"},stdout:"pipe",stderr:"pipe"});
  const start=performance.now();
  expect(await p.exited).toBe(0);
  const elapsed=performance.now()-start;
  console.log(`Missing socket hook process returned in ${elapsed.toFixed(1)} ms`);
  expect(elapsed).toBeLessThan(250); expect(await new Response(p.stderr).text()).toBe("");
});
