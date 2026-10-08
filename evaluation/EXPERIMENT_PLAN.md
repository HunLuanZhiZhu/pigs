# PIGS formal evaluation plan

[简体中文](./EXPERIMENT_PLAN_CH.md) · [Evaluation guide](./README.md) · [评测运行说明](./README_CH.md)

> Status: implemented and actively running. The benchmark matrix is fixed; model selection beyond the current DeepSeek anchor remains cost-driven and may still change.

## 1. Evaluation questions

The formal evaluation has two complementary parts.

### Part A — Control / non-regression

Question: does adding PIGS materially damage ordinary single-turn reasoning or strict instruction following?

These benchmarks are not expected to prove a large gain. Their role is to detect regressions, routing mistakes, representation pollution, and harness failures.

### Part B — Agentic / long-horizon

Question: does `Pre → Executor → Post` improve end-to-end success when the outer task already requires multiple model calls, tools, environment feedback, and revision?

The causal rule is:

> Keep the official benchmark task, agent/harness, tools, environment, and scorer fixed. Change only whether model calls use direct inference or the PIGS model suffix.

## 2. Model policy

DeepSeek V4.1 Flash is the current anchor model.

Additional formal models are selected primarily by:

1. low cost;
2. public/reproducible access for other users or researchers;
3. clear model identity rather than opaque aliases;
4. stable API behavior suitable for the benchmark harness;
5. useful diversity relative to DeepSeek.

Open weights are not required. Reproducibility here means that another user can access the same named model/API later. MiMo remains useful as a compatibility/stress-test family but is not automatically a final core model.

Every formal comparison is paired within the same named model:

```text
MODEL
vs
MODEL-pigs (combined output)
vs
MODEL-pig (single accepted output)
```

Base, PIGS, and PIG must use the same provider route, reasoning effort, harness settings, concurrency, task order, tools, timeouts, and scorer. The legacy `-pigsb` suffix is still accepted as an alias for `-pig` to preserve reproducibility of historical runs.

## 3. Benchmark matrix

| Part | Benchmark | Formal scope | Official execution/scoring path | Main capability |
|---|---|---:|---|---|
| Control | GSM8K | full 1319 | EvalScope | arithmetic reasoning / target fidelity |
| Control | IFEval | full 541 | EvalScope + IFEval scorer | strict instruction following |
| Agent | BFCL V4 Multi-Turn | full 800 | official BFCL multi-turn loop + scorer | multi-turn tools/state/missing information |
| Agent | SWE-bench Lite | official Lite test 300 | mini-SWE-agent + official Docker scorer | long-horizon code search/edit/test/revision |

The formal benchmark itself must not be manually reduced. Small subsets are allowed only for connectivity, timing, and concurrency calibration and must never be reported as the formal benchmark result.

MMLU-Pro is not in the current core matrix because of cost and weaker alignment with the agentic claim. GAIA remains an extension candidate. SWE-bench Verified remains a future upgrade candidate after the Lite pipeline is stable and affordable.

## 4. Agent-benchmark causal design

### BFCL V4 Multi-Turn

```text
BFCL task
  ↓
official multi-turn handler
  ↓
MODEL or MODEL-pigs
  ↓
BFCL simulated tools / environment
  ↓
handler continues interaction
  ↓
official scorer
```

Do not modify BFCL policy, tool environment, or task flow specifically for PIGS. The adapter may register an OpenAI-compatible model and set experiment-wide request parameters such as reasoning effort and timeout.

### SWE-bench Lite

```text
SWE-bench Lite task
  ↓
mini-SWE-agent
  ↓
MODEL or MODEL-pigs
  ↓
repo / shell / tests
  ↓
observation
  ↓
mini-SWE-agent continues trajectory
  ↓
patch
  ↓
official SWE-bench Docker scorer
  ↓
resolved / unresolved
```

The outer mini-SWE-agent loop is part of the fixed harness. PIGS sits behind the model endpoint; it must not replace the benchmark agent loop.

## 5. Fairness constraints

For one paired Base/PIGS comparison, keep identical:

- provider and model alias/snapshot;
- reasoning effort;
- temperature/top-p and any explicit generation parameters;
- tool definitions and tool-choice policy;
- benchmark agent/harness version;
- task set/order/seed;
- sample concurrency;
- environment/container image;
- transport timeout and retry policy;
- official scorer.

Different model families may use provider-native behavior when necessary, but do not compare two different harness conditions as if the only variable were PIGS.

## 6. Reasoning, output, and timeout policy

Current formal reasoning effort is `low` unless a separately declared experiment condition says otherwise.

Evaluation runners do not impose arbitrary output-token limits. Provider/model native limits apply, and explicit length truncation must be treated as an execution failure rather than silently scored as a complete answer.

Current long-request timeout policy:

