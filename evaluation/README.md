# PIGS evaluation

[简体中文](./README_CH.md) · [Evaluation plan](./EXPERIMENT_PLAN.md) · [评测方案（中文）](./EXPERIMENT_PLAN_CH.md)

The formal evaluation is organized as **one Python entry point per benchmark**. Dataset construction, agent loops, environments, and scoring are delegated to official harnesses whenever possible. This directory is responsible for Base/PIGS pairing, run metadata, background execution, file-based progress, retry policy, and unified summaries.

## Formal benchmark matrix

| Part | Benchmark | Scope | Harness | Entry point |
|---|---|---:|---|---|
| Control | GSM8K | full 1319 | EvalScope | `run_gsm8k.py` |
| Control | IFEval | full 541 | EvalScope + IFEval scorer | `run_ifeval.py` |
| Agent | BFCL V4 Multi-Turn | full 800 | official BFCL multi-turn loop + scorer | `run_bfcl_multiturn.py` |
| Agent | SWE-bench Lite | official Lite test 300 | mini-SWE-agent + official SWE-bench Docker scorer | `run_swebench_lite.py` |

The old combined MMLU-Pro / BFCL Core matrix is no longer the formal plan. `run_evaluation.py` remains only as an index/compatibility entry point.

## Model and arm semantics

All benchmark runners accept one or more base models:

```bash
--models deepseek=deepseek-v4.1-flash MODEL_2 ...
```

Each model can run three default arms:

```text
Base: MODEL
PIGS: MODEL-pigs   (combined phase output)
PIG:  MODEL-pig    (one accepted business output)
```

`--arms pigsb` remains available for reproducing older runs and sends `MODEL-pigsb`, an alias of `MODEL-pig`. New runs default to `base pigs pig`. Use `--arms base`, `--arms pigs`, or `--arms pig` to select only one arm. For formal experiments, separate run IDs per model are preferred so failures, provider behavior, and cost remain easy to audit.

Current model-selection policy is not “two fixed model families forever.” DeepSeek V4.1 Flash is the current anchor model. Additional formal models should be low-cost, publicly accessible/reproducible through a named API, and have a clear identity. MiMo remains useful as a compatibility/stress-test family but is not automatically a final core model.

## Concurrency

There are two independent concurrency controls:

- `--model-workers N`: number of model pipelines running at once. Base and PIGS remain sequential within one model pipeline.
- `--sample-workers N`: benchmark-internal task/sample concurrency.

Current DeepSeek formal runs use `sample-workers=4` (`c4`) unless a benchmark-specific calibration requires otherwise. Base and PIGS for the same paired comparison must use the same concurrency.

## Background execution and progress

All formal runners support `--background`.

Long runs write progress to files rather than relying on terminal-only progress bars:

```text
<run>/progress.txt
<run>/progress.json
```

Stages distinguish `RUNNING`, `RETRYING`, `ARCHIVING`, `DONE`, and `FAILED` where applicable. For a running job, inspect `progress.txt`; do not infer completion from launcher output alone.

## Reasoning effort and output limits

The current formal default is:

```text
thinking / reasoning effort = low
```

The runner records this in the manifest and must also propagate it to the benchmark harness. For SWE-bench Lite it is passed as mini-SWE-agent `model_kwargs.reasoning_effort`.

The evaluation runners do **not** impose an arbitrary `max_tokens`, `max_output_tokens`, or `max_completion_tokens` cap. Provider/model native output limits apply.

## PIGS usage accounting

PIGS exposes two downstream usage modes:

- `usage_mode = "max"` (default): return the complete usage JSON from the real upstream call with the largest `total_tokens`; intended for normal coding agents that use provider usage as a context-window signal.
- `usage_mode = "sum"`: recursively add numeric usage fields across every real model call triggered by the current client API request; intended for evaluation and real-consumption accounting.

Formal PIGS evaluation runs should use `sum`. Tool-pause responses close one client API request; when a continuation is resumed by a new client request, aggregation starts from zero, so physical model calls are not counted twice. For OpenAI Responses streaming runs, the terminal `response.completed` must contain the committed text in `response.output`; the runtime now closes any still-open message item during finalization so EvalScope can recover the final answer reliably.

For OpenAI-compatible Chat responses, relevant observed fields include `prompt_tokens`, `completion_tokens`, `total_tokens`, `prompt_tokens_details.cached_tokens`, provider-specific cache-write detail when present, and `completion_tokens_details.reasoning_tokens`.

