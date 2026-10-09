# DeepSeek v4.1 Flash — SWE-bench Lite Base（2026-10-08）

## 最终官方评分（2026-10-08 19:24:46 +0800 完成）

**官方 resolved_rate = 284/300 = 94.67%。** 官方评分器已完成，完整结果文件：
`/root/pigs-eval/outputs/formal/swebench_lite/deepseek-swebench-lite-base-current-low-c4-20261008/deepseek/base/scoring/logs/evaluation/deepseek-swebench-lite-base-current-low-c4-20261008-deepseek-base-merged300/results.json`。

- 300 submitted / 298 nonempty patched instances completed / 284 resolved / 14 nonempty unresolved / 2 empty patches / 0 infrastructure failures / 0 scorer errors / 0 unstopped containers。
- **人工终止的初次 runner 原始状态依然是 `failed`，但合并后 300 题的官方评分已成功完成。** `summary.md` 的 `Status: failed` 指第一次推理进程，不是官方评分失败；不可混淆。
- `sympy__sympy-19007` 为单独用 60 秒命令超时补跑的实例，在完整官方评分中属于 resolved；原本 299 道使用 1,000,000 秒超时，故结果是两种超时条件的组合，不应称作完全同条件的 300 道一次性实验。

### 未解决 16 道

14 个非空补丁失败实例：
`astropy__astropy-14182`、`django__django-11815`、`django__django-11905`、`django__django-12308`、`django__django-13660`、`django__django-15400`、`matplotlib__matplotlib-22711`、`mwaskom__seaborn-3407`、`psf__requests-2317`、`psf__requests-2674`、`psf__requests-3362`、`sphinx-doc__sphinx-8282`、`sympy__sympy-14024`、`sympy__sympy-24102`。

其中按测试报告分解为：8 道仅 FAIL_TO_PASS 未全部通过、4 道仅 PASS_TO_PASS 回归、1 道两者皆有、1 道补丁无法应用（`django__django-11905`）。另外 2 道空补丁见下文。Requests 的 3 个失败都有回归测试问题，且涉及网络连接/超时测试，不能在没有独立复核时认定一定是模型引入的逻辑错误或一定是环境波动。

### 项目维度 resolved（总分母 300）

| Repo | resolved / total |
|---|---:|
| Django | 109/114 |
| SymPy | 74/77 |
| Matplotlib | 22/23 |
| scikit-learn | 23/23 |
| pytest | 16/17 |
| Sphinx | 15/16 |
| Astropy | 5/6 |
| Requests | 3/6 |
| Pylint | 6/6 |
| PyData | 5/5 |
| Seaborn | 3/4 |
| Pallets | 3/3 |

**结论边界：** 本轮仅 DeepSeek **Base**，没有 Full PIGS 配对实验，不能得出 PIGS 的增益。由于该公开基准上的 Base resolved rate 极高，用于论文前建议进行模型后端身份、测试环境复现、污染/记忆化可能性以及独立运行的复核。

---


## 状态与范围

- 模型：`deepseek-v4.1-flash`，Base（不经过 PIGS 编排），`reasoning_effort=low`。
- 数据集：官方 SWE-bench Lite test，300 个实例。
- 初次推理：299 个实例完成，最后一个 `sympy__sympy-19007` 执行 `grep -R ... /` 卡住，在人工终止之后停止；初次命令超时配置为 1,000,000 秒。
- 只重跑了 `sympy__sympy-19007` 一题：2026-10-08 18:24:28 +0800 完成，退出状态 `Submitted`，单条命令超时为 60 秒，生成非空补丁。
- 合并预测：独立补跑的补丁、trajectory 和 exit status 已合入初次推理的 `inference/`，最终 300 条预测、300 个轨迹。**初次 299 条预测有独立备份** `preds_original_299_20261008.json`；补跑来源和不同 timeout 条件记在 `supplemental_integration_20261008.json`。
- **注意：这是 299 + 1 的补全评测，并非在完全相同环境超时设置下完成的一次性 300 样本推理。** 这点必须在论文数据中披露。
- 整体官方评分现已完成；官方 `results.json` 确认 **284/300 resolved（94.67%）**，298 条非空补丁中 284 条成功、14 条失败；另有 2 条空补丁。0 基础设施失败、0 模糊失败、0 评分器错误。注意这是在原始 299 条的基础上补跑 1 条后得到的合并评测。

## 最后一个样本的独立官方得分（已确认）

- `sympy__sympy-19007`：`resolved=true`，`patch_successfully_applied=true`，`infra_failure=false`。全量 `resolved_ids` 也包含该样本。
- FAIL_TO_PASS：3/3 全部通过；PASS_TO_PASS：26/26 全部通过。
- 单题官方判定文件已移至独立诊断目录 `/root/pigs-eval/outputs/diagnostic/swebench-one-sympy-19007-20261008/official_scoring/logs/evaluation/deepseek-swebench-19007-rerun-official/`，避免汇总脚本误把 1/1 计为整体 300 题的最终正确率。

