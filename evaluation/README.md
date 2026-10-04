# PIGS evaluation

正式评测现在按 **一个 benchmark 一个 Python 入口** 组织。benchmark 的数据构造、agent loop 和评分尽量交给官方 harness；本目录只负责 Base/PIGS 配对、模型矩阵、后台运行、进度记录和统一实验元数据。

> 当前 `evaluation/` 仍是根仓库的 untracked 目录；在正式实验前应纳入 Git。`__pycache__/` 已由根 `.gitignore` 排除。

## 正式矩阵

| Part | Benchmark | 正式范围 | 评测 Agent / Harness | 入口 |
|---|---|---:|---|---|
| Control | GSM8K | full 1319 | EvalScope，无 agent | `run_gsm8k.py` |
| Control | IFEval | full 541 | EvalScope + IFEval scorer，无 agent | `run_ifeval.py` |
| Agent | BFCL V4 Multi-Turn | full 800 | BFCL 官方 multi-turn interaction/execution loop + scorer | `run_bfcl_multiturn.py` |
| Agent | SWE-bench Lite | official Lite test 300 | mini-SWE-agent + SWE-bench 官方 Docker scorer | `run_swebench_lite.py` |

旧的 MMLU-Pro / BFCL Core 统一矩阵已不再是正式 runner。`run_evaluation.py` 仅保留为入口索引，避免误跑旧方案。

## 多模型

所有四个入口都接受任意数量的 base model：

```bash
--models mimo-v2.6-flash deepseek-v4.1-flash MODEL_3 ...
```

也可给输出目录使用稳定短标签：

```bash
--models mimo=mimo-v2.6-flash deepseek=deepseek-v4.1-flash
```

每个 base model 自动形成两臂：

```text
MODEL
MODEL-pigs
```

`--arms base pigs` 可用于开发时只跑某一臂。

### 两层并发

- `--model-workers N`：同时运行多少条**模型管线**。每个模型内部仍保持 `base → pigs` 顺序，避免同一模型两臂直接互相竞争吞吐。`0` 表示所有传入模型同时运行。
- `--sample-workers N`：该 benchmark harness 内部同时处理多少个样本/task。

例如两个模型同时跑、每个 job 内 4 并发：

```bash
python run_gsm8k.py \
  --models mimo-v2.6-flash deepseek-v4.1-flash \
  --model-workers 2 \
  --sample-workers 4
```

正式并发值仍应先做 calibration 后冻结，并保证同一模型 Base/PIGS 使用相同配置。

## 后台运行

每个入口都原生支持 `--background`：

```bash
python run_ifeval.py \
  --models mimo-v2.6-flash deepseek-v4.1-flash \
  --model-workers 2 \
  --sample-workers 4 \
  --background
```

命令会立即打印：

```text
background_pid=...
run_dir=...
progress_file=.../progress.txt
launcher_log=.../_launch/<run_id>.log
```

进度仍然写文件，不依赖终端进度条：

```bash
watch -n 2 cat <run_dir>/progress.txt
```

每个 run 同时保存：

- `progress.txt`：人类可读总体进度；
- `progress.json`：机器可读总体进度；
- `run_manifest.json`：本次调用的模型、两臂、并发、harness commit、PIGS commit 等冻结信息；
- `<model>/model_manifest.json`：单个模型自己的两臂、session ID 和模型 ID；
- `<model>/<arm>/runner.log`：该模型/arm 的完整 harness 日志；
- `<model>/<arm>/pigs-http/`：按 `x-opencode-session` 从 PIGS 全局运行时日志中归档出的该模型/arm HTTP + orchestration 日志。

因此即使今天只跑 MiMo、之后另开一次 run 再跑 DeepSeek，两次实验的正式日志和 manifest 也是各自独立的。PIGS 的根 `logs/http` 只作为运行时日志源，不再作为某个实验结果的唯一日志位置。mini-proxy 自身的 `logs/proxy.log*` 仍是共享 transport-service 日志；它当前没有足够稳定的 session 关联信息，暂不伪装成可精确拆分的 per-model 正式日志。

## 思考强度

思考强度是正式实验变量，由 **evaluation harness 显式设置**，不依赖也不修改 mini-proxy 配置。四个正式入口统一提供：

```bash
--thinking-effort low
```

默认值当前就是 `low`。它会分别通过各 benchmark 的兼容层传入模型请求：

- GSM8K / IFEval：EvalScope `generation_config.reasoning_effort`；
- BFCL V4 Multi-Turn：本目录的 BFCL adapter 在官方 OpenAI handler 请求上附加 `reasoning_effort`；
- SWE-bench Lite：mini-SWE-agent `model_kwargs.reasoning_effort`。

PIGS 的 phase 请求从原始请求克隆，因此 Base 与 PIGS 的 Pre / Executor / Post 都继承相同的 reasoning effort。

每次 run 会把该值写入：

- `run_manifest.json`；
- `<model>/model_manifest.json`；
- `<model>/<arm>/runner.log` 的 `THINKING_EFFORT ...` 行。

后续若运行第二个 reasoning-budget 条件，只需在评测命令上改为例如 `--thinking-effort high` 或 `--thinking-effort xhigh`；不需要修改 mini-proxy。正式比较时，同一条件下 Base/PIGS 必须使用相同 effort。

## 输出长度

