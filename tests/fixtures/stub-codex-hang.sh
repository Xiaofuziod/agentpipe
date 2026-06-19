#!/usr/bin/env bash
# 模拟挂死/极慢的 codex:长睡,等引擎超时把整组 kill
sleep 30
echo '{"verdict":"clean","findings":[]}'
