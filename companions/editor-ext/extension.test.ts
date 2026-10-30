import {test, expect, mock} from "bun:test";
function hook() { const callbacks: Function[]=[]; return {subscribe:(fn:Function)=>{callbacks.push(fn); return {dispose(){}};},fire:(...args:any[])=>callbacks.forEach(fn=>fn(...args))}; }
test("editor emits workspace/file focus and debug lifecycle without reading file content", async () => {
  const active=hook(),windowState=hook(),folders=hook(),start=hook(),stop=hook(),debug=hook();
  const root={scheme:"file",path:"/tmp/synthetic",fsPath:"/tmp/synthetic"};
  const uri={scheme:"file",path:"/tmp/synthetic/main.ts",fsPath:"/tmp/synthetic/main.ts"};
  const api:any={
    window:{activeTextEditor:{document:{uri,languageId:"typescript"}},state:{focused:true},onDidChangeActiveTextEditor:active.subscribe,onDidChangeWindowState:windowState.subscribe},
    workspace:{isTrusted:true,workspaceFolders:[{uri:root}],getWorkspaceFolder:()=>({uri:root}),onDidChangeWorkspaceFolders:folders.subscribe},
    extensions:{getExtension:()=>({isActive:true,exports:{getAPI:()=>({repositories:[{rootUri:root,state:{HEAD:{name:"synthetic-branch"}}}]})}})},
    debug:{onDidStartDebugSession:start.subscribe,onDidTerminateDebugSession:stop.subscribe,onDidChangeActiveDebugSession:debug.subscribe},
  };
  const sent:any[]=[];
  mock.module("vscode",()=>api);
  mock.module("./src/core",()=>({editorEvent:(kind:string,payload:any)=>({kind,payload}),deliver:async(e:any)=>{sent.push(e);return true;}}));
  const {activate}=await import("./src/extension");
  const context:any={subscriptions:[]}; activate(context);
  expect(sent[0]).toEqual({kind:"editor.focus",payload:{workspace:"/tmp/synthetic",file_path:"/tmp/synthetic/main.ts",language:"typescript",git_branch:"synthetic-branch",focused:true}});
  api.window.state.focused=false; windowState.fire({focused:false}); expect(sent.at(-1).payload.focused).toBe(false);
  start.fire({id:"session-1",type:"node"}); expect(sent.at(-1).payload.action).toBe("started");
  stop.fire({id:"session-1",type:"node"}); expect(sent.at(-1).payload.action).toBe("terminated");
  debug.fire(undefined); expect(sent.at(-1).payload.session_id).toBeNull();
  api.workspace.isTrusted=false; const before=sent.length; active.fire(); expect(sent).toHaveLength(before);
  expect(context.subscriptions).toHaveLength(6);
});
