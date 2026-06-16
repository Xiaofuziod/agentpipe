#!/usr/bin/env bash
# 解析 -o 后面的输出文件路径
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
verdict="${STUB_VERDICT:-changes_requested}"
cat > "$out" <<EOF
{"verdict":"$verdict","findings":[{"severity":"high","file":"a.rs","line":10,"summary":"示例问题"}]}
EOF
echo "stub codex done"
