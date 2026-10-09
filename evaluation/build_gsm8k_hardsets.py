#!/usr/bin/env python3
"""Build reproducible GSM8K development hardsets from existing EvalScope reviews.

The sets are *diagnostic*, selected using prior GSM8K outcomes; they must not
be reported as independent held-out benchmark evaluations. Keep model-specific
error cases even when the other model solves them. No model calls are made.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
from pathlib import Path

ROOT = Path("/root/pigs-eval/outputs/formal/gsm8k")
DEFAULT_OUTPUT = Path("/root/pigs-eval/outputs/diagnostic/gsm8k-hardsets-20261008")
SOURCES = {
    "ds_v4": {
        "model": "deepseek-v4.1-flash",
        "base": ROOT / "deepseek-gsm8k-low-c4-20261004/deepseek/base/20261004_160653/reviews/deepseek-v4.1-flash/gsm8k_main.jsonl",
        "v5": ROOT / "deepseek-gsm8k-pigs-v5-low-c4-20261004/deepseek/pigs/20261004_215118/reviews/deepseek-v4.1-flash-pigs/gsm8k_main.jsonl",
        "v6": ROOT / "deepseek41-gsm8k-pig-preverify-v6-low-c4-20261008/deepseek/pig/20261008_221947/reviews/deepseek-v4.1-flash-pig/gsm8k_main.jsonl",
    },
    "muse": {
        "model": "muse-spark-1.3-contributor",
        "base": ROOT / "muse13-gsm8k-responses-v5-low-c4-20261005/base/reviews/muse-spark-1.3-contributor/gsm8k_main.jsonl",
        "v5": ROOT / "muse13-gsm8k-pig-newpre-responses-resume-low-c4-20261008/muse13/pig/20261008_141306/reviews/muse-spark-1.3-contributor-pig/gsm8k_main.jsonl",
        "v6": ROOT / "muse13-gsm8k-pig-preverify-v6-low-c4-20261008/muse13/pig/20261008_213434/reviews/muse-spark-1.3-contributor-pig/gsm8k_main.jsonl",
    },
}

# Prior human-reviewed ambiguity / reference-answer conflicts.
# Metadata for analysis ONLY; do not make model rules or choose scoring
# exceptions on the basis of individual question IDs.
REVIEW_FLAGS = {
    "ds_v4": {
        93: "Reference answer treats 10% faster running speed as a 10% decrease in time.",
        1038: "Stored reference reasoning gives 6 years, but gold target is 4.",
    },
    "muse": {
        306: "Morning/afternoon/evening bread allocation is ambiguous against reference.",
        403: "Reference reading of decreased daily AC hours conflicts with literal wording.",
        423: "Question asks direction inconsistent with absolute positive target.",
        454: "Reference calculates one person's daily apples as 1 despite each eating 4.",
        823: "Reference adds Julie's points instead of Sasha's to total Sasha's score.",
        952: "Reference interprets 11:00 PM as 11:00 AM.",
        1309: "Reference adds an incorrect amount (400 rather than Sarah's 300).",
    },
}


def load(path: Path) -> dict[int, dict]:
    return {row["index"]: row for line in path.open(encoding="utf-8")
            if (row := json.loads(line))}


def correct(row: dict | None) -> bool:
    return bool(row and row["sample_score"]["score"]["value"].get("accuracy") == 1)


def record(row: dict | None) -> dict:
    if row is None:
        return {"present": False, "correct": False, "extracted_answer": None, "output": None}
    score = row["sample_score"]["score"]
    return {"present": True, "correct": correct(row),
            "extracted_answer": score.get("extracted_prediction"),
            "output": score.get("prediction")}


def question_from_prompt(prompt: str) -> str:
    last = prompt.rsplit("\n\n", 1)[-1]
    return last.split("\nPlease reason step by step,", 1)[0].strip()


def write_jsonl(path: Path, rows: list[dict]) -> None:
    with path.open("w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False) + "\n")


def write_index(path: Path, rows: list[dict]) -> None:
    fields = ("index", "base_correct", "v5_correct", "v6_correct",
              "regression_vs_base", "regression_vs_v5",
              "fix_vs_base", "fix_vs_v5", "review_flag")
    with path.open("w", newline="", encoding="utf-8-sig") as fh:
        writer = csv.DictWriter(fh, fieldnames=fields)
        writer.writeheader()
        for row in rows:
            writer.writerow({col: row.get(col) for col in fields})


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    out = args.output
    out.mkdir(parents=True, exist_ok=True)
    summary = {"scope": "complete GSM8K test 1319", "kind": "development-hardset",
               "not_held_out": True, "selection": "errors in Base or v5 or v6",
               "all_files": {}, "models": {}}
    all_cases: dict[str, list[dict]] = {}
    raw: dict[str, dict[str, dict[int, dict]]] = {}
    for name, paths in SOURCES.items():
        reviews = {arm: load(paths[arm]) for arm in ("base", "v5", "v6")}
        assert all(len(reviews[x]) == 1319 for x in ("v5", "v6")), name
        if name == "ds_v4":
            assert set(range(1319)) - set(reviews["base"]) == {675}
        else:
            assert len(reviews["base"]) == 1319
        raw[name] = reviews
        indices = sorted(i for i in range(1319) if not all(correct(reviews[arm].get(i))
                          for arm in ("base", "v5", "v6")))
        rows = []
        for idx in indices:
            sample = reviews["v6"][idx]
            prompt = next(m["content"] for m in sample["messages"] if m["role"] == "user")
            for key in ("base", "v5"):
                other = reviews[key].get(idx)
                if other:
                    other_prompt = next(m["content"] for m in other["messages"] if m["role"] == "user")
                    assert other_prompt == prompt, (name, idx, key)
            result = {
                "index": idx, "model_label": name, "model": paths["model"],
                "question": question_from_prompt(prompt),
                "prompt": prompt, "prompt_sha256": hashlib.sha256(prompt.encode()).hexdigest(),
                "gold_target": sample["target"][0],
                "reference_reasoning": sample["sample_score"].get("sample_metadata", {}).get("reasoning"),
                "reviews": {arm: record(reviews[arm].get(idx)) for arm in ("base", "v5", "v6")},
                "review_flag": REVIEW_FLAGS.get(name, {}).get(idx),
            }
            result.update({arm + "_correct": result["reviews"][arm]["correct"]
                           for arm in ("base", "v5", "v6")})
            result.update(
                regression_vs_base=result["base_correct"] and not result["v6_correct"],
                regression_vs_v5=result["v5_correct"] and not result["v6_correct"],
                fix_vs_base=not result["base_correct"] and result["v6_correct"],
                fix_vs_v5=not result["v5_correct"] and result["v6_correct"],
            )
            rows.append(result)
        all_cases[name] = rows
        write_jsonl(out / f"{name}_hard.jsonl", rows)
        write_index(out / f"{name}_hard_index.csv", rows)
        summary["models"][name] = {
            "model": paths["model"], "count": len(rows),
            "base_error_count": sum(not correct(r) for r in reviews["base"].values()) + 1319-len(reviews["base"]),
            "v5_error_count": sum(not correct(r) for r in reviews["v5"].values()),
            "v6_error_count": sum(not correct(r) for r in reviews["v6"].values()),
            "v6_regression_vs_base": [r["index"] for r in rows if r["regression_vs_base"]],
            "v6_regression_vs_v5": [r["index"] for r in rows if r["regression_vs_v5"]],
            "source_files": {k: str(v) for k, v in paths.items() if k in ("base", "v5", "v6")},
            "files": [f"{name}_hard.jsonl", f"{name}_hard_index.csv"],
        }
    overlap = sorted({r["index"] for r in all_cases["ds_v4"]} &
                     {r["index"] for r in all_cases["muse"]})
    union = sorted({r["index"] for r in all_cases["ds_v4"]} |
                   {r["index"] for r in all_cases["muse"]})
    summary["overlap_count"] = len(overlap)
    summary["overlap_indices"] = overlap
    summary["union_count"] = len(union)
    # Fixed deterministic common-correct controls. They check unintended
    # regressions but do not prove Agent/task generalization.
    good = [i for i in range(1319)
            if all(correct(raw[model][arm].get(i))
                   for model in ("ds_v4", "muse")
                   for arm in ("base", "v5", "v6"))]
    controls = sorted(good, key=lambda i: hashlib.sha256(
        f"pigs-gsm8k-controls-20261008:{i}".encode()).digest())[:60]
    control_rows = []
    for i in controls:
        row = raw["ds_v4"]["v6"][i]
        prompt = next(m["content"] for m in row["messages"] if m["role"] == "user")
        control_rows.append({"index": i, "prompt": prompt, "question": question_from_prompt(prompt),
                             "prompt_sha256": hashlib.sha256(prompt.encode()).hexdigest(),
                             "gold_target": row["target"][0],
                             "selection": "all_model_versions_correct"})
    write_jsonl(out / "shared_correct_controls.jsonl", control_rows)
    summary["correct_controls"] = {"count": len(controls), "indices": controls,
                                    "file": "shared_correct_controls.jsonl"}
    (out / "manifest.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2)+"\n",
                                       encoding="utf-8")
    print(json.dumps({"out": str(out),
                      "ds_v4": summary["models"]["ds_v4"]["count"],
                      "muse": summary["models"]["muse"]["count"],
                      "overlap": len(overlap), "unique_union": len(union),
                      "controls": len(controls)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
