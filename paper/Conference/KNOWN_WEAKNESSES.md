# KNOWN WEAKNESSES — Phase 1
# Atomized prior reviewer concerns with stable IDs

## From Reviewer 1 (Venue A Workshop)

- id: W1
  severity: critical
  type: experiment-coverage
  source: reviewer_1_venue_a
  concern: "The paper presents a system design but no experimental evaluation. There is no evidence that phased orchestration actually improves task completion quality."
  addressability: text-fixable
  recommended_fix: "Add experiments section with ablation study, control marker study, protocol consistency, and cost-benefit analysis. Mark results as pending if evaluation not yet run."
  status: ADDRESSED — experiments.tex and results.tex merged into Conference paper

- id: W2
  severity: major
  type: novelty
  source: reviewer_1_venue_a
  concern: "No quantitative comparison with existing multi-stage reasoning approaches (ReAct, Reflexion, Self-Refine). Unclear whether three-phase structure outperforms simpler approaches."
  addressability: text-fixable
  recommended_fix: "Add ablation conditions (Pre-only, Executor+Post) that isolate the contribution of each phase. Discuss in related work how the ablation design compares to ReAct/Reflexion."
  status: ADDRESSED — Experiment 1 ablation includes conditions C (Pre-only) and D (Executor+Post)

- id: W3
  severity: major
  type: scope
  source: reviewer_1_venue_a
  concern: "Three-protocol consistency claim is made but not verified."
  addressability: text-fixable
  recommended_fix: "Add Experiment 3: three-protocol consistency evaluation."
  status: ADDRESSED — Experiment 3 designed in experiments.tex

- id: W4
  severity: minor
  type: framing
  source: reviewer_1_venue_a
  concern: "Cost overhead of three-phase orchestration not discussed."
  addressability: text-fixable
  recommended_fix: "Add Experiment 4: cost-benefit analysis with token, latency, and API call overhead metrics."
  status: ADDRESSED — Experiment 4 designed in experiments.tex

## From Reviewer 2 (Venue A Workshop)

- id: W5
  severity: critical
  type: experiment-coverage
  source: reviewer_2_venue_a
  concern: "No experiments. The paper says 'future work includes empirical evaluation' — this should be the core of the paper."
  addressability: text-fixable
  recommended_fix: "Merge experimental evaluation into the paper body. Remove 'empirical evaluation' from future work in conclusion."
  status: ADDRESSED — experiments merged; conclusion updated to reference evaluation framework

- id: W6
  severity: major
  type: rigor
  source: reviewer_2_venue_a
  concern: "PIGFAIL failure-recovery mechanism not evaluated. How often does it trigger? Does replanning lead to success?"
  addressability: text-fixable
  recommended_fix: "Add Experiment 2: control marker effectiveness study measuring PIGFAIL trigger rate and replan success rate."
  status: ADDRESSED — Experiment 2 designed in experiments.tex

- id: W7
  severity: minor
  type: scope
  source: reviewer_2_venue_a
  concern: "Single-model assumption not explored. Would different models for different phases change results?"
  addressability: unaddressable-under-constraints
  recommended_fix: "Acknowledge in Discussion/Limitations that multi-model evaluation is left for future work."
  status: ADDRESSED — already in discussion.tex limitations; conclusion lists as future work

- id: W8
  severity: minor
  type: framing
  source: reviewer_2_venue_a
  concern: "Marker robustness: false positive/negative rate not discussed."
  addressability: unaddressable-under-constraints
  recommended_fix: "Acknowledge in Discussion/Limitations that marker robustness is a known limitation; the detection algorithm mitigates but does not eliminate false positives."
  status: ADDRESSED — discussion.tex already has "Marker robustness" limitation paragraph

## From Reviewer 3 (Venue A Workshop)

- id: W9
  severity: critical
  type: experiment-coverage
  source: reviewer_3_venue_a
  concern: "Complete absence of experimental evaluation. Claims about improved task completion quality are unsupported."
  addressability: text-fixable
  recommended_fix: "Add full experimental evaluation section."
  status: ADDRESSED — experiments.tex + results.tex merged

- id: W10
  severity: major
  type: novelty
  source: reviewer_3_venue_a
  concern: "Three-phase structure is essentially Plan-and-Solve + review. Need empirical demonstration that three phases outperform two or one."
  addressability: text-fixable
  recommended_fix: "Ablation study with conditions A (no orchestration), C (Pre-only), D (Executor+Post), B (full) directly addresses this."
  status: ADDRESSED — Experiment 1 ablation conditions cover this comparison

- id: W11
  severity: major
  type: scope
  source: reviewer_3_venue_a
  concern: "Three-protocol consistency claim made but not verified."
  addressability: text-fixable
  recommended_fix: "Add Experiment 3."
  status: ADDRESSED — duplicate of W3; Experiment 3 designed

- id: W12
  severity: minor
  type: experiment-coverage
  source: reviewer_3_venue_a
  concern: "Cost-benefit analysis mentioned as future work but should be included."
  addressability: text-fixable
  recommended_fix: "Add Experiment 4."
  status: ADDRESSED — duplicate of W4; Experiment 4 designed

- id: W13
  severity: minor
  type: framing
  source: reviewer_3_venue_a
  concern: "Conclusion says 'future work includes empirical evaluation' — suggests premature submission."
  addressability: text-fixable
  recommended_fix: "Remove 'empirical evaluation' from future work; reference the evaluation framework instead."
  status: ADDRESSED — conclusion.tex rewritten

## Summary

| Severity | Count | Addressed | Unaddressable |
|---|---|---|---|
| Critical | 3 (W1, W5, W9) | 3 | 0 |
| Major | 4 (W2, W6, W10, W11) | 4 | 0 |
| Minor | 6 (W3, W4, W7, W8, W12, W13) | 4 | 2 |

All critical and major concerns are addressed through the merged experimental sections. The 2 unaddressable concerns (W7: multi-model, W8: marker robustness) are acknowledged in the Discussion/Limitations section and listed as future work in the conclusion — no new experiments needed.
