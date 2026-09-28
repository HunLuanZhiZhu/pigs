# RESUBMIT REPORT — Master Ledger
# Date: 2026-07-14

## Overview

| Field | Value |
|---|---|
| Audit skill | resubmit-pipeline |
| Verdict | **WARN** |
| Reason code | partially_addressed_concerns |
| Source dir | paper/system-design/ |
| Target venue | Conference |
| New dir | paper/Conference/ |
| Assurance level | submission |
| Effort | max |
| Rounds completed | 1 (converged) |
| Final page count | 15 |
| Citation count | 41 (0 undefined) |
| Bib entries | 54 |

## Verdict Summary

**WARN** — All MUST-FIX gates passed (compile, anonymity, audits non-blocking). Two critical/major concerns from the adversarial gate (AP2: no experimental results, AP5: protocol claim unverifiable) remain unresolved because they require running the evaluation, which is outside the text-only resubmit constraint. The authors have explicitly acknowledged this in the paper ("Experimental results are pending").

## Phase Results

### Phase 0: Physical Isolation ✅
- New sibling dir `paper/Conference/` created (never overwrites prior)
- Sections copied from `paper/system-design/sections/`
- `experiments.tex` and `results.tex` merged from `paper/experimental/`
- `references.bib` merged and deduplicated (54 unique entries)
- `edit_whitelist.yaml` generated

### Phase 0.5: Health Check + Anonymity Scan ✅
- **Compile:** SUCCESS — 15 pages, 0 undefined citations
- **Anonymity scan (5-layer):** ALL CLEAN
  - Layer 1 (surface identifiers): 0 hits
  - Layer 2 (self-citation phrasing): 0 hits
  - Layer 3 (acknowledgments/funding): 0 hits
  - Layer 4 (cross-rebuttal references): 0 hits
  - Layer 5 (internal codenames/links): 0 hits
- **Residual coloring:** 0 hits

### Phase 1: Audit ✅
- **PROOF_AUDIT:** PASS — Algorithm 1 restatement consistent across all sections
- **PAPER_CLAIM_AUDIT:** PASS — all numerical claims (port 3927, 13 crates, TTL 30min, capacity 256) verified against codebase
- **CITATION_AUDIT (soft-only):** PASS — 41/41 keys in bib, 4 soft suggestions for preprint sources
- **KNOWN_WEAKNESSES:** 13 concerns atomized (3 critical, 4 major, 6 minor)

### Phase 2: Targeted Text Microedits ✅ (Converged after Round 1)
5 edits applied:
1. `introduction.tex` — expanded to 6 contributions (added experiment items) [W1, W5, W9]
2. `discussion.tex` — removed inline Conclusion (structural cleanup) [W13]
3. `conclusion.tex` — rewritten to merge system+experiment summary [W5, W13]
4. `related.tex` — softened 4 preprint citation phrases [citation-audit soft suggestions]
5. `experiments.tex` — added Experimental Setup subsection [W2, W10]

### Phase 3: Adversarial Gate ⚠️ (WARN)
6 attack points decomposed:
- AP1 (major): "just prompt chaining rebranded" → **partially_answered** (framing strengthened)
- AP2 (critical): "no experimental evidence" → **still_unresolved** (requires running evaluation)
- AP3 (major): "markers fragile and untested" → **partially_answered** (limitation strengthened)
- AP4 (minor): "HTTP loopback unnecessary" → **answered** (discussion justifies)
- AP5 (major): "3-protocol claim unverifiable" → **still_unresolved** (subsumed by AP2)
- AP6 (minor): "continuation store no eviction analysis" → **answered**

Text-fixable improvements applied: AP1 (guarantee framing in introduction), AP3 (empirical rates as future work in discussion).

### Phase 4: Final Compile + Diff Report ✅
- Final PDF: 15 pages, 348 KB
- 0 undefined citations, 0 undefined references
- DIFF_REPORT.md generated (280 lines, section-by-section diff vs system-design)

## User Escalation Queue

| AP | Severity | Issue | Fixable? |
|---|---|---|---|
| AP2 | critical | No experimental results | ❌ Requires running evaluation |
| AP5 | major | 3-protocol claim unverifiable | ❌ Subsumed by AP2 |

## Artifacts

| File | Phase | Status |
|---|---|---|
| BASELINE.md | 0.5 | ✅ |
| PROOF_AUDIT.md | 1 | ✅ |
| PAPER_CLAIM_AUDIT.md | 1 | ✅ |
| CITATION_AUDIT.md | 1 | ✅ |
| KNOWN_WEAKNESSES.md | 1 | ✅ |
| PAPER_IMPROVEMENT_LOG.md | 2 | ✅ |
| KILL_ARGUMENT.md | 3 | ✅ |
| .aris/DIFF_REPORT.md | 4 | ✅ |
| .aris/edit_whitelist.yaml | 0 | ✅ |
| main.tex + sections/ + main.pdf | 4 | ✅ |

## Skipped Constraints

None.

## Review Tracing

All review decisions are recorded in this report and the per-phase artifacts. No external Codex MCP reviewer calls were made (audits conducted via static analysis of the LaTeX source).
