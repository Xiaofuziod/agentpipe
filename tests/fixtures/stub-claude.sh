#!/usr/bin/env bash
# stub claude:模拟 `claude --output-format stream-json --verbose -p <prompt>` 的 NDJSON 输出。
# 真 claude 每行一个 JSON 对象;runner 的 StreamParser 据此提轮次/答案/度量。
last="${@: -1}"
# 最小 JSON 转义:换行/回车压成空格(真 claude 会 \n 转义;stub 简化为压扁,
# 避免裸换行产出非法 JSON 被解析层跳过),再 escape 反斜杠与引号。
esc=$(printf '%s' "$last" | tr '\n\r' '  ' | sed 's/\\/\\\\/g; s/"/\\"/g')
# result 文本默认是 MR 链接;claude-verifier 测试用 STUB_CLAUDE_RESULT 注入 "VERDICT: pass/fail"。
result=$(printf '%s' "${STUB_CLAUDE_RESULT:-https://gitlab.example.com/mr/42}" | tr '\n\r' '  ' | sed 's/\\/\\\\/g; s/"/\\"/g')
echo "{\"type\":\"system\",\"subtype\":\"init\",\"model\":\"stub\"}"
echo "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"STUB CLAUDE 收到: ${esc}\"}]}}"
echo "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":1,\"duration_ms\":1234,\"total_cost_usd\":0.01,\"result\":\"${result}\"}"
