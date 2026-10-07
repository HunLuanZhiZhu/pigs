# Reviewer Memory

No prior auto-review round. Earlier integrity/result-to-claim reports are separate actual artifacts; they are not this reviewer's verdict.

## Round 1 — Score: 7/10; verdict almost

### Raw Reviewer Response (verbatim)
## Score: 7/10
## Verdict: almost

The bounded package is draftable now, but not submission-ready for the strongest causal/system wording. The main predeclared comparison is real, scoped, and robust across two model coders and failure handling. The main blockers are validity of model-assisted semantic labels, limited generalization, and thin client/system evidence—not execution failure.

## Verified claims

- **Design/execution intact.** 12 synthetic deterministic tasks × 2 placements (api/app) × 4 handoffs (I/R/T0/T1) × 3 replicates = 288 planned; 286 completed; 2 failed and retained. `formal_main_analysis.json` cells sum to 286 completed, 2 failures; all 12 task blocks complete.
- **Main attribution result.** Root-final labels: 17 error, 108 explicit_correct, 161 unscorable. Predeclared primary I−R error difference = −0.236111, exact p = 0.0078125, Holm = 0.03125, task-bootstrap 95% CI [−0.347222, −0.125]. Verified from `formal_main_analysis.json`.
- **Robustness across initial coders.** Independent DeepSeek initial labels: I−R = −0.3125, Holm = 0.00390625. Both coders are co-equal model-assisted, not human gold.
- **Failure-inclusive sensitivity.** I−R = −0.25, Holm = 0.015625. Direction survives treating failures as errors.
- **External complete adjudication.** Reported final external labels: 17/109/160; primary I−R remains −0.236111, Holm = 0.03125. The two extra external divergences change two descriptive cells, not the error metric.
- **Coder agreement.** 267/286 = 93.36% agreement, Cohen κ = 0.878922; 19 disagreements retained. Initial class counts differ (root 17/111/158; DeepSeek 22/110/154).
- **T0/T1 diagnostic.** T1−T0 primary error difference = 0, p = 1; I−T1 = 0, p = 1; api−app = 0.006944, p = 1. These are nulls, not equivalence. T1−T0 post-read difference = −0.9375, exploratory p = 0.001953125, CI [−1.333333, −0.583333].
- **Access probe.** 215/215 eligible 512-token probes exactly recovered all three nonces; nonce positions are late (~0.764–0.900). This supports content extractability under the probe, not natural Post attention or unique mediation.
- **Metering.** 1276 main upstream rounds = Pre 289 + Executor 578 + Post 409; 215 diagnostics; 1491 formal total; 1546 global including sanity/probe/B4. Audit-reported from `PHASE_METERING.json`; I did not directly read that JSON.
- **Failures retained.** The two failures have full usage retained; completed runs have strict three-line contract 0/286 and correct final DECISION 286/286.
- **Client constraints.** B4: two OpenCode built-in-plan sessions; ordinary 2 upstream rounds, pigs 5; native SSE, tool-ID pause/resume, no final-tool replay, marker filtering pass. Initial inputs differ in model and five skill paths; pigs adds an extra glob. No config-free, byte-equivalent, or universal-client claim is supported. `strict_read_only_tool_request_satisfied=false`.

## False or unverified claims

- **Human gold / human evaluation:** false. Both initial coders, final root adjudication, and external adjudication are model-assisted. External adjudication saw both initial labels and full source; not blind.
- **Independent blind adjudication:** false/unsupported. Root final adjudicator is same OpenAI family as one initial coder and saw mapping/sensitivity; external is DeepSeek-family and non-blind.
- **Universal or config-free client compatibility:** false. B4 shows input differences and only two sessions, one task.
- **T1−T0 or I−T1 equivalence:** false. Null results do not establish equivalence.
- **Diagnostics prove natural attention or unique mediator:** false. They show probe access only.
- **Cost/billing savings from lower total input:** unsupported. No price/billing record; cache/uncached must be reported separately; exploratory only.
- **Correct DECISION 286/286 as quality evidence:** false. It does not imply correct reasoning or client contract; strict three-line contract is 0/286.
- **“run03 confirm” as scientific acceptance:** not established. Status documents are layered/historical; actual claim status remains partial and scoped.
- **Unverified here:** raw `.aris` traces, `FINAL_ATTRIBUTION_ANNOTATIONS.json`, `ROOT_ADJUDICATION_DECISIONS.json`, `PHASE_METERING.json`, raw provider captures, Rust source, and SHA-256 freeze recomputation. The audit reports PASS/warn, but that is process-level integrity, not scientific validation.

