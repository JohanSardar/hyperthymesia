import {test, expect} from "bun:test";
function hook() {
  const listeners: Function[]=[];
  return {addListener:(fn:Function)=>listeners.push(fn),fire:(...args:any[])=>listeners.map(fn=>fn(...args))};
}
test("worker correlates focus, SPA navigation and visibility; exclusions never reach storage", async () => {
  let state:Record<string,any>={};
  const writes: any[]=[];
  let tab:any={id:1,windowId:2,url:"https://example.test/one",title:"Synthetic",active:true,incognito:false};
  let focused=true;
  let idle="active";
  const onMessage=hook(),onFocusChanged=hook(),onHistoryStateUpdated=hook(),onActivated=hook(),onUpdated=hook(),onStateChanged=hook(),onRemoved=hook();
  const chromeMock:any={
    storage:{local:{get:async()=>structuredClone(state),set:async(v:any)=>{writes.push(structuredClone(v)); state={...state,...structuredClone(v)};}},onChanged:hook()},
    runtime:{onMessage,sendNativeMessage:async()=>{throw new Error("test host unavailable");}},
    tabs:{get:async()=>({...tab}),query:async()=>[{...tab}],sendMessage:async()=>{},onActivated,onUpdated,onRemoved},
    windows:{getLastFocused:async()=>({id:2,focused}),onFocusChanged},
    idle:{queryState:async()=>idle,onStateChanged},
    alarms:{create:async()=>{},onAlarm:hook()},
    webNavigation:{onCommitted:hook(),onHistoryStateUpdated},
  };
  (globalThis as any).chrome=chromeMock;
  await import("./src/background");
  async function until(predicate:()=>boolean) {
    const end=Date.now()+1000;
    while(!predicate()) {if(Date.now()>end) throw new Error("worker condition timed out"); await new Promise(r=>setTimeout(r,5));}
  }
  const message=(msg:any)=>new Promise<any>(resolve=>onMessage.fire(msg,{tab:{...tab},frameId:0,url:tab.url},resolve));
  await until(()=>!!state.visits?.[1]);
  expect(state.visits[1].current.payload.visible).toBe(false);
  await message({type:"visibility",url:tab.url,pageVisible:true});
  expect(state.visits[1].current.payload.visible).toBe(true);
  const previous=structuredClone(state.visits[1].current);
  focused=false; onFocusChanged.fire(-1);
  await until(()=>state.visits[1].current.payload.visible===false);
  expect(state.outbox.find((e:any)=>e.source_key===previous.source_key && e.ended_at)).toBeDefined();
  focused=true; onFocusChanged.fire(2);
  await until(()=>state.visits[1].current.payload.visible===true);
  tab.url="https://example.test/two"; onHistoryStateUpdated.fire({frameId:0,tabId:1,url:tab.url});
  await until(()=>state.visits[1].url===tab.url);
  expect(state.visits[1].page).toBe(false);
  await message({type:"snapshot",url:tab.url,pageVisible:true,text:"Synthetic viewport",truncated:true});
  const snapshot=state.outbox.find((e:any)=>e.kind==="text.snapshot");
  expect(snapshot.payload.completeness).toBe("truncated"); expect(snapshot.payload.visible).toBe(true);
  expect(snapshot.payload.text_sha256).toBe(new Bun.CryptoHasher("sha256").update("Synthetic viewport").digest("hex"));
  idle="locked"; onStateChanged.fire("locked");
  await until(()=>state.visits[1].current.payload.visible===false);
  const count=state.outbox.filter((e:any)=>e.kind==="text.snapshot").length;
  await message({type:"snapshot",url:tab.url,pageVisible:true,text:"Never stored while locked",truncated:false});
  expect(state.outbox.filter((e:any)=>e.kind==="text.snapshot")).toHaveLength(count);
  // Visibility changes while locked must remain available on unlock.
  await message({type:"visibility",url:tab.url,pageVisible:false});
  await message({type:"visibility",url:tab.url,pageVisible:true});
  expect(state.visits[1].page).toBe(true);
  idle="active"; onStateChanged.fire("active");
  await until(()=>state.visits[1].current.payload.visible===true);

  tab.url="https://chase.com/private-fixture"; tab.title="Excluded synthetic title";
  onHistoryStateUpdated.fire({frameId:0,tabId:1,url:tab.url});
  await until(()=>!state.visits[1]);
  expect(await message({type:"policy"})).toEqual({allowed:false});
  expect(JSON.stringify(writes)).not.toContain("private-fixture");
  expect(JSON.stringify(writes)).not.toContain("Excluded synthetic title");
  expect(JSON.stringify(writes)).not.toContain("Never stored while locked");
});
