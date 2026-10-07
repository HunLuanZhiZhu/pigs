# PIGS 正式评测初步方案

> 状态：**初步方案 / runner 已按该结构实现，正式参数尚未冻结**  
> 当前已拆分为 GSM8K、IFEval、BFCL V4 Multi-Turn、SWE-bench Lite 四个独立入口；正式模型版本、并发参数和 provider snapshot 仍需在真实实验前冻结。

## 1. 评测目标

正式评测分为两个互补部分，避免只用单轮问答 benchmark 来证明一个面向 agentic inference 的系统。

### Part A — Control / Non-regression

目标不是要求 PIGS 在所有普通任务上显著提升，而是验证：

> **加入 PIGS 后，普通单轮推理与复杂指令遵循能力不能出现明显退化。**

这部分使用传统、无需 agent harness 的完整 benchmark。

### Part B — Agentic / Long-horizon

目标是验证 PIGS 的完整 `Pre → Executor → Post` 编排在多轮、工具驱动、需要环境反馈与持续修正的任务中，是否提高 end-to-end task success。

核心实验原则：

> **保持 benchmark 官方 agent / harness、工具、环境、任务和评分器不变，只改变底层模型调用是 direct inference 还是 PIGS inference。**

因此 agent 部分优先选择具有官方评测 harness / agent loop 的 benchmark。

---

## 2. 模型

正式实验计划使用两个不同模型家族：

- **MiMo-V2.6-Flash**
- **DeepSeek-V4.1-Flash**

每个模型都进行成对比较：

```text
MiMo-V2.6-Flash
vs
MiMo-V2.6-Flash + PIGS

DeepSeek-V4.1-Flash
vs
DeepSeek-V4.1-Flash + PIGS
```

正式实验不使用 preview 模型作为主结果模型。

### 版本冻结要求

正式 run 不能只记录论文展示名称，还必须在 manifest 中冻结并记录：

- provider；
- 实际 API model identifier；
- 实验日期；
- provider 可提供的 snapshot/version 信息；
- 返回响应中的 model/version 信息（若有）；
- decoding / generation 配置；
- PIGS commit；
- benchmark harness commit / version。

如果 provider 只提供滚动 alias，则论文和实验记录必须明确 API access date，不能把 alias 当作永远固定的 checkpoint。

---

## 3. Benchmark 矩阵

### Part A — Control / Non-regression

| Benchmark | 范围 | Agent / Harness | 主要目的 |
|---|---:|---|---|
| **GSM8K** | 官方完整 split，1319 | 无 agent；EvalScope 直接调用模型并评分 | 基础推理能力不退化 |
| **IFEval** | 官方完整 split，541 | 无 agent；EvalScope + IFEval rule-based scorer | 复杂约束与指令遵循不退化 |

这两项的论文定位是 **control benchmarks**，不是证明 PIGS long-horizon 能力的 headline 结果。

### Part B — Agentic / Long-horizon

| Benchmark | 范围 | 官方评测 Agent / Harness | 主要目的 |
|---|---:|---|---|
| **BFCL V4 Multi-Turn** | 官方完整 Multi-Turn，800 | BFCL 官方 multi-turn interaction / execution loop + 官方 scorer | 多轮工具调用、环境 state、缺失信息处理、持续交互 |
| **SWE-bench Lite** | 官方完整 Lite variant，300 | mini-SWE-agent 做 inference；SWE-bench 官方 Docker harness 做 execution-based scoring | 长程代码搜索、修改、测试、失败反馈与修正 |

### 当前明确不纳入核心矩阵

#### BFCL V4 Agentic

暂不纳入。其 Web Search 部分需要额外搜索 API / 外部搜索基础设施，会引入额外费用、网络稳定性和第三方服务变量，不符合当前希望保持评测环境干净的目标。

#### MMLU-Pro

不纳入当前正式核心矩阵。其 12032 题主要测知识与推理，单并发成本很高，但和 PIGS 的 agentic / long-horizon claim 对齐度低。

#### GAIA

暂不纳入第一版核心矩阵。GAIA 任务本身很适合 general agent，但 agent scaffold 的标准化程度不如 BFCL 官方 interaction loop 和 SWE-bench + mini-SWE-agent 这一组合。后续可以作为扩展实验重新评估。

#### SWE-bench Verified

作为升级候选保留。Verified 500 的任务质量和复杂度更适合 long-horizon claim，但第一版正式实验优先使用 **官方 SWE-bench Lite 300 全量** 控制成本；Lite 是官方 variant，不是我们人为抽取的子集。

