#!/usr/bin/env python3
"""Rebuild paired GSM8K challenge sets from immutable EvalScope review artifacts.

Usage (stdout only):
  python evaluation/build_gsm8k_hard_cases.py --kind ds
  python evaluation/build_gsm8k_hard_cases.py --kind muse
  python evaluation/build_gsm8k_hard_cases.py --kind controls
  python evaluation/build_gsm8k_hard_cases.py --kind manifest

The challenge sets are developer diagnostics, not independent holdout scores.
Each model set is the union of failures of Base, historical v5, and v6.
Control cases are hash-selected from samples correct in ALL six review files.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path("/root/pigs-eval/outputs/formal/gsm8k")
SOURCES = {
 "ds": {
  "base": "deepseek-gsm8k-low-c4-20261004/deepseek/base/20261004_160653/reviews/deepseek-v4.1-flash/gsm8k_main.jsonl",
  "v5": "deepseek-gsm8k-pigs-v5-low-c4-20261004/deepseek/pigs/20261004_215118/reviews/deepseek-v4.1-flash-pigs/gsm8k_main.jsonl",
  "v6": "deepseek41-gsm8k-pig-preverify-v6-low-c4-20261008/deepseek/pig/20261008_221947/reviews/deepseek-v4.1-flash-pig/gsm8k_main.jsonl",
 },
 "muse": {
  "base": "muse13-gsm8k-responses-v5-low-c4-20261005/base/reviews/muse-spark-1.3-contributor/gsm8k_main.jsonl",
  "v5": "muse13-gsm8k-pig-newpre-responses-resume-low-c4-20261008/muse13/pig/20261008_141306/reviews/muse-spark-1.3-contributor-pig/gsm8k_main.jsonl",
  "v6": "muse13-gsm8k-pig-preverify-v6-low-c4-20261008/muse13/pig/20261008_213434/reviews/muse-spark-1.3-contributor-pig/gsm8k_main.jsonl",
 },
}
MODELS = {"ds": "deepseek-v4.1-flash", "muse": "muse-spark-1.3-contributor"}
PROTOCOLS = {"ds": "openai_chat", "muse": "openai_responses"}
CONTROL_COUNT = 28


def load():
    data = {}
    for model, versions in SOURCES.items():
        data[model] = {}
        for version, relative in versions.items():
            source = ROOT / relative
            if not source.is_file():
                raise FileNotFoundError(source)
            result = {}
            for line in source.open(encoding="utf-8"):
                row = json.loads(line)
                index = int(row["index"])
                if index in result:
                    raise ValueError(f"duplicate index: {source} {index}")
                result[index] = row
            data[model][version] = result
    return data


def correct(row):
    if row is None:
        return False
    return row["sample_score"]["score"]["value"]["accuracy"] == 1


def extract_task(prompt: str) -> str:
    # EvalScope GSM8K few-shot format; the task is the final paragraph.
    last = prompt.rsplit("\n\n", 1)[-1]
    return last.split("\nPlease reason step by step", 1)[0].strip()


def score_record(row):
    if row is None:
        return {"present": False, "correct": False, "answer": None, "output": None}
    s = row["sample_score"]["score"]
    return {
        "present": True,
        "correct": correct(row),
        "answer": s.get("extracted_prediction"),
        "output": s.get("prediction", ""),
    }


def sample(model, index, data, selection):
    versions = data[model]
    row = versions["v6"][index]
    prompt = next(m["content"] for m in row["messages"] if m["role"] == "user")
    for version, rows in versions.items():
        other = rows.get(index)
        if other is None:
            continue
        input_ = next(m["content"] for m in other["messages"] if m["role"] == "user")
        if input_ != prompt:
            raise ValueError(f"Different benchmark input for {model} {version} #{index}")
    records = {version: score_record(rows.get(index)) for version, rows in versions.items()}
    return {
        "index": index,
        "dataset": "gsm8k_main",
        "model": MODELS[model],
        "protocol": PROTOCOLS[model],
        "selection": selection,
        "target": row["target"][0],
        "reference_reasoning": row["sample_score"].get("sample_metadata", {}).get("reasoning", ""),
        "task": extract_task(prompt),
        "prompt": prompt,  # Exact few-shot EvalScope payload: no rewriting
        "prompt_sha256": hashlib.sha256(prompt.encode()).hexdigest(),
        "versions": records,
        "flags": {
            "base_error": not records["base"]["correct"],
            "v5_error": not records["v5"]["correct"],
            "v6_error": not records["v6"]["correct"],
            "v6_regression_vs_base": records["base"]["correct"] and not records["v6"]["correct"],
            "v6_fix_vs_base": not records["base"]["correct"] and records["v6"]["correct"],
            "v6_regression_vs_v5": records["v5"]["correct"] and not records["v6"]["correct"],
            "v6_fix_vs_v5": not records["v5"]["correct"] and records["v6"]["correct"],
        },
    }


def build(data):
    error_sets = {
        model: sorted(i for i in range(1319) if any(
            not correct(data[model][version].get(i)) for version in ("base", "v5", "v6")))
        for model in ("ds", "muse")
    }
    jointly_correct = [i for i in range(1319) if all(
        correct(data[model][version].get(i))
        for model in ("ds", "muse") for version in ("base", "v5", "v6"))]
    controls = sorted(
        sorted(jointly_correct,
               key=lambda i: hashlib.sha256(f"pigs-gsm8k-control-v1:{i}".encode()).hexdigest())
        [:CONTROL_COUNT])
    return error_sets, controls


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--kind", required=True, choices=("ds", "muse", "controls", "manifest"))
    p.add_argument("--offset", type=int, default=0, help="Start index when emitting JSONL")
    p.add_argument("--count", type=int, default=100000, help="Maximum JSONL rows to emit")
    args = p.parse_args()
    data = load()
    errors, controls = build(data)
    if args.kind in ("ds", "muse"):
        for i in errors[args.kind][args.offset:args.offset + args.count]:
            print(json.dumps(sample(args.kind, i, data, "union_base_v5_v6_errors"),
                             ensure_ascii=False))
    elif args.kind == "controls":
        pairs = [(model, i) for model in ("ds", "muse") for i in controls]
        for model, i in pairs[args.offset:args.offset + args.count]:
            print(json.dumps(sample(model, i, data, "all_six_versions_correct_hash_control"),
                             ensure_ascii=False))
    else:
        print(json.dumps({
            "purpose": "development_error_analysis_not_independent_generalization_estimate",
            "dataset": "GSM8K test 1319",
            "selected_from": "full historical Base, v5, and v6 review files",
            "selection_rule": "union of incorrect or missing scores across three arms per model",
            "source_paths": {model: {v: str(ROOT / relative) for v, relative in items.items()}
                             for model, items in SOURCES.items()},
            "counts": {"ds": len(errors["ds"]), "muse": len(errors["muse"]),
                       "overlap": len(set(errors["ds"]) & set(errors["muse"])),
                       "unique": len(set(errors["ds"]) | set(errors["muse"])),
                       "controls_per_model": len(controls)},
            "hard_indices": errors,
            "control_indices": controls,
            "modes_note": "historical v5 may use -pigs (mode A), current v6 -pig (mode B); do not conflate output changes with prompt alone",
            "quality_note": "some GSM8K references are ambiguous or internally inconsistent; preserve raw source and inspect disagreements manually",
        }, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
