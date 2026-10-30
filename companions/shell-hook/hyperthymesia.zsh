# Source from .zshrc. Records interactive preexec/precmd pairs only.
[[ -n ${_HX_HOOK_LOADED:-} ]] && return
[[ -n ${commands[python3]:-} ]] || return
zmodload zsh/datetime || return
zmodload zsh/system || return
zmodload zsh/parameter || return
typeset -g _HX_HOOK_LOADED=1
typeset -g _HX_EMITTER="${${(%):-%x}:A:h}/emit.py"
typeset -g _HX_PYTHON="${commands[python3]}"
typeset -g _HX_SESSION="${sysparams[pid]}:${EPOCHREALTIME}"
typeset -gi _HX_SEQUENCE=0
typeset -g _HX_COMMAND='' _HX_CWD='' _HX_STARTED=''

_hx_preexec() {
  _HX_COMMAND="$1"
  _HX_CWD="$PWD"
  _HX_STARTED="$EPOCHREALTIME"
  (( ++_HX_SEQUENCE ))
  return 0
}
_hx_precmd() {
  local hx_exit=$?
  [[ -n $_HX_STARTED ]] || return 0
  local hx_end="$EPOCHREALTIME"
  local hx_command="$_HX_COMMAND" hx_cwd="$_HX_CWD" hx_start="$_HX_STARTED"
  local hx_key="shell:${_HX_SESSION}:${_HX_SEQUENCE}"
  _HX_STARTED='' _HX_COMMAND=''
  # JSON encoding and socket I/O run in a disowned subprocess. The prompt does
  # not wait for Python startup, connect, backpressure, or a missing agent.
  (
    builtin printf '%s\0%s\0%s\0%s\0%s\0%s\0%s\0' "$hx_command" "$hx_cwd" "$hx_start" "$hx_end" "$hx_exit" "$hx_key" "${HYPERTHYMESIA_SOCKET:-}" |
      "$_HX_PYTHON" -c '
import datetime,json,os,sys
parts=sys.stdin.buffer.read().decode("utf-8", "replace").split("\0")
command,cwd,start,end,code,key,path=parts[:7]
def iso(value): return datetime.datetime.fromtimestamp(float(value),datetime.timezone.utc).isoformat()
data={"command":command,"cwd":cwd,"occurred_at":iso(start),"ended_at":iso(end),"exit_code":int(code),"duration_ms":max(0,round((float(end)-float(start))*1000)),"source_key":key}
import runpy
module=runpy.run_path(sys.argv[1])
if path: os.environ["HYPERTHYMESIA_SOCKET"]=path
import io
sys.stdin=io.StringIO(json.dumps(data))
module["main"]()
' "$_HX_EMITTER"
  ) </dev/null >/dev/null 2>&1 &!
  return 0
}
if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  add-zsh-hook preexec _hx_preexec
  add-zsh-hook precmd _hx_precmd
fi
