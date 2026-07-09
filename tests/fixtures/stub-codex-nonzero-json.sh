#!/usr/bin/env bash
# 输出合法结构化结果,但以非零码退出。
# 退出码不该让引擎丢弃一份已经拿到的可解析 verdict。
echo "some warning" >&2
echo '{"verdict":"clean","findings":[]}'
exit 3
