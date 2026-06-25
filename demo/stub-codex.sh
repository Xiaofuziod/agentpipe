#!/usr/bin/env bash
# Demo stub for the `codex` CLI review. Writes the structured {verdict, findings}
# JSON to the -o output file (the parser's fallback path) and streams human-readable
# progress to stdout. Counter-based: first review requests changes, second is clean,
# so the `until: codex-clean` loop converges after two rounds. Demo-only, deterministic.
set -euo pipefail

out=""; prev=""
for arg in "$@"; do
  [ "$prev" = "-o" ] && out="$arg"
  prev="$arg"
done

cnt="/tmp/ap-demo/codex.count"
n=$(( $(cat "$cnt" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$cnt"

echo "Reviewing diff against base branch..."; sleep 0.4
if [ "$n" -ge 2 ]; then
  echo "No remaining issues. Verdict: clean"
  [ -n "$out" ] && printf '{"verdict":"clean","findings":[]}' > "$out"
else
  echo "Found 1 high-severity issue. Verdict: changes_requested"
  [ -n "$out" ] && printf '{"verdict":"changes_requested","findings":[{"severity":"high","file":"src/auth/session.rs","line":42,"summary":"Session token is not invalidated on logout"}]}' > "$out"
fi
