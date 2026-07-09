#!/usr/bin/env bash
# 模拟 codex 在输出最终结构化消息前崩溃(真实场景:写 stderr 触发 EAGAIN → panic)。
# 不写 -o 文件、stdout 无 JSON、非零退出。
echo "codex boom: failed printing to stderr" >&2
exit 1
