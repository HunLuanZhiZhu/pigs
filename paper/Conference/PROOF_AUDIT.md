# PROOF AUDIT — Phase 1
# Date: 2026-07-14

## Restatement Check

**Paper type:** Systems paper (no formal theorems, lemmas, or propositions).

### Formal Mathematical Content

No `\begin{theorem}`, `\begin{lemma}`, `\begin{proposition}`, or `\begin{corollary}` environments found.

### Algorithm 1: Phase State Machine Transition

**Location:** sections/design.tex, lines 52-85

**Restatement sites:**
1. **design.tex:4** — "three-layer system: proxy layer, phased runtime layer (Pre→Executor→Post state machine), CLI layer"
2. **design.tex:50** — "pure state machine with three phases and three transition signals"
3. **design.tex:87-93** — Key properties described in prose:
   - "Markerless Post stays in Post" — matches Algorithm line 80 (`Return Continue(Post)`)
   - "Budget exhaustion is an error" — matches Algorithm line 77 (`Return Error(PostBudgetExceeded)`)
   - "Failure preserves history" — matches Algorithm line 62/74 (`Return replan(output)`)
4. **design.tex:95-140** — Control markers section describes PIGEND/PIGFAIL detection
5. **implementation.tex:36-46** — HTTP Runtime describes the same loop: construct → send → extract → check tools → advance
6. **discussion.tex:10-11** — "Why Post Stays in Post" restates the same property

### Cross-Section Consistency

| Property | design.tex | implementation.tex | discussion.tex | introduction.tex |
|---|---|---|---|---|
| Pre→Executor→Post | ✓ (Algorithm) | ✓ (HTTP Runtime) | ✓ (Why Post stays) | ✓ (enumerate) |
| PIGEND = complete | ✓ (Algorithm L60,72) | — | — | ✓ |
| PIGFAIL = replan | ✓ (Algorithm L62,74) | — | ✓ (Why Post stays) | ✓ |
| Markerless Post = stay | ✓ (Algorithm L80) | — | ✓ (dedicated section) | ✓ |
| Budget = error | ✓ (Algorithm L77) | — | ✓ (dedicated section) | — |
| Executor always → Post | ✓ (Algorithm L69) | ✓ (step 5: advance) | ✓ | ✓ |

### Verdict

**PASS** — All state machine transitions described in prose are consistent with Algorithm 1. No restatement gaps detected. The three key properties (markerless Post stays, budget = error, failure preserves history) are correctly reflected in both the algorithm and the discussion section.

### Notes

- The algorithm uses `\Function{advance}{output}` which is a transition function, not a full proof. This is appropriate for a systems paper.
- The state machine is simple enough (3 states, 3 signals) that formal verification is straightforward but not included — this is noted as a limitation in discussion.tex.
