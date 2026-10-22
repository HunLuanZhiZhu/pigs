#!/usr/bin/env bash
# Single task, same Pier agent and model as original run.
set -euo pipefail
TASK=/mnt/d/AIWorkspace/DeepSWE/deep-swe/tasks/actionlint-action-pinning-lint
PIER=/root/pigs-eval/deepswe-venv/bin/pier
JOBS=/root/pigs-eval/outputs/deepswe-pig
JOB_NAME=ds-pig-v11-actionlint-delta-20261022
test -f "$TASK/task.toml"
test ! -e "$JOBS/$JOB_NAME"
cd /root/pigs-eval
export LITELLM_LOCAL_MODEL_COST_MAP=true
exec "$PIER" run --path "$TASK" --jobs-dir "$JOBS" --job-name "$JOB_NAME" --n-concurrent 1 --max-retries 0 --agent mini-swe-agent --model openai/deepseek-v4.1-flash-pig --agent-kwarg model_class=litellm --agent-kwarg version=2.4.6 --agent-kwarg reasoning_effort=low --agent-env OPENAI_API_BASE=http://172.17.0.1 --agent-env OPENAI_BASE_URL=http://172.17.0.1 --agent-env OPENAI_API_KEY=local-evaluation --agent-env MSWEA_API_KEY=local-evaluation --yes
