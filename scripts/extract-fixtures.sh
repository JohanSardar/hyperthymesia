#!/bin/bash
# Raw private data streams only through memory into a deny-by-default anonymizer.
set -euo pipefail
source "$HOME/.cargo/env"
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/Hyperthymesia/.target-main}"
anonymizer=$(cat <<'PY'
import sys,json,hashlib,re
safe={
 'kind':{'agent.message','code.commit','usage.aggregate','message.event','calendar.event'},
 'provider':{'t3code','codex','claude-code'},
 'operation':{'observe','upsert','delete'},
 'role':{'user','assistant','tool','lifecycle'},
 'sensitivity':{'normal','sensitive','restricted'},
 'completeness':{'complete','partial','truncated','unknown'},
 'rel':{'session','same_as','parent','caused_by'},
 'direction':{'sent','received'},
 'claim_scope':{'provider_provenance','history_index'},
}
def hashed(s):return 'h_'+hashlib.sha256(s.encode()).hexdigest()[:24]
def clean(v,key=''):
 if isinstance(v,dict):return {k:clean(x,k) for k,x in sorted(v.items())}
 if isinstance(v,list):return [clean(x,key) for x in v]
 if not isinstance(v,str):return v
 if v in safe.get(key,set()):return v
 if key in {'text','message','note'}:
  return 'x'*(0 if not v else 8 if len(v)<=16 else 32 if len(v)<=64 else 128 if len(v)<=256 else 512)
 if key in {'source_start','source_end','observed_at','start','end'}:return '2000-01-01T00:00:00Z'
 if key=='day':return '2000-01-01'
 if key=='sha':return hashlib.sha256(v.encode()).hexdigest()
 return hashed(v)
def ulid(s):
 n=int.from_bytes(hashlib.sha256(s.encode()).digest()[:16],'big');alphabet='0123456789ABCDEFGHJKMNPQRSTVWXYZ';out=''
 for _ in range(26):out=alphabet[n&31]+out;n>>=5
 return out
for line in sys.stdin:
 e=json.loads(line);out=clean(e)
 out['observation_id']=ulid(json.dumps([e['connector_instance'],e['source_key'],e['revision']],separators=(',',':')))
 print(json.dumps(out,sort_keys=True,separators=(',',':')))
PY
)
if [[ "${1:-}" == "--anonymize" ]]; then
    exec python3 -c "$anonymizer"
fi
for connector in t3code codex claude-code git apple-bridge; do
    destination="fixtures/sources/$connector/envelopes.ndjson"
    # Only anonymized data reaches the temporary file, atomically replacing the sample.
    temporary=$(mktemp "${destination}.XXXXXX")
    if cargo run --quiet -p "hx-connector-$connector" -- sync --dry-run --limit 8 2>/dev/null | python3 -c "$anonymizer" > "$temporary"; then
        mv "$temporary" "$destination"
    else
        rm -f "$temporary"
        echo "Fixture extraction failed for $connector; existing sample retained" >&2
        exit 1
    fi
done