旧 runner 曾显式设置：

```text
max_tokens = 8192
```

这一限制已经删除。新 runner **不向模型请求主动设置 `max_tokens` / `max_output_tokens` / `max_completion_tokens`**，由目标模型/provider 的原生输出上限决定。

这样评测层不会再人为制造 `8192` 截断。若 provider 自身存在硬上限，manifest 与响应中的 `finish_reason` 仍应记录并单独分析。

## PIGS / mini-proxy 链路

默认模型 API base 为：

```text
http://127.0.0.1:3927
```

对应当前本地链路：

```text
benchmark harness
      ↓
PIGS :3927
      ↓
mini-proxy :7946
      ↓
provider
```

Base 和 PIGS 两臂都经过同一个 PIGS HTTP proxy 和同一个 mini-proxy；区别仅在 model 是否带 `-pigs` 后缀，因此 mini-proxy 的重试/传输能力不会成为 arm 间额外变量。

## 各数据集运行

### GSM8K

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/run_gsm8k.py --background
```

开发 smoke test 可以用 `--limit N`；正式论文结果不得使用人为 limit。

### IFEval

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/run_ifeval.py --background
```

同样仅开发阶段允许 `--limit N`。

### BFCL V4 Multi-Turn

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/run_bfcl_multiturn.py --background
```

固定使用 BFCL 官方 `multi_turn` category：

- `multi_turn_base`
- `multi_turn_miss_func`
- `multi_turn_miss_param`
- `multi_turn_long_context`

共 800 题。`bfcl_official_adapter.py` 只负责把任意 OpenAI-compatible 模型临时注册到官方 handler；不修改 BFCL 的 agent loop、工具环境或 scorer。

### SWE-bench Lite

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/run_swebench_lite.py --background
```

固定使用官方 Lite `test` 300 题。流程为：

```text
mini-SWE-agent inference
→ inference/preds.json
→ SWE-bench official Docker evaluator
→ resolved / unresolved
```

本地环境位于：

```text
/root/pigs-eval/src/SWE-bench
/root/pigs-eval/src/mini-swe-agent
/root/pigs-eval/swebench-venv
```

## Dry run

四个入口都可只生成 manifest / progress 而不请求模型：

```bash
python run_bfcl_multiturn.py \
  --models mimo-v2.6-flash deepseek-v4.1-flash \
  --model-workers 2 \
  --sample-workers 4 \
  --dry-run
```

## 统计

控制 benchmark 继续由 `summarize_evaluation.py` 读取 EvalScope 产物。它现在按 `dataset + model + arm` 分开统计，并分别计算每个模型的 `PIGS - Base` 差值。

Agent benchmark 的 headline metric 以各自官方 scorer 的输出为准；后续可以再增加统一汇总层，但不能替代官方 scorer。

## 统计脚本

新的测试结构配套两个统计入口。

### 单个 run：`summarize_evaluation.py`

适配四个正式 benchmark，并且可在实验运行中执行：

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/summarize_evaluation.py \
  /root/pigs-eval/outputs/formal/gsm8k/deepseek-gsm8k-low-c4-20261004
```

运行中使用 `progress.json` 的实时状态。尚未执行的样本不会被计为 failure；只有 job 进入 `done` / `failed` 后才计算最终 missing。

输出：

```text
<run>/summary.json   # 完整机器可读统计
<run>/summary.md     # 人类可读报告
<run>/summary.csv    # 每个 model/arm 一行，方便后续分析
```

统计内容包括：

- 官方 benchmark metric；
- fixed benchmark denominator（适用时）；
- prediction / malformed / execution failure / final missing；
- latency、TTFT、TPOT；
- endpoint-reported input/output/reasoning/cache token；
- PIGS Simple Path / Full Path / Pig 数 / replan / tool call；
- Base ↔ PIGS task-level paired comparison；
- fixes / breaks；
- 二元指标的 exact McNemar p-value。

各 benchmark 的 headline metric 直接读取官方 scorer：

```text
GSM8K              accuracy
IFEval             prompt_level_strict
BFCL Multi-Turn    multi_turn_overall
SWE-bench Lite     resolved_rate
```

统计脚本不自行重新判题。

BFCL 同时读取官方四个 Multi-Turn category score；SWE-bench 同时读取官方 `results.json`、resolved IDs，以及 mini-SWE-agent trajectory 的 API-call / exit-status 等信息。

### 多个独立 run：`summarize_runs.py`

用于模型分开跑、或 reasoning effort 分批跑之后统一生成论文表。例如：

```bash
python /mnt/d/AIWorkSpace/pigs/evaluation/summarize_runs.py \
  /root/pigs-eval/outputs/formal/gsm8k/deepseek-gsm8k-low-c4-20261004 \
  /root/pigs-eval/outputs/formal/gsm8k/<future-mimo-run> \
  --output-dir /root/pigs-eval/outputs/paper-summary
```

输出：

```text
aggregate.json
aggregate.md
comparisons.csv
```

表格按 `dataset + thinking_effort + model` 保留独立条件，因此可以直接同时比较例如 DeepSeek-low、MiMo-low、DeepSeek-high，而不会混淆不同 run。

所有 benchmark runner 在正式 job 完成后都会自动调用单-run 统计器；也可以在运行中手工重复调用来获取 live snapshot。
