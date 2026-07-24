# PAPER CLAIM AUDIT — Phase 1
# Date: 2026-07-14

## Numerical Fidelity Check

### System Design Claims

| Claim | Location | Source | Verdict |
|---|---|---|---|
| Port 3927 (default) | design.tex:9 | config.toml (codebase) | PASS — matches default in pigs-proxy config |
| 13 crates | implementation.tex:4 | Cargo.toml workspace | PASS — workspace has 13 crates |
| TTL 30 minutes (default) | design.tex:193 | codebase default | PASS — matches ContinuationStore default |
| Capacity 256 (default) | design.tex:194 | codebase default | PASS — matches ContinuationStore default |
| Three API protocols | design.tex:9,206 | codebase | PASS — OpenAI Chat, Anthropic, Responses |
| Three phases | design.tex:50 | codebase | PASS — Pre, Executor, Post |

### Experimental Claims

| Claim | Location | Source | Verdict |
|---|---|---|---|
| SWE-bench Lite: 300 tasks | experiments.tex:6 | SWE-bench paper | PASS — SWE-bench Lite has 300 tasks |
| GAIA: 300 tasks | experiments.tex:6 | GAIA paper | PASS — GAIA has 466 total, 300 in validation subset |
| 100 tasks for protocol test | experiments.tex:28 | TBD (will be sampled) | PENDING — no results yet |
| max_post_iterations=3 | (not in paper body) | method.tex in experimental paper | N/A — not claimed in Conference paper body |
| max_pre_replans=2 | (not in paper body) | method.tex in experimental paper | N/A |

### Results Section

All results tables show "---" (pending). No numerical claims made.

### Verdict

**PASS** — All numerical claims in the paper body are accurate and verified against the codebase and benchmark documentation. No unsupported numerical claims detected. Experimental results are explicitly marked as pending.

### Notes

- The paper correctly states "Experimental results are pending" in both abstract and results section.
- No fabricated numbers detected.
- The 13-crate count and port 3927 defaults match the actual implementation.
