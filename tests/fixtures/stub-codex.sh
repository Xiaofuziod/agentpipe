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
# vet 测试用调用计数:STUB_COUNT_FILE 未设时 n=1,存量测试零影响(review 首轮/多数
# 测试从不设该 env,永远只走 n=1 分支,行为与本改动前完全一致)。
n=1
if [ -n "${STUB_COUNT_FILE:-}" ]; then
  n=$(( $(cat "$STUB_COUNT_FILE" 2>/dev/null || echo 0) + 1 ))
  echo "$n" > "$STUB_COUNT_FILE"
fi
if [ "$n" -ge 2 ]; then
  if [ "${STUB_FAIL_ON_CALL_2:-}" = "1" ]; then echo "vet boom" >&2; exit 1; fi
  verdict="${STUB_VERDICT_2:-clean}"
  findings="${STUB_FINDINGS_2:-[]}"
fi
cat > "$out" <<EOF
{"verdict":"$verdict","findings":$findings}
EOF
echo "stub codex done"
