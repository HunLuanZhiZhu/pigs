# NARRATIVE REPORT: Pigs Phased Orchestration — Experimental Evaluation

## Problem Statement and Core Claim

LLM-based agents typically interact with model providers through stateless API calls. The Pigs system wraps these calls with a Pre→Executor→Post phased orchestration state machine using text control markers (PIGEND/PIGFAIL). The core claim is that **phased orchestration improves task completion quality without excessive cost overhead**, and that **control markers provide an effective failure-recovery mechanism**.

## Method Summary

Pigs is a middleware that intercepts `-pig` model API requests and routes them through three phases:
1. **Pre** (planning): model analyzes the task and formulates a plan
2. **Executor** (execution): model executes the plan with tool calls
3. **Post** (review): model independently reviews the result against the goal

Control markers (PIGEND/PIGFAIL) route between phases. The system preserves protocol-native HTTP requests across three API protocols (OpenAI Chat, Anthropic Messages, OpenAI Responses).

## Key Quantitative Results

**[EXPERIMENTAL RESULTS — TO BE FILLED]**

The following experiments are planned but not yet executed:

### Experiment 1: Phased Orchestration vs. No Orchestration (Ablation)
- Benchmark: SWE-bench Lite (300 tasks) or GAIA (300 tasks)
- Conditions: A (passthrough, no orchestration), B (full Pre→Executor→Post), C (Pre only), D (Executor→Post only)
- Metrics: Task completion rate, average turns, token consumption, latency
- **Results: [PENDING]**

### Experiment 2: Control Marker Effectiveness
- Benchmark: GAIA or AgentBoard
- Conditions: A (with PIGFAIL — failure replan), B (without PIGFAIL — no replan on failure)
- Metrics: Error recovery rate, replan success rate
- **Results: [PENDING]**

### Experiment 3: Three-Protocol Consistency
- Benchmark: BFCL or ToolBench
- Conditions: A (OpenAI Chat), B (Anthropic Messages), C (OpenAI Responses)
- Metrics: Output consistency, tool call accuracy
- **Results: [PENDING]**

### Experiment 4: Cost-Benefit Analysis
- Benchmark: SWE-bench Lite
- Metrics: Per-task token consumption, API calls, latency, success rate
- **Results: [PENDING]**

## Figure/Table Inventory

- Figure 1: Architecture diagram (exists in system-design paper)
- Figure 2: State machine diagram (exists in system-design paper)
- Figure 3: [NEEDED] Ablation comparison bar chart (completion rate by condition)
- Figure 4: [NEEDED] Token consumption comparison
- Figure 5: [NEEDED] Protocol consistency comparison
- Table 1: [NEEDED] Overall results summary
- Table 2: [NEEDED] Per-benchmark breakdown

## Limitations and Remaining Follow-up Items

- No multi-model orchestration (all phases use the same model)
- No formal verification of the state machine
- Marker robustness depends on model instruction-following ability
- Protocol coverage limited to three major protocols
- Benchmark results not yet collected — all experimental data is pending
