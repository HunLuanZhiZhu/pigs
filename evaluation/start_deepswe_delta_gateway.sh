#!/usr/bin/env bash
# Start the updated PIGS Docker gateway for the one-task comparison.
set -euo pipefail
cd /mnt/d/AIWorkspace/pigs
mkdir -p /root/pigs-eval/service-logs/deepswe-docker-delta-http
nohup /root/pigs-eval/build/release/pigs \
    --listen 172.17.0.1:80 \
    --log-detail basic \
    --log-dir /root/pigs-eval/service-logs/deepswe-docker-delta-http \
    > /root/pigs-eval/service-logs/deepswe-delta-gateway.log 2>&1 < /dev/null &
echo $! > /root/pigs-eval/service-logs/deepswe-delta-gateway.pid
sleep 1
kill -0 "$(cat /root/pigs-eval/service-logs/deepswe-delta-gateway.pid)"
