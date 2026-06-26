#!/usr/bin/env bash
# stub codex 输出多种占位词形式的 suggestion(review-2 §D finding #14)。
# 验证 render_finding 的 SUGGESTION_PLACEHOLDERS 集合覆盖 "none" / "无" / "tbd" 等同义。
# 注:review-2 §D finding #9 收窄占位集合 — 删了 "no" / "-" / "todo"(code review
# 上下文中是合法短建议),fixture 同步删 "-" 行。
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
cat > "$out" <<'EOF'
{"verdict":"changes_requested","findings":[
  {"severity":"high","file":"a.rs","line":1,"summary":"实际建议","suggestion":"用 X 替换 Y"},
  {"severity":"low","file":"b.rs","line":2,"summary":"none 占位","suggestion":"none"},
  {"severity":"low","file":"c.rs","line":3,"summary":"中文无 占位","suggestion":"无"},
  {"severity":"low","file":"d.rs","line":4,"summary":"TBD 占位","suggestion":"TBD"}
]}
EOF
echo "stub codex placeholder done"
