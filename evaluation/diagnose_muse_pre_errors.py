#!/usr/bin/env python3
"""Development-only paired regression check for Muse Pre prompt edits.

The cases come from the completed 2026-10-08 full Muse PIGS run:
27 incorrect Pre/SIMPLE_PATH cases, 3 repaired Pre cases, and 4 Full
regressions as route/control checks. This targeted set is NOT a benchmark
sample, cannot estimate overall accuracy, and must not be used as paper score.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import threading
import time
import uuid

import httpx
from evalscope.metrics.math.parser import extract_answer, math_equal

BASE = Path("/root/pigs-eval/outputs/formal/gsm8k/muse13-gsm8k-responses-v5-low-c4-20261005/base/reviews/muse-spark-1.3-contributor/gsm8k_main.jsonl")
OLD_PIG = Path("/root/pigs-eval/outputs/formal/gsm8k/muse13-gsm8k-pig-newpre-responses-resume-low-c4-20261008/muse13/pig/20261008_141306/reviews/muse-spark-1.3-contributor-pig/gsm8k_main.jsonl")
# These are complete classes from the FULL run, not randomly or
# outcome-selectively sampled purported benchmark subsets.
PRE_SHARED_ERRORS = (12, 306, 368, 403, 423, 454, 552, 590, 649, 749,
                     768, 782, 814, 823, 952, 962, 1001, 1042, 1048, 1161, 1309)
PRE_REGRESSIONS = (182, 409, 652, 835, 956, 1016)
PRE_FIXES = (340, 988, 1176)
FULL_REGRESSIONS = (640, 777, 852, 858)
CATEGORIES = {"pre_shared_errors": PRE_SHARED_ERRORS,
              "pre_regressions": PRE_REGRESSIONS,
              "pre_fixes": PRE_FIXES,
              "full_regressions": FULL_REGRESSIONS}


def timestamp() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def atomic_json(path: Path, data: object) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    tmp.replace(path)


def read_rows(path: Path) -> dict[int, dict]:
    return {row["index"]: row for ln in path.open(encoding="utf-8")
            if (row := json.loads(ln))}


def score(row: dict) -> dict:
    s = row["sample_score"]["score"]
    return {"correct": s["value"]["accuracy"] == 1,
            "answer": s["extracted_prediction"],
            "output": s["prediction"]}


def rows_for_diagnostic(base: dict, old: dict) -> list[dict]:
    result = []
    for group, ids in CATEGORIES.items():
        for idx in ids:
            a, b = base[idx], old[idx]
            prompt = [m["content"] for m in b["messages"] if m["role"] == "user"][-1]
            assert prompt == [m["content"] for m in a["messages"] if m["role"] == "user"][-1], idx
            bs, os = score(a), score(b)
            assert (group == "pre_regressions" and bs["correct"] and not os["correct"]) or (
                    group == "pre_fixes" and not bs["correct"] and os["correct"]) or (
                    group == "pre_shared_errors" and not bs["correct"] and not os["correct"]) or (
                    group == "full_regressions" and bs["correct"] and not os["correct"]), (group, idx)
            result.append({"index": idx, "group": group, "prompt": prompt,
                           "target": b["target"][0], "base": bs, "old_pig": os,
                           "prompt_sha256": hashlib.sha256(prompt.encode()).hexdigest()})
    assert len(result) == len(set(row["index"] for row in result)) == 34
    return sorted(result, key=lambda r: r["index"])


def completed_text(response: dict) -> str:
    return "".join(part.get("text", "") for item in response.get("output", [])
                   if item.get("type") == "message"
                   for part in item.get("content", [])
                   if part.get("type") == "output_text")


def run_one(row: dict, url: str, timeout: int) -> dict:
    session = f"muse-precheck-{row['index']}-{uuid.uuid4().hex[:10]}"
    payload = {"model": "muse-spark-1.3-contributor-pig",
               "input": [{"role": "user", "content": row["prompt"]}],
               "temperature": 0.0, "reasoning": {"effort": "low"}, "stream": True}
    t = time.monotonic()
    result = {"index": row["index"], "group": row["group"], "target": row["target"],
              "base_correct": row["base"]["correct"],
              "old_correct": row["old_pig"]["correct"],
              "base_answer": row["base"]["answer"], "old_answer": row["old_pig"]["answer"],
              "prompt_sha256": row["prompt_sha256"], "session": session,
              "errors": [], "usage": None}
    try:
        events, completed = [], None
        with httpx.Client(timeout=httpx.Timeout(timeout, connect=20.0), trust_env=False) as client:
            with client.stream("POST", url, json=payload, headers={
                    "Authorization": "Bearer local-eval-placeholder",
                    "Accept": "text/event-stream",
                    "x-opencode-session": session,
                    "Content-Type": "application/json"}) as response:
                response.raise_for_status()
                for line in response.iter_lines():
                    if not line.startswith("data:"):
                        continue
                    raw = line[5:].strip()
                    if not raw or raw == "[DONE]":
                        continue
                    try:
                        event = json.loads(raw)
                    except json.JSONDecodeError:
                        result["errors"].append("invalid_json_event")
                        continue
                    typ = event.get("type", "")
                    if typ == "response.output_text.delta":
                        events.append(event.get("delta", ""))
                    elif typ == "response.completed":
                        completed = event.get("response", {})
                    elif typ in ("response.failed", "error"):
                        result["errors"].append(str(event.get("error", typ))[:500])
        if completed is None:
            result["errors"].append("missing_response.completed")
        else:
            result["usage"] = completed.get("usage")
        answer = completed_text(completed) if completed else ""
        if not answer:
            answer = "".join(events)
        extracted = extract_answer(answer) if answer else None
        result.update(answer=extracted, output=answer,
                      correct=bool(extracted is not None and
                                   math_equal(str(extracted), str(row["target"]))))
    except Exception as exc:
        result.update(answer=None, output="", correct=False)
        result["errors"].append(f"{type(exc).__name__}: {exc}")
    result["elapsed_sec"] = round(time.monotonic() - t, 3)
    return result


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--endpoint", default="http://127.0.0.1:3930/responses")
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--workers", type=int, default=4)
    p.add_argument("--timeout", type=int, default=1800)
    p.add_argument("--max-requests", type=int, default=34)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    rows = rows_for_diagnostic(read_rows(BASE), read_rows(OLD_PIG))
    if len(rows) > args.max_requests:
        raise ValueError("Diagnostic request cap exceeded.")
    prior_manifest = args.output / "manifest.json"
    if prior_manifest.exists():
        old_manifest = json.loads(prior_manifest.read_text(encoding="utf-8"))
        if old_manifest.get("total") != len(rows) or old_manifest.get("model") != "muse-spark-1.3-contributor-pig":
            raise ValueError("Existing manifest does not match this diagnostic.")
    manifest = {"purpose": "developer_diagnostic_not_a_benchmark",
                "selection": "all historical Pre direct wrong, all Pre flips toward correct, all Full regressions",
                "categories": {k: list(v) for k, v in CATEGORIES.items()},
                "source_base": str(BASE), "source_old_pig": str(OLD_PIG),
                "model": "muse-spark-1.3-contributor-pig", "endpoint": args.endpoint,
                "settings": {"temperature": 0.0, "reasoning_effort": "low", "stream": True},
                "total": len(rows), "workers": args.workers,
                "started": timestamp(), "is_official_benchmark": False}
    if not prior_manifest.exists():
        atomic_json(prior_manifest, manifest)
    prior_results = args.output / "results.jsonl"
    done: list[dict] = ([json.loads(line) for line in prior_results.open(encoding="utf-8")]
                        if prior_results.exists() else [])
    expected = {r["index"]: r for r in rows}
    if len({r["index"] for r in done}) != len(done) or any(
        r["index"] not in expected or r["group"] != expected[r["index"]]["group"]
        or r["prompt_sha256"] != expected[r["index"]]["prompt_sha256"] for r in done
    ):
        raise ValueError("Existing results do not match this diagnostic.")
    lock = threading.Lock()

    def persist():
        clean = [r for r in done if not r["errors"]]
        lines = [f"updated: {timestamp()}", f"completed: {len(done)}/{len(rows)}",
                 f"valid: {len(clean)}/{len(done)}",
                 f"new_correct: {sum(r['correct'] for r in clean)}/{len(clean)}",
                 "completed_indices: " + ",".join(map(str, sorted(r["index"] for r in done)))]
        (args.output / "progress.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
        atomic_json(args.output / "progress.json",
                    {"updated": timestamp(), "completed": len(done), "total": len(rows),
                     "valid": len(clean)})
        with (args.output / "results.jsonl").open("w", encoding="utf-8") as f:
            for res in sorted(done, key=lambda z: z["index"]):
                f.write(json.dumps(res, ensure_ascii=False) + "\n")

    with lock:
        persist()
    completed_ids = {r["index"] for r in done}
    remaining = [row for row in rows if row["index"] not in completed_ids]
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        futures = {pool.submit(run_one, row, args.endpoint, args.timeout): row["index"]
                   for row in remaining}
        for f in as_completed(futures):
            res = f.result()
            with lock:
                done.append(res)
                persist()
    groups = {}
    for name, ids in CATEGORIES.items():
        batch = [r for r in done if r["group"] == name]
        good = [r for r in batch if not r["errors"]]
        groups[name] = {
            "count": len(batch), "valid": len(good),
            "old_correct": sum(r["old_correct"] for r in good),
            "new_correct": sum(r["correct"] for r in good),
            "repaired": sorted(r["index"] for r in good if r["correct"] and not r["old_correct"]),
            "broken": sorted(r["index"] for r in good if not r["correct"] and r["old_correct"]),
            "invalid": sorted(r["index"] for r in batch if r["errors"])}
    atomic_json(args.output / "summary.json",
                {"completed": len(done), "total": len(rows),
                 "valid": sum(not r["errors"] for r in done), "groups": groups,
                 "ended": timestamp(), "not_formal_accuracy": True})
    print(json.dumps(groups, ensure_ascii=False))


if __name__ == "__main__":
    main()
