import * as vscode from "vscode";
import {editorEvent, deliver} from "./core";
interface GitAPI { repositories: {rootUri: vscode.Uri; state: { HEAD?: {name?: string} }}[] }
export function activate(context: vscode.ExtensionContext) {
  function details() {
    const editor = vscode.window.activeTextEditor;
    const uri = editor?.document.uri;
    const workspace = uri ? vscode.workspace.getWorkspaceFolder(uri) : vscode.workspace.workspaceFolders?.[0];
    const git = vscode.extensions.getExtension<{getAPI(version: number): GitAPI}>("vscode.git");
    const api = git?.isActive ? git.exports.getAPI(1) : undefined;
    const repository = api?.repositories.filter(r => uri && uri.scheme === r.rootUri.scheme && (uri.path === r.rootUri.path || uri.path.startsWith(r.rootUri.path + "/"))).sort((a,b) => b.rootUri.path.length - a.rootUri.path.length)[0];
    return {workspace:workspace?.uri.fsPath ?? null, file_path:uri?.scheme === "file" ? uri.fsPath : null, language:editor?.document.languageId ?? null, git_branch:repository?.state.HEAD?.name ?? null};
  }
  const send = (kind: "editor.focus" | "editor.debug", extra: Record<string, unknown> = {}) => {
    if (!vscode.workspace.isTrusted) return;
    void deliver(editorEvent(kind, {...details(), ...extra}));
  };
  const focus = () => send("editor.focus", {focused:vscode.window.state.focused});
  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(focus),
    vscode.window.onDidChangeWindowState(focus),
    vscode.workspace.onDidChangeWorkspaceFolders(focus),
    vscode.debug.onDidStartDebugSession(s => send("editor.debug", {action:"started", session_id:s.id, debug_type:s.type})),
    vscode.debug.onDidTerminateDebugSession(s => send("editor.debug", {action:"terminated", session_id:s.id, debug_type:s.type})),
    vscode.debug.onDidChangeActiveDebugSession(s => send("editor.debug", {action:"active", session_id:s?.id ?? null, debug_type:s?.type ?? null})),
  );
  focus();
}