## Ranked critical/major weaknesses

1. **Semantic outcome labels lack human validation.** The primary variable is whether Post made an explicit counterfactual attribution claim. The boundary between counterfactual, visibility-only, and interpretive claims is semantic. Two model coders and a model adjudicator are not human gold. This is the strongest threat to C2.
2. **Low statistical power and narrow generalization.** 12 synthetic tasks, one model (`deepseek-v4.1-flash`), n=12 task-level units. The effect is significant but estimated on a small fixed task set; no cross-model or natural-task evidence.
3. **Compound handoff manipulation limits mechanism attribution.** I vs R changes native structure, control-marker handling, and tool-evidence availability. R is a policy-level contrast, not a pure structural intervention. T0/T1 title controls do not show a main-error effect, so native-structure superiority over a lossless text payload is not established.
4. **Missing-data handling needs sharper reporting.** Primary error contrasts use completed runs; one api-T0 and one app-R failure are excluded from process denominators, while failure-as-error sensitivity is provided. The task-level cell means for affected cells use 2 rather than 3 runs. This is disclosed but should be explicit per cell.
5. **Client/system evidence is thin.** B4 is two sessions on one task, with model and skill-path input differences. It supports protocol shape in a registered configuration, not general middleware portability.
6. **Adjudication independence is weak.** External adjudication resolved all 19 disagreements but was not blind and was selected by the caller; identity is caller-declared/unverified.
7. **Diagnostics have narrow scope.** 215/215 exact nonce recovery is a validity screen. Late nonce positions and probe-only access mean no natural-use or mediation claim.

## Minimum necessary fixes

1. **Add human validation or downgrade C2.** Have at least one human coder, blind to condition, code a stratified sample: all 17 error runs, a sample of explicit_correct and unscorable runs across I/R/T0/T1 and placements. Report human–model agreement and adjudicate disagreements. If impossible, label the primary outcome as model-assisted and avoid strong “faithfulness”/“attribution correctness” language.
2. **Use a canonical claim-status table.** Reconcile `NARRATIVE_REPORT`, `CLAIMS_REVISED`, and `CLAIMS_FROM_RESULTS` into one current table: claim, source, status, scope, what would falsify. Keep historical FAIL/supplement/confirm traces as history, not current verdicts.
3. **Report missing data explicitly.** For each cell, report planned/completed/failed and n used in each metric. Keep failure-as-error sensitivity. For affected tasks, show primary-error difference with failed runs excluded and with failed runs as error side by side.
4. **Scope C1 to the actual client evidence.** Write B4 as two-session prototype integration under a registered configuration. Include the initial input diffs and extra glob. Remove any implication of byte-equivalence, config-free use, or universal client compatibility.
5. **Scope C4 to what T0/T1 support.** State that T0/T1 payload equality is within-trace construction; arms are separately generated traces. Do not infer title effect on primary error; keep post-read as exploratory.
6. **Keep diagnostics and C8 exploratory.** Present nonce recovery as access screening only. Keep cost/round effects exploratory, with no price or billing inference.

## Memory update

This is a real but bounded system/behavioral package. The main predeclared I−R attribution-error effect is present and survives two model coders and failure-as-error handling: root I−R −0.236111, Holm 0.03125; DeepSeek initial I−R −0.3125, Holm 0.00390625; external complete adjudication keeps the error metric at 17. The strongest scientific gap is not execution but label validity: all semantic labels are model-assisted, no human gold; external adjudication saw both labels and was not blind. The system/client evidence is only two OpenCode sessions on one task with input differences, so C1 stays prototype/protocol-shape. T0/T1 and nonce diagnostics are controls/screens, not mechanism proof. Failures, invalid scoring attempts, exploratory C8/T1-T0-read results, and 0/286 strict output contract must remain visible. The package can be drafted as a limited COLING-style empirical/system paper, but strong causal/system claims need human validation and tighter missing-data/client scoping.
