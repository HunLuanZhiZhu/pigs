#!/usr/bin/env python3
"""Aggregate multiple independent PIGS evaluation runs into paper-facing tables."""
from __future__ import annotations

import argparse
import csv
import json
from datetime import datetime
from pathlib import Path
from typing import Any

from summarize_evaluation import pct, summarize_run, write_json, atomic_write


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Aggregate multiple PIGS benchmark run directories.")
    p.add_argument("run_dirs", nargs="+", type=Path)
    p.add_argument("--output-dir", type=Path, default=None)
    return p.parse_args()


def main() -> int:
    args = parse_args()
    summaries = [summarize_run(path, write_files=False) for path in args.run_dirs]
    out = args.output_dir or Path.cwd() / f"evaluation-summary-{datetime.now().strftime('%Y%m%d_%H%M%S')}"
    out.mkdir(parents=True, exist_ok=True)

    jobs: list[dict[str, Any]] = []
    comparisons: list[dict[str, Any]] = []
    for summary in summaries:
        for job in summary["jobs"]:
            head = job.get("headline_metric") or {}
            jobs.append({
                "run_id": summary["run_id"],
                "dataset": summary["dataset"],
                "thinking_effort": summary.get("thinking_effort"),
                "model_label": job["model_label"],
                "model": job["model"],
                "arm": job["arm"],
                "status": job["status"],
                "headline_metric": head.get("metric"),
                "headline_score": head.get("score"),
                "expected_samples": job["expected_samples"],
                "completed_samples": job["completed_samples"],
            })
        for comp in summary.get("comparisons") or []:
            base_job = next((j for j in summary["jobs"] if j["model_label"] == comp["model_label"] and j["arm"] == "base"), None)
            arm = comp.get("arm", "pigs")  # pre-rename summaries have no arm field
            pigs_job = next((j for j in summary["jobs"] if j["model_label"] == comp["model_label"] and j["arm"] == arm), None)
            base_head = (base_job or {}).get("headline_metric") or {}
            pigs_head = (pigs_job or {}).get("headline_metric") or {}
            delta = None
            if isinstance(base_head.get("score"), (int, float)) and isinstance(pigs_head.get("score"), (int, float)):
                delta = pigs_head["score"] - base_head["score"]
            primary_pair = None
            paired = comp.get("paired") or {}
            preferred = {
                "gsm8k": "accuracy",
                "ifeval": "prompt_level_strict",
                "bfcl_multiturn": "multi_turn_overall",
                "swebench_lite": "resolved",
            }.get(summary["dataset"])
            if preferred in paired:
                primary_pair = paired[preferred]
            comparisons.append({
                "run_id": summary["run_id"],
                "dataset": summary["dataset"],
                "thinking_effort": summary.get("thinking_effort"),
                "model_label": comp["model_label"],
                "arm": arm,
                "headline_metric": base_head.get("metric") or pigs_head.get("metric"),
                "base_score": base_head.get("score"),
                "pigs_score": pigs_head.get("score"),
                "delta": delta,
                "paired_n": (primary_pair or {}).get("paired_n"),
                "fixes": (primary_pair or {}).get("fixes"),
                "breaks": (primary_pair or {}).get("breaks"),
                "mcnemar_exact_p": (primary_pair or {}).get("mcnemar_exact_p"),
            })

    payload = {"schema_version": 1, "generated_at": datetime.now().astimezone().isoformat(timespec="seconds"), "runs": [s["run_id"] for s in summaries], "jobs": jobs, "comparisons": comparisons}
    write_json(out / "aggregate.json", payload)

    fields = ["run_id", "dataset", "thinking_effort", "model_label", "arm", "headline_metric", "base_score", "pigs_score", "delta", "paired_n", "fixes", "breaks", "mcnemar_exact_p"]
    with (out / "comparisons.csv").open("w", encoding="utf-8", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=fields); writer.writeheader(); writer.writerows(comparisons)

    lines = ["# PIGS aggregated evaluation", "", "| Dataset | Effort | Model | Arm | Metric | Base | PIGS | Delta | Paired N | Fixes | Breaks | McNemar p |", "|---|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|"]
    for row in sorted(comparisons, key=lambda x: (x["dataset"], str(x["thinking_effort"]), x["model_label"])):
        pval = row.get("mcnemar_exact_p")
        lines.append(
            f"| {row['dataset']} | {row.get('thinking_effort')} | {row['model_label']} | {row.get('arm', 'pigs')} | {row.get('headline_metric') or '-'} | "
            f"{pct(row.get('base_score'))} | {pct(row.get('pigs_score'))} | {pct(row.get('delta'))} | {row.get('paired_n') or '-'} | "
            f"{row.get('fixes') if row.get('fixes') is not None else '-'} | {row.get('breaks') if row.get('breaks') is not None else '-'} | "
            f"{f'{pval:.4f}' if isinstance(pval,(int,float)) else '-'} |"
        )
    atomic_write(out / "aggregate.md", "\n".join(lines) + "\n")
    print(out / "aggregate.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
