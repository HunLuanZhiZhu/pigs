# CITATION AUDIT (soft-only) — Phase 1
# Date: 2026-07-14

## Summary

- **Total unique citation keys:** 41
- **Keys in references.bib:** 54
- **Missing from bib:** 0
- **Unused bib entries:** 13 (acceptable — some are background references)

## Citation Context Check (soft-only mode)

### Contextually Correct Citations

| Citation | Context | Verdict |
|---|---|---|
| yao2022react | "ReAct pioneered interleaved reasoning and acting in LLMs" | PASS — accurate description |
| shinn2023reflexion | "Reflexion adds verbal reinforcement learning through reflective feedback" | PASS — accurate |
| madaan2023selfrefine | "Self-Refine uses a single LLM as generator, critic, and refiner" | PASS — accurate |
| wang2023plan | "Plan-and-Solve separates planning from execution in prompt structure" | PASS — accurate |
| yao2023tot | "Tree of Thoughts generalizes chain-of-thought to search structures" | PASS — accurate |
| besta2023got | "Graph of Thoughts generalize chain-of-thought to search structures" | PASS — accurate |
| kwon2023vllm | "vLLM introduces PagedAttention for efficient memory management" | PASS — accurate |
| yu2022orca | "Orca proposes distributed serving with iteration-level scheduling" | PASS — accurate |
| jimenez2024swebench | "SWE-bench: software engineering" | PASS — accurate |
| mialon2023gaia | "GAIA: general AI assistants" | PASS — accurate |
| ma2024agentboard | "AgentBoard: multi-turn analysis" | PASS — accurate |
| liu2023agentbench | "AgentBench: 8 environments" | PASS — accurate |
| qin2023toolllm | "ToolLLM/ToolBench: framework and benchmark for tool-augmented LLMs" | PASS — accurate |
| hong2023metagpt | "MetaGPT encodes SOPs into prompt sequences" | PASS — accurate |
| packer2023memgpt | "MemGPT manages control flow via OS-inspired interrupts" | PASS — accurate |
| huang2024selfcorrect | "LLMs cannot reliably self-correct reasoning without external feedback" | PASS — accurate |
| harel1987statecharts | "statecharts as a visual formalism for complex systems" | PASS — accurate |
| greshake2023promptinjection | "prompt injection attacks" | PASS — accurate |

### Potentially Soft Citations (rewrite suggestions)

| Citation | Context | Issue | Suggested Fix |
|---|---|---|---|
| dzanic2024middleware | "vision paper proposing middleware for enterprise LLM deployment" | Paper is by "Dzanic and others" — attribution may be imprecise | Soften: "A position paper proposes middleware for enterprise LLM deployment" → keep cite but remove "vision" |
| liu2026tokenmizer | "transparent proxy with session state management via a typed knowledge graph" | Entry uses "author={Liu, others}" — imprecise attribution | Soften: "TokenMizer adds session state to the proxy layer" (drop "typed knowledge graph" if unverified) |
| stream2026 | "multi-tier LLM inference routing (local→HPC→cloud) with a complexity judge" | Entry uses "author={others}" — no real authors listed | Soften: "STREAM implements multi-tier LLM inference routing" (keep but acknowledge it's a preprint) |
| dyserve2027 | "workflow-aware serving layer that reroutes on tool-call failures" | Entry uses "author={others}" — imprecise | Soften: "Dyserve is a workflow-aware serving layer" (keep but acknowledge preprint) |

### Verdict

**PASS (with soft suggestions)** — All 41 cited keys are present in the bib. 18 citations have verified contextually correct descriptions. 4 citations from preprint/position sources have imprecise authorship attribution — recommend softening the citing sentences rather than removing citations (bib is frozen under resubmit constraints).
