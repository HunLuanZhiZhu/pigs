#!/usr/bin/env bash
# Launch precisely one DeepSWE trial and keep its process independent of this terminal.
set -euo pipefail
ROOT=/root/pigs-eval/outputs/deepswe-pig
NAME=ds-pig-v11-actionlint-delta-20261022
mkdir -p "$ROOT"
test ! -e "$ROOT/$NAME"
nohup bash /mnt/d/AIWorkspace/pigs/evaluation/run_deepswe_actionlint_delta.sh \
    > "$ROOT/$NAME.launch.log" 2>&1 < /dev/null &
echo $! > "$ROOT/$NAME.pid"
sleep 2
kill -0 "$(cat "$ROOT/$NAME.pid")"
