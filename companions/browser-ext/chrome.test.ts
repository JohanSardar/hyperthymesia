import {test, expect} from "bun:test";
import {chromium} from "playwright-core";
import {existsSync} from "node:fs";
import {join} from "node:path";
test.skipIf(!existsSync("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"))("installed Chrome exercises bundled viewport capture with mocked extension messaging", async () => {
  const script=join(import.meta.dir,"dist/content.js");
  const built=Bun.spawnSync(["bun","run","build"],{cwd:import.meta.dir});
  expect(built.exitCode).toBe(0);
  const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch:()=>new Response('<!doctype html><title>Synthetic fixture</title><p>Synthetic viewport evidence</p><input value="Never read input"><p style="display:none">Never read hidden</p><div style="opacity:0"><span>Never read transparent</span></div><p style="position:absolute;top:5000px">Never read offscreen</p><button onclick="history.pushState({},\'\',\'/route-two\')">Route</button>',{headers:{"content-type":"text/html"}})});
  const browser=await chromium.launch({channel:"chrome",headless:true});
  try {
    const page=await browser.newPage();
    await page.addInitScript(() => {
      const w=window as any;
      w.messages=[];
      w.chrome.runtime={sendMessage:async(msg:any)=>{w.messages.push(msg); return {allowed:true};}, onMessage:{addListener:(fn:any)=>{w.route=fn;}}};
    });
    await page.goto(`http://127.0.0.1:${server.port}/`);
    await page.addScriptTag({path:script});
    await page.waitForFunction(()=>(window as any).messages.some((m:any)=>m.type==="snapshot"));
    const snap=await page.evaluate(()=>(window as any).messages.find((m:any)=>m.type==="snapshot"));
    expect(snap.text).toContain("Synthetic viewport evidence");
    expect(snap.text).not.toContain("Never read");
    await page.click("button");
    await page.evaluate(()=>(window as any).route({type:"route"}));
    await page.waitForFunction(()=>(window as any).messages.some((m:any)=>m.type==="snapshot"&&m.url.endsWith("/route-two")));
    await page.evaluate(()=>{ document.querySelector("p")!.textContent="x".repeat(18000); (window as any).route({type:"route"}); });
    await page.waitForFunction(()=>(window as any).messages.some((m:any)=>m.type==="snapshot"&&m.truncated));
    const capped=await page.evaluate(()=>(window as any).messages.find((m:any)=>m.type==="snapshot"&&m.truncated));
    expect(capped.text.length).toBeLessThanOrEqual(16000);
    const edges=await page.evaluate(() => {
      const w=window as any, start=w.messages.length;
      Object.defineProperty(document,"visibilityState",{value:"hidden",configurable:true}); document.dispatchEvent(new Event("visibilitychange"));
      Object.defineProperty(document,"visibilityState",{value:"visible",configurable:true}); document.dispatchEvent(new Event("visibilitychange"));
      return w.messages.slice(start).map((m:any)=>m.pageVisible);
    });
    expect(edges).toEqual([false,true]);
  } finally {await browser.close(); server.stop(true);}
},30000);
