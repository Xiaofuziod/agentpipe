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
severity="${STUB_SEVERITY:-high}"
# 默认 findings 先落中间变量再喂 ${STUB_FINDINGS:-...}:default word 里裸 `}` 会被 bash
# 参数展开的括号匹配提前收尾(非 ${} 嵌套不计入深度),直接内联会把 JSON 尾部的 `}` 吃掉
# 产出坏 JSON(实测 `x="${Y:-[{\"a\":1}]}"` 展开成 `[{"a":1]}`),导致 codex 输出解析失败。
default_findings="[{\"severity\":\"$severity\",\"file\":\"a.rs\",\"line\":10,\"summary\":\"示例问题\",\"suggestion\":\"N/A\"}]"
findings="${STUB_FINDINGS:-$default_findings}"
cat > "$out" <<EOF
{"verdict":"$verdict","findings":$findings}
EOF
echo "stub codex done"
