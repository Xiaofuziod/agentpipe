#!/usr/bin/env bash
# 解析 -o 后面的输出文件路径
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
verdict="${STUB_VERDICT:-changes_requested}"
# review §A finding #11 修正后:RawFinding 全部字段 required(去 #[serde(default)]),
# 与 OpenAI strict schema 对齐。stub 必须带 suggestion 字段否则整 RawReview 解析失败
# 走 fallback ChangesRequested,parses_clean / parses_changes_requested 期望失败。
cat > "$out" <<EOF
{"verdict":"$verdict","findings":[{"severity":"high","file":"a.rs","line":10,"summary":"示例问题","suggestion":"N/A"}]}
EOF
echo "stub codex done"
