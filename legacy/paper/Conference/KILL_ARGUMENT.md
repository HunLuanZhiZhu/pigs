# KILL ARGUMENT — Phase 3: Adversarial Gate
# Date: 2026-07-14
# Protocol: 2-thread Attack-Adjudication (ultra deep-audit tier)
# Assurance level: submission

## Thread 1: Attack (Hostile Reviewer)

### Decomposed Attack Points

**AP1: "Phased orchestration is just prompt chaining rebranded."**
- The Pre→Executor→Post structure is conceptually identical to Plan-and-Solve + review. The paper claims novelty in "externalizing control flow," but ReAct already externalizes acting from reasoning. The distinction between "model decides when to act" (ReAct) and "middleware decides when to advance phase" (Pigs) is a matter of degree, not kind.
- Severity if unresolved: **major**

**AP2: "No experimental evidence supports any claim."**
- The paper has an experimental framework but zero results. All tables show "---". The abstract says "results pending." This makes the contribution claims unverifiable. A conference paper without results is incomplete.
- Severity if unresolved: **critical** (but acknowledged by authors — results explicitly pending)

**AP3: "Text markers are fragile and untested."**
- The paper acknowledges marker robustness as a limitation but provides no false positive/negative rates. If the model outputs PIGEND in prose, the detection algorithm may fail. The "last non-empty line only" rule is a heuristic, not a guarantee.
- Severity if unresolved: **major**

**AP4: "HTTP loopback is unnecessary overhead."**
- Phase subrequests re-enter the proxy via HTTP, adding latency. The paper claims this ensures "transparent feature inheritance," but the same could be achieved with in-process dispatch + shared middleware pipeline. The HTTP overhead is unjustified for a single-process system.
- Severity if unresolved: **minor** (design choice, not a correctness issue)

**AP5: "The 3-protocol claim is unverifiable without results."**
- The paper claims protocol-native preservation across 3 protocols, but Experiment 3 has no results. The design is sound in theory, but edge cases (e.g., Anthropic's content block indexing, Responses API's sequence numbers) may fail in practice.
- Severity if unresolved: **major** (but subsumed by AP2)

**AP6: "Bounded continuation store has no eviction analysis."**
- The TTL=30min and capacity=256 defaults are stated without justification. What happens under high load? The tombstone mechanism is described but not stress-tested.
- Severity if unresolved: **minor**

## Thread 2: Adjudication

### Verdicts per Attack Point

| AP | Severity | Verdict | Rationale |
|---|---|---|---|
| AP1 | major | partially_answered | The distinction between ReAct (model-driven control flow) and Pigs (middleware-enforced control flow) is architecturally significant — ReAct cannot guarantee phase discipline, Pigs can. However, the paper should more explicitly quantify what "guarantee" means (e.g., "the system never skips the Post phase"). The discussion.tex section on "Why Text Markers" partially addresses this but does not use the word "guarantee." |
| AP2 | critical | still_unresolved | Results are genuinely pending. **However:** this is acknowledged by the authors in the abstract, results section, and conclusion. The paper explicitly states "Experimental results are pending." This is not a deception — it is an incomplete evaluation. Under resubmit constraints (no new experiments), this cannot be fixed via text edits. **User escalation required.** |
| AP3 | major | partially_answered | The discussion.tex "Marker robustness" limitation paragraph acknowledges the issue. The detection algorithm (last-line-only + reason-required) is a concrete mitigation. However, empirical false positive/negative rates are not provided. Under resubmit constraints (no new experiments), this is text-fixable only by strengthening the limitation acknowledgment. |
| AP4 | minor | answered | The discussion.tex "Why HTTP Loopback Instead of In-Process Dispatch?" section directly addresses this: (1) feature inheritance (retry, body cleaning, model mapping) is automatic, (2) explicit debuggability. The design choice is justified. |
| AP5 | major | still_unresolved | Subsumed by AP2. Without results, the 3-protocol claim is theoretical. Same escalation as AP2. |
| AP6 | minor | answered | The defaults are reasonable for a single-user CLI agent system. The discussion could mention stress-testing, but the system is not designed for high-concurrency server use. Acknowledged in limitations. |

### Residual Risk Summary

| Severity | Still Unresolved | Partially Answered | Answered |
|---|---|---|---|
| Critical | 1 (AP2) | 0 | 0 |
| Major | 1 (AP5, subsumed by AP2) | 2 (AP1, AP3) | 0 |
| Minor | 0 | 0 | 2 (AP4, AP6) |

### Recommended Fix for AP1 (text-fixable, maps to allowed paths)

Strengthen the framing in introduction.tex or discussion.tex to explicitly state: "Unlike ReAct, where the model may skip planning or reflection, Pigs \emph{guarantees} that every non-trivial request passes through all three phases — the state machine cannot be bypassed by the model."

### Recommended Fix for AP3 (text-fixable, maps to allowed paths)

Strengthen the "Marker robustness" paragraph in discussion.tex to acknowledge: "Empirical measurement of false positive and false negative rates is left for future evaluation."

### User Escalation Queue

| AP | Severity | Issue | Fixable? |
|---|---|---|---|
| AP2 | critical | No experimental results | ❌ Not text-fixable — requires running the evaluation |
| AP5 | major | 3-protocol claim unverifiable | ❌ Subsumed by AP2 — requires running Experiment 3 |

### Verdict

**WARN** — Two unresolved critical/major points (AP2, AP5) require experimental results that cannot be produced under text-only resubmit constraints. The authors have acknowledged this explicitly in the paper. The remaining partially-answered points (AP1, AP3) have text-fixable improvements that have been applied in Phase 2 or can be applied as minor edits.

**Decision:** Apply the two text-fixable improvements (AP1 framing, AP3 limitation strengthening). Do NOT trigger an extra Phase 2 round for these — they are minor framing improvements. Surface AP2/AP5 to the user as acknowledged incomplete evaluation.
