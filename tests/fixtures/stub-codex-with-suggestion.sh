#!/usr/bin/env bash
# stub codex 输出带 suggestion 字段(spec §3.2 反馈深度增强)。
# 用于验证 raw_to_result 渲染:suggestion 非空且非 "N/A" 时附加 "↳ 建议:" 行。
# 第二个 finding 用 "N/A" 验证占位也不渲染 ↳ 行。
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
cat > "$out" <<EOF
{"verdict":"changes_requested","findings":[
  {"severity":"high","file":"a.rs","line":10,"summary":"nil check missing","suggestion":"第 10 行加 if let Some(x) = y { ... }"},
  {"severity":"low","file":"b.rs","line":20,"summary":"naming","suggestion":"N/A"}
]}
EOF
echo "stub codex with suggestion done"
