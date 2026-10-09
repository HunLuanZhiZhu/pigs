# GSM8K 双模型易错题开发实验（2026-10-08）

## 目标与边界

开发目标是让同一套正式 PIGS Prompt 在 DeepSeek V4.1 Flash 与 Muse Spark 1.3 Contributor 的完整 GSM8K 上各自不低于自己的 Base。允许由不同模型的非重合失误分别启发 Prompt 改动；一项修改只改善一个模型也可以，只要另一个模型不发生整体退化。

**不允许针对题号、标准答案或 GSM8K 常见题型添加特判。** 对标准答案与题面、参考推理不一致的情况保留备注，不以提升这些题的得分为优化目的。Agent 工具能力与自主完成任务的表现还需要 BFCL / 其他真实 Agent 任务的独立验证；GSM8K 全量持平不是 Agent 无损的证明。

## 现有历史数据

- DeepSeek Base: 1285 / 1318 完成（第 675 题缺失；固定分母 1285 / 1319）
- DeepSeek v5: 1289 / 1319
- DeepSeek v6: 1282 / 1319
- Muse Base: 1293 / 1319
- Muse v5: 1286 / 1319
- Muse v6: 1293 / 1319

## 易错题集

生成脚本：`evaluation/build_gsm8k_hardsets.py`，无需模型 API。生成位置：`/root/pigs-eval/outputs/diagnostic/gsm8k-hardsets-20261008/`。

- `ds_v4_hard.jsonl`，44 道（Base/v5/v6 中至少一次错误）
- `muse_hard.jsonl`，37 道
- 两者交集 26 道，去重并集 55 道
- `shared_correct_controls.jsonl`，60 道历史全部版本正确的固定种子对照样本
- `manifest.json` 和两个 CSV 索引

JSONL 包含题号、模型、原始 EvalScope 输入、题面、目标、原始回答、抽取答案、得分、参考推理、模型专属退化/修复标记、可疑参考标记。数据集是已见过错误的**开发集**，不得作为新独立测试集报告统计显著性。

## Prompt 试验

上一版 Pre 中英文快照：
`evaluation/prompt_trials/gsm8k_20261008/v6_pre_user_{zh,en}.txt`。

第 1 个候选（v7）：仅变动 Simple Path 结束前的核验句，Pre 五问、分流规则、Executor/Post 与代理结构保持不变。

中文：
> 执行完成后，以原任务而非中间复述为准，核对关键事实、条件及其相互关系是否准确采用，结果是否覆盖要求的对象、范围和输出形式；发现遗漏、误读或不符时先修正再结束，不增加或改变任务要求。

英文：
> After execution, verify the result against the original task rather than an intermediate paraphrase: check that the relevant facts, conditions, and their relationships were used accurately, and that the result covers the requested subject, scope, and output form. Correct any omission, misreading, or mismatch before finishing, without adding or changing task requirements.

两项独立的通用关注点：避免 DeepSeek 遗漏/自行改写原始条件；避免 Muse 只返回中间量或错误对象/范围/输出形式。不是模型名条件分支。

Rust 单元测试 55/55 通过；编译新代理服务 `127.0.0.1:3932`。

## 验证计划与实验记录

诊断脚本：`evaluation/diagnose_dual_gsm8k_hardsets.py`，支持断点续跑、流式 Chat/Responses、模型隔离 session、文件进度。

当前定向实验：`/root/pigs-eval/outputs/diagnostic/dual-prompt-v7-20261008/focused/`。选取两个模型历史 v6 相对 Base/v5 退化、历史修复样本，以及每模型 12 道正确对照题，总计 58 请求。结果未完成前不得启动两个模型的全量实验。对照题保证不了 Agent 泛化，后续应增加 BFCL 开发验证。上游 OpenCode Go/mini-proxy 近期有网络错误与重试，完成时需要确认请求有效数量，避免将传输故障记为模型正确性错误。

v7 focused 结果已完成（58/58 有效）：
- DeepSeek 32 题：v6 19 题正确，v7 26 题正确，净 +7；修复 87、107、184、424、552、796、951、955、1059，破坏 652、1035；12 个正确对照均保留。
- Muse 26 题：v6 23 题正确，v7 21 题正确，净 -2；修复 1288，破坏 652、1016、1176；12 个正确对照均保留。
- 三个 Muse 退化集中在价格/百分比等条件的解释基准；不能确定是核验阶段主动重释还是首次理解改变。v7 不通过双模型门槛。

v8 候选仅更换 Pre 最终核验句，保留五问、分流、Executor/Post 及代码：

中文：
> 执行完成后，对照原任务明确给出的事实、条件和输出要求核对结果；发现确切的遗漏、误读或错误时先修正再结束，但不要仅因核验而引入新假设或改换原本合理的理解。

英文：
> After execution, check the result against the facts, conditions, and output requirements explicitly stated in the original task. Correct identifiable omissions, misreadings, or errors before finishing, but do not introduce new assumptions or reinterpret otherwise reasonable readings merely for verification.

目的：保留 DeepSeek 需要的事实/条件核对，而把 Muse 的过度重新解释风险压低。注意：这是待实验证实的假说，不是已证实的模型内部因果。

v8 focused 实验已启动：`/root/pigs-eval/outputs/diagnostic/dual-prompt-v8-20261008/focused/`，独立代理端口 `127.0.0.1:3933`，58 道请求；尚未通过双模型门槛，也未启动全量实验。

按阶段仅保留同时满足双模型非退化门槛的通用 Prompt 改动；候选效果不好时回滚，不添加数学题专项规则。
