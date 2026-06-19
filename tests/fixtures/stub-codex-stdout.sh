#!/usr/bin/env bash
# 模拟真实 codex v0.139.0:最终结构化结果打到 stdout,不写 -o(--output-last-message)文件
verdict="${STUB_VERDICT:-clean}"
echo "审查中…(进度行)"
echo "{\"verdict\":\"$verdict\",\"findings\":[]}"
