#!/usr/bin/env bash
# stub codex 输出缺核心字段(severity)的 finding(review-2 §D finding #8)。
# 验证 RawFinding 去 #[serde(default)] 后,缺字段触发整次解析失败 → fallback
# ChangesRequested,而非静默渲染 "[] :0 summary" 乱码喂下游 fixer。
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
# finding 缺 severity 字段(file/line/summary 都在),RawFinding deserialize 应失败
cat > "$out" <<'EOF'
{"verdict":"changes_requested","findings":[
  {"file":"a.rs","line":10,"summary":"缺 severity 字段","suggestion":"N/A"}
]}
EOF
echo "stub codex malformed done"