## 推理统计（300 条）

| 指标 | 数值 |
|---|---:|
| 总预测条数 | 300 |
| 非空补丁 | 298（99.33%） |
| 空补丁 | 2 |
| Submitted | 298 |
| RepeatedFormatError | 2 |
| 平均 API 调用 | 28.42 次/题 |
| 中位 API 调用 | 26 次/题 |
| P95 API 调用 | 53.1 次/题 |
| 平均工具返回数 | 34.31 次/题 |
| 中位工具返回数 | 32 次/题 |
| 轨迹时间均值 | 340.03 秒/题 |
| 轨迹时间中位数 | 209.6 秒/题 |
| 轨迹时间 P95 | 745.37 秒/题 |
| 总输入 token（模型报告） | 177,455,530 |
| 缓存读取输入 token | 162,732,928 |
| 未缓存输入 token（差额） | 14,722,602 |
| 总输出 token | 2,865,635 |
| 其中 reasoning token | 1,685,793 |
| 总 token | 180,321,165 |
| 模型响应中含 usage 的条数 | 8,496 |

以上 token 包含上下文重复发送及缓存命中，不等于计费价格，无法据此简单用单一输入单价估算成本。由于 299 + 1 条是不同时间的请求，应保留这个实验条件差异。

空补丁实例：`pytest-dev__pytest-5103`、`sympy__sympy-16988`（均 `RepeatedFormatError`）。

最长轨迹实例：`pytest-dev__pytest-5103` 约 7812 秒、`sympy__sympy-16988` 约 7776 秒。这两个极端长尾实例均没有最终有效补丁。

独立补跑的 `sympy__sympy-19007`：模型 API 调用 32 次，轨迹内耗时约 159 秒，总报告 token 727,421，最终由官方 scorer 判为 resolved。

## 输出和命令

WSL 运行目录：

```text
/root/pigs-eval/outputs/formal/swebench_lite/deepseek-swebench-lite-base-current-low-c4-20261008/deepseek/base/
```

- 完整预测：`inference/preds.json`
- 初次 299 条备份：`inference/preds_original_299_20261008.json`
- 补跑来源：`inference/supplemental_integration_20261008.json`
- 推理详细统计：`scoring/inference_statistics.json`
- 全量官方 scorer 实时日志：`scoring/scorer.log`
- 全量 scorer PID：`scoring/scorer.pid`
- 单题独立官方 scorer：`/root/pigs-eval/outputs/diagnostic/swebench-one-sympy-19007-20261008/official_scoring/`
- 最终全量官方评分（评分完成后）：`scoring/logs/evaluation/deepseek-swebench-lite-base-current-low-c4-20261008-deepseek-base-merged300/results.json`

评分命令：

```bash
swebench eval lite --predictions <full-300-preds.json> \
  --run-id deepseek-swebench-lite-base-current-low-c4-20261008-deepseek-base-merged300 \
  --workers 4
```

2026-10-08 评分结束后已重新运行 `evaluation/summarize_evaluation.py`，正式 `summary.json`、`summary.md` 和 `summary.csv` 已显示 284/300、94.67%。保留原 runner `failed` 状态，因为当时存在人为终止，不应覆盖其审计记录。

## 全量官方结果（已确认）

| 结果 | 数量 |
|---|---:|
| 官方分母 | 300 |
| Resolved | **284** |
| Unresolved（非空补丁） | 14 |
| Empty patch | 2 |
| Infra failure / ambiguous failure / scoring error | 0 / 0 / 0 |
| 已完成 Docker 测试 | 298 |

分项目 resolved/total：Astropy 5/6，Django 109/114，Matplotlib 22/23，Seaborn 3/4，Pallets 3/3，Requests 3/6，PyData 5/5，Pylint 6/6，Pytest 16/17，scikit-learn 23/23，Sphinx 15/16，SymPy 74/77。

14 个 Unresolved ID：`astropy__astropy-14182`、`django__django-11815`、`django__django-11905`、`django__django-12308`、`django__django-13660`、`django__django-15400`、`matplotlib__matplotlib-22711`、`mwaskom__seaborn-3407`、`psf__requests-2317`、`psf__requests-2674`、`psf__requests-3362`、`sphinx-doc__sphinx-8282`、`sympy__sympy-14024`、`sympy__sympy-24102`。

已检查 300 条预测与官方 resolved/unresolved/empty 的划分完全覆盖，没有缺失或重复。14 个 Unresolved 中，`django__django-11905` 补丁未能应用，其他 13 个已应用但测试不全部通过。