---

## 4. 禁止人为缩减 benchmark

正式结果遵循以下原则：

- 使用完整官方 split；或
- 使用官方正式发布、可独立引用和比较的 variant/category。

**不允许为了降低成本自行随机抽 30/50/100 题作为正式结果。**

开发、连通性检查、concurrency calibration 可以使用少量 smoke samples，但这些结果不能进入正式 benchmark 表格。

---

## 5. Agent 实验的因果比较

Agent benchmark 中，必须保持外层 agent scaffold 不变。

### BFCL V4 Multi-Turn

```text
BFCL task
   ↓
BFCL 官方 multi-turn handler
   ↓
model endpoint
   ↓
BFCL simulated tools / environment
   ↓
observation
   ↓
BFCL handler 继续下一轮
   ↓
BFCL official scorer
```

比较时仅替换：

```text
model
vs
model-pigs
```

不得为了 PIGS 单独修改 BFCL agent policy、工具环境或任务流程。

### SWE-bench Lite

```text
SWE-bench Lite task
   ↓
mini-SWE-agent
   ↓
model endpoint
   ↓
repo / shell / tests
   ↓
observation
   ↓
mini-SWE-agent 继续 trajectory
   ↓
最终 patch
   ↓
SWE-bench official Docker harness
   ↓
resolved / not resolved
```

比较时同样仅替换：

```text
model
vs
model-pigs
```

mini-SWE-agent 配置、step budget、shell timeout、repo environment 和 scorer 必须在 paired arms 中一致。

---

## 6. 配置公平性

### 同一模型内部

Base 与 PIGS 必须完全一致：

- provider；
- model snapshot / alias；
- temperature / top_p；
- max output tokens；
- tool definitions；
- harness；
- task order / seed；
- concurrency；
- timeout；
- retry policy；
- 外部环境。

唯一主要变量是是否经过 PIGS。

### 不同模型之间

MiMo 与 DeepSeek 不要求为了“表面统一”强行使用完全相同的 decoding 参数。

若模型官方对 agent workload 有明确推荐设置，可以分别采用各自合理的 operating point；但每个模型自己的 Base/PIGS 两臂必须完全一致。

主要统计比较是：

```text
MiMo → MiMo + PIGS
DeepSeek → DeepSeek + PIGS
```

而不是把 MiMo 与 DeepSeek 本身作为主要 causal comparison。

---

## 6.1 Runner 实现约束

当前正式 runner 已实现以下结构：

- `run_gsm8k.py`、`run_ifeval.py`、`run_bfcl_multiturn.py`、`run_swebench_lite.py` 四个独立入口；
- `--models` 接受任意数量模型，PIGS arm 自动使用 `MODEL-pigs`；
- `--model-workers` 允许多个模型管线并行；同一模型内部 Base/PIGS 顺序执行；
- `--sample-workers` 控制 benchmark 内部并发；
- 每个入口支持 `--background`，并持续写 `progress.txt` / `progress.json`；进度文件区分 RUNNING / RETRYING / ARCHIVING / DONE / FAILED，并记录首轮成功数、失败数和失败 sample ID；
- GSM8K / IFEval 首轮全部样本结束后，对缺失 prediction 默认补跑 1 次（`--sample-retries 1`），使用 EvalScope sample cache，仅重试失败样本；首轮失败率仍单独保留，不用 retry 掩盖 first-attempt reliability；
- GSM8K / IFEval 的单请求 timeout 为 1800 秒；mini-proxy upstream total timeout 同步提高为 1800 秒；
- runner 不再设置 `max_tokens`、`max_output_tokens` 或 `max_completion_tokens`，避免评测层主动截断模型输出。

## 6.2 Reasoning budget

Reasoning effort 作为独立实验变量由 evaluation harness 显式设置，不由 mini-proxy 决定。当前第一轮正式条件默认：

```text
thinking_effort = low
```

所有模型、所有 benchmark，以及同一模型的 Base/PIGS 两臂均使用同一 effort。后续可完整重复同一正式矩阵于 `high` / `xhigh` 条件，研究 PIGS 增益是否随底层 reasoning budget 变化。温度等 sampling 参数无需跨模型强行统一，优先保持各官方 harness / 模型的合理设置；同一模型 Base/PIGS 两臂保持一致即可。

## 7. 并发与运行策略