- GSM8K / IFEval generation timeout: 1800 s;
- BFCL OpenAI-compatible request timeout: 1800 s;
- mini-proxy upstream total timeout: 1800 s.

SWE-bench uses the official mini-SWE-agent benchmark config with these experiment overrides:

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

The official `step_limit: 250` remains unchanged. These overrides remove mini-SWE-agent's software dollar cutoff and per-shell-command cutoff as experimental confounders; they do not remove external provider limits or the official Docker scorer's evaluation timeout.

## 7. Usage accounting

PIGS has two explicit downstream usage modes:

- `max` (default): return the complete original usage JSON from the real call with the largest `total_tokens`; intended for normal coding-agent context-window decisions.
- `sum`: recursively add numeric usage fields from every real model call triggered by the current client API request; intended for formal evaluation and consumption accounting.

Formal PIGS runs should use `sum`. The accounting boundary is one client API request. A tool-call pause closes that request; continuation after tool execution is a new request and starts usage aggregation from zero.

Do not confuse total billed/physical model consumption with the context-window usage that an outer coding agent needs. They are intentionally represented by different modes.

## 8. Concurrency and run strategy

Use benchmark-native worker parallelism. Current DeepSeek formal runs use `sample_workers=4` (`c4`) unless calibration demonstrates instability.

Before increasing concurrency, run a small calibration and check:

- provider/runtime error rate;
- rate-limit/retry behavior;
- Docker/CPU/RAM pressure;
- whether benchmark outcomes visibly change under concurrency.

Calibration tasks are not formal results.

Different model families should normally use separate run IDs. Within one model, Base/PIGS should remain sequential unless there is a strong reason to accept direct throughput contention.

## 9. Reliability and retry semantics

Execution reliability and task correctness are separate quantities.

For GSM8K / IFEval, the runner may retry missing predictions after the first pass using EvalScope cache. First-pass successes are not re-requested, and first-pass reliability remains recorded.

For BFCL and SWE-bench, preserve the official interaction/evaluation semantics rather than inventing a generic cross-benchmark retry layer.

A failed request should not silently disappear from the denominator. Reports should distinguish:

- successful prediction but wrong answer;
- malformed prediction;
- benchmark/harness execution failure;
- provider/transport failure;
- final missing sample.

## 10. Reporting

Headline metrics come from official scorers:

```text
GSM8K           accuracy
IFEval          prompt_level_strict
BFCL Multi-Turn multi_turn_overall
SWE-bench Lite  resolved_rate
```

Also report, where applicable:

- fixed-denominator score;
- Base/PIGS common-success paired score;
- fixes and breaks;
- exact McNemar p-value for paired binary outcomes;
- latency / TTFT / TPOT;
- input/output/reasoning/cache usage;
- execution failure counts and IDs;
- PIGS routing data (Simple/Full, pig count, replans, tool calls).

Do not treat one stochastic run difference as a universal model claim. Separate observed result, statistical evidence, harness artifact, and causal interpretation.

## 11. Minimum reproducibility record

Every formal run should preserve at least:

- PIGS repository commit;
- benchmark/harness commit;
- model ID and provider route;
- Base/PIGS arm;
- reasoning effort;
- concurrency;
- timeout/retry policy;
- exact benchmark scope/split;
- run ID and session ID;
- output-limit policy;
- official scorer output;
- progress/failure metadata;
- relevant PIGS HTTP/orchestration logs when diagnostics are needed.

Private API keys must never be written into committed manifests or documentation.

## 12. Current execution status — 2026-10-07

Completed control work has already shown that routing/representation details matter as much as raw benchmark score. DeepSeek GSM8K and IFEval have formal Base/PIGS data; IFEval in particular exposed Full-path response-composition problems that motivated later routing/output refinements.

BFCL V4 Multi-Turn has a partial DeepSeek formal run. The frozen first-408 diagnostic comparison was mildly positive overall but not statistically significant; the run was interrupted by provider/account `402` behavior and is not a completed formal 800-task result.

SWE-bench Lite is now in formal execution. Current Base run:

```text
run id: deepseek-swebench-lite-base-low-c4-20261007
model: deepseek-v4.1-flash
arm: base
scope: Lite test 300
reasoning_effort: low
sample_workers: 4
```

The Base run must finish and pass the official Docker scorer before a final baseline `resolved_rate` is recorded. Only then should the paired PIGS arm be launched with the same harness/runtime parameters.

## 13. Main interpretation questions

### Q1. Does PIGS preserve ordinary-task quality?

Use GSM8K and IFEval, but interpret execution failures and representation artifacts separately from semantic correctness.

### Q2. Does PIGS improve long-horizon agent success?

Use BFCL Multi-Turn and SWE-bench Lite under fixed outer harnesses. The important causal comparison is the same agent/environment with only the model endpoint changed from `MODEL` to `MODEL-pigs`.
