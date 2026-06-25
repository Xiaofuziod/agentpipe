#!/usr/bin/env bash
# Demo stub for the `claude` CLI — emits the same stream-json NDJSON shape the real
# binary does, so AgentPipe's stream parser extracts progress lines + metrics.
# Used only to record the README demo GIF (no API calls, fully deterministic).
set -euo pipefail
prompt="${*: -1}"

emit() {
  printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]}}\n' "$1"
}

echo '{"type":"system","subtype":"init","model":"claude-opus-stub"}'
if printf '%s' "$prompt" | grep -qiE 'check ?out|branch'; then
  emit "Resolving PR branch via gh pr checkout..."; sleep 0.3
  emit "Switched to branch fix/session-logout"; sleep 0.3
  result="fix/session-logout"
else
  emit "Reading Codex findings..."; sleep 0.3
  emit "Editing src/auth/session.rs - invalidate token on logout"; sleep 0.35
  emit "Pushing fix to PR branch"; sleep 0.3
  result="pushed 1 commit"
fi
printf '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":4200,"total_cost_usd":0.06,"result":"%s"}\n' "$result"
