#!/usr/bin/env bash
# 模拟挂死/极慢的 claude:长睡,等引擎超时把整组 kill
sleep 30
echo '{"type":"result","subtype":"success","num_turns":1,"result":"done"}'