上一轮 `concurrency = 1` 仅适合作为早期尝试，不作为正式默认值。

正式运行前先做独立的 concurrency calibration，例如：

```text
1 → 2 → 4 → 8
```

使用 smoke tasks 检查：

- API 502 / rate-limit / timeout 比例；
- p50 / p90 latency；
- throughput；
- benchmark outcome 是否因并发明显变化。

正式并发取“没有显著增加错误率的最高稳定档位”。

要求：

- 同一 benchmark、同一模型的 Base/PIGS 两臂使用相同 concurrency；
- 并发校准样本不进入正式结果；
- SWE-bench 优先使用官方 harness 的 worker 并发机制；
- 不为了追求速度引入不受控的 provider overload。

---

## 8. 单并发时间量级（仅用于资源规划）

以下不是最终实验数据，仅用于决定并发和资源预算。

| Benchmark | 单 arm / concurrency=1 的粗略量级 |
|---|---:|
| GSM8K 1319 | 数小时；上一轮 LongCat Base 约 4h44m，PIGS 约 7h35m |
| IFEval 541 | 数小时；上一轮 LongCat Base 约 5h05m |
| BFCL V4 Multi-Turn 800 | 约半天到一天以上，取决于每题工具轮数与 API latency |
| SWE-bench Lite 300 | 约数天量级，必须依赖 worker 并发降低墙钟时间 |

正式运行前应分别用目标模型和目标 harness 做小规模 timing calibration，不把以上估计当作结果。

---

## 9. 主要报告指标

### Control benchmarks

报告：

- benchmark 官方主指标；
- Base vs PIGS 差值；
- execution success / failure；
- paired regression / improvement（样本可对应时）；
- latency；
- endpoint 暴露的 token / cache usage，明确其不是 PIGS 内部所有 phase 的累计真实成本。

Control 部分希望看到的是：

```text
PIGS ≈ Base
```

即普通任务没有明显退化。

### Agent benchmarks

以官方 end-to-end success metric 为主，例如：

- BFCL 官方 Multi-Turn score / task success；
- SWE-bench resolved rate。

同时记录：

- trajectory length / model-call count（harness 可提供时）；
- wall-clock latency；
- tool-call count；
- PIGS phase / path 统计；
- replanning / Post iteration；
- execution failures；
- paired task-level Base ↔ PIGS outcome。

Agent 部分希望检验的是：

```text
PIGS > Base
```

并分析收益是否随任务 trajectory / difficulty 增大而更明显。

---

## 10. 正式 run 的最低可复现记录

每次 formal run 至少冻结：

```text
run_id
benchmark name + official variant/split
benchmark version / commit
agent harness version / commit
model display name
provider
API model identifier / snapshot if available
API access date
generation config
concurrency
retry / timeout policy
PIGS commit
PIGS config
random seed / official task order
per-task output / trajectory location
scorer version
```

Base 和 PIGS 必须能通过 manifest 还原为一一对应的 paired experiment。

---

## 11. 当前实施顺序

1. 保留上一轮 GSM8K / IFEval 结果作为开发阶段诊断资料，不把被重启打断的 run 当作正式结果。
2. 确认 MiMo-V2.6-Flash 与 DeepSeek-V4.1-Flash 的正式 provider、API model identifier 与版本冻结方式。
3. BFCL V4 Multi-Turn 官方完整 harness runner 已接入；真实模型连通性尚待验证。
4. SWE-bench Lite + mini-SWE-agent + 官方 Docker scorer 已接入，本地 gold-patch smoke test 已通过。
5. 为四个 benchmark 做小规模 connectivity / concurrency calibration。
6. 冻结正式配置、PIGS commit、harness commit 和 run manifest。
7. 对两个模型分别执行 Base/PIGS paired formal runs。
8. 生成 control non-regression 表与 agentic long-horizon 主结果表。

---

## 12. 当前实验故事线

正式论文实验希望回答两个不同的问题：

### Q1. PIGS 会不会伤害普通任务？

由 GSM8K + IFEval 回答。

### Q2. PIGS 是否真正提高长程 Agent 能力？

由 BFCL V4 Multi-Turn + SWE-bench Lite 回答。

最终理想但不预设的结果模式是：

```text
Control tasks:
Base ≈ PIGS

Long-horizon agent tasks:
PIGS > Base
```

这比只在大量单轮 QA 上追求平均分提升，更直接对应 PIGS 的设计目标。
