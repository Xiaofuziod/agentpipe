#!/usr/bin/env bash
# 向 stderr 写远超管道容量(macOS 默认 64KB)的数据。
# run_command 若不持续排空 stderr,子进程会阻塞在 write 直到超时 —— 本 stub 守护那条路径。
for i in $(seq 1 2000); do
  echo "stderr-flood-$i ................................................................" >&2
done
echo "stdout-marker"