## Runtime chain

The local formal environment uses:

```text
benchmark harness
    ↓
PIGS :3927
    ↓
mini-proxy :7946
    ↓
provider (currently OpenCode Go for the DeepSeek runs)
```

Base requests use the unsuffixed model and pass through PIGS normally. PIGS/PIG-arm requests use the `-pigs`/`-pig` suffix and enter the same orchestration; `-pigsb` remains a compatible alias. Do not bypass this chain for formal runs.

## Control benchmarks

### GSM8K

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/run_gsm8k.py --background
```

Full 1319-question test set. EvalScope generation timeout is 1800 seconds. The runner supports whole-sample retry for missing first-pass predictions; cached first-pass successes are not re-requested.

### IFEval

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/run_ifeval.py --background
```

Full 541 prompts. Report both official successful-row metrics and fixed-denominator metrics. When comparing Base/PIGS, also inspect common-success paired results and separate execution failures from semantic instruction-following failures.

## Agent benchmarks

### BFCL V4 Multi-Turn

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/run_bfcl_multiturn.py --background
```

Uses the official `multi_turn` categories:

```text
multi_turn_base
multi_turn_miss_func
multi_turn_miss_param
multi_turn_long_context
```

Total: 800 tasks. The adapter registers an OpenAI-compatible model with the official BFCL handler and preserves the official interaction loop, simulated tools/environment, and scorer. Request timeout defaults to 1800 seconds.

### SWE-bench Lite

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/run_swebench_lite.py --background
```

Fixed scope: official SWE-bench Lite `test`, 300 tasks.

```text
mini-SWE-agent inference
→ inference/preds.json
→ official SWE-bench Docker evaluator
→ resolved / unresolved
```

Local environment:

```text
/root/pigs-eval/src/SWE-bench
/root/pigs-eval/src/mini-swe-agent
/root/pigs-eval/swebench-venv
```

The adapter keeps the upstream official benchmark config but overrides experiment-facing model/runtime settings. Current formal settings intentionally relax two mini-SWE-agent software limits while preserving the old values as comments in the generated override:

```yaml
agent:
  # cost_limit: 3.
  cost_limit: 1000000000.0
environment:
  # timeout: 60
  timeout: 1000000
model:
  cost_tracking: "ignore_errors"
```

`step_limit: 250` remains the official config value. The relaxed `cost_limit` prevents the harness from terminating based on its own dollar estimate; `cost_tracking: ignore_errors` prevents unknown LiteLLM pricing metadata from aborting the run. Actual experiment usage is taken from API usage fields. The large shell timeout removes the per-command harness cutoff as an experimental confounder; the official SWE-bench scorer still runs in Docker with its own evaluation timeout semantics.

Current DeepSeek Base run:

```text
run id: deepseek-swebench-lite-base-low-c4-20261007
model: deepseek-v4.1-flash
arm: base
thinking_effort: low
sample_workers: 4
scope: 300 Lite test tasks
```

This is a live run; do not write a final score into documentation until the official scorer finishes.

## Dry runs

All four runners can generate manifests/progress files without requesting the model:

```bash
python run_swebench_lite.py \
  --models deepseek=deepseek-v4.1-flash \
  --arms base \
  --sample-workers 4 \
  --dry-run
```

## Output and summarization

Per run:

```text
<run>/run_manifest.json
<run>/progress.json
<run>/progress.txt
<run>/summary.json
<run>/summary.md
<run>/summary.csv
```

`summarize_evaluation.py` summarizes one run. `summarize_runs.py` combines independent runs without pretending they were one execution.

The summary layer records, when available:

- official benchmark metric;
- fixed benchmark denominator;
- predictions, malformed outputs, execution failures, and final missing samples;
- latency / TTFT / TPOT;
- endpoint-reported input/output/reasoning/cache usage;
- PIGS routing counters (Simple/Full, pig count, replans, tool calls);
- Base ↔ PIGS paired fixes/breaks;
- exact McNemar p-value for paired binary outcomes.

Headline metrics come from the official scorer rather than being reimplemented locally:

```text
GSM8K           accuracy
IFEval          prompt_level_strict
BFCL Multi-Turn multi_turn_overall
SWE-bench Lite  resolved_rate
```
