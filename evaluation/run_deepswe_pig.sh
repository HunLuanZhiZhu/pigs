#!/usr/bin/env bash
# DeepSWE v1.1 full 113-task -pig evaluation; official Pier + mini-swe-agent.
# Run inside Ubuntu-24.04 WSL as root, not Windows PowerShell.
# This script never carries an upstream API key: PIGS injects it on the host.
set -euo pipefail

TASKS=/mnt/d/AIWorkspace/DeepSWE/deep-swe/tasks
PIER=/root/pigs-eval/deepswe-venv/bin/pier
JOBS=/root/pigs-eval/outputs/deepswe-pig
JOB_NAME=ds-pig-v11-full-fixed-20261022
CONCURRENT=4

test -x "$PIER"
test -d "$TASKS"
test "$(find "$TASKS" -mindepth 2 -maxdepth 2 -name task.toml | wc -l)" -eq 113
docker compose version >/dev/null
docker info >/dev/null
curl --connect-timeout 3 --max-time 5 --silent --output /dev/null \
  http://172.17.0.1/v1/models

mkdir -p "$JOBS"
cd /root/pigs-eval
export LITELLM_LOCAL_MODEL_COST_MAP=true

exec "$PIER" run \
    --path "$TASKS" \
    --jobs-dir "$JOBS" \
    --job-name "$JOB_NAME" \
    --n-concurrent "$CONCURRENT" \
    --max-retries 0 \
    --agent mini-swe-agent \
    --model openai/deepseek-v4.1-flash-pig \
    --agent-kwarg model_class=litellm \
    --agent-kwarg version=2.4.6 \
    --agent-kwarg reasoning_effort=low \
    --agent-env OPENAI_API_BASE=http://172.17.0.1 \
    --agent-env OPENAI_BASE_URL=http://172.17.0.1 \
    --agent-env OPENAI_API_KEY=local-evaluation \
    --agent-env MSWEA_API_KEY=local-evaluation \
    --yes
