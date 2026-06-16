#!/usr/bin/env bash
# 模拟挂死的 CLI:睡 5 秒再输出,用于验证 timeout 能及时 kill。
sleep 5
echo "should not be seen before timeout"
