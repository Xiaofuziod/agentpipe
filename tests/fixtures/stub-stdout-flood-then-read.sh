#!/usr/bin/env bash
# 先往 stdout 灌满(约 100KB > 64KB 管道容量),之后才读 stdin。
# 若宿主在起 stdout reader 之前就阻塞在 write stdin,双方互等 → 死锁。
for i in $(seq 1 1500); do
  echo "out-$i ................................................................"
done
cat > /dev/null   # 现在才消费 stdin
echo "consumed-stdin"
