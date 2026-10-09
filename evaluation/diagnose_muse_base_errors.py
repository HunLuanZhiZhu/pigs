#!/usr/bin/env python3
"""Diagnostic only: evaluate Muse forced-Full PIGS on the 26 historical Base misses.

Prompts and targets come directly from the complete 2026-10-05 Muse Base EvalScope
review.  This deliberately selected error subset is NOT a formal accuracy estimate.
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

BASE_REVIEWS = Path("/root/pigs-eval/outputs/formal/gsm8k/muse13-gsm8k-responses-v5-low-c4-20261005/base/reviews/muse-spark-1.3-contributor/gsm8k_main.jsonl")
MODEL = "muse-spark-1.3-contributor-pig3"


def utcnow():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def collect_samples(source: Path):
    result = []
    for line in source.open(encoding="utf-8"):
        row = json.loads(line)
        if row["sample_score"]["score"]["value"]["accuracy"] == 1:
            continue
        user_inputs = [m["content"] for m in row["messages"] if m["role"] == "user"]
        if not user_inputs:
            raise ValueError(f"missing user prompt at index {row['index']}")
        prompt = user_inputs[-1]
        original = next((m["content"] for m in reversed(row["messages"]) if m["role"] == "assistant"), "")
        result.append(dict(index=int(row["index"]), target=row["target"][0], prompt=prompt,
                           baseline_output=original, prompt_sha256=hashlib.sha256(prompt.encode()).hexdigest()))
    result.sort(key=lambda row: row["index"])
    if len(result) != 26:
        raise ValueError(f"historical Muse Base should have 26 errors, got {len(result)}")
    return result


def parse_completed_output(resp):
    texts = []
    for item in resp.get("output", []):
        if item.get("type") == "message":
            for part in item.get("content", []):
                if part.get("type") == "output_text":
                    texts.append(part.get("text", ""))
    return "".join(texts)


def run_sample(row, url: str, timeout: int):
    idx = row["index"]
    session = "pigs-full26-" + str(idx) + "-" + uuid.uuid4().hex[:12]
    request = {
        "model": MODEL, "input": [{"role": "user", "content": row["prompt"]}],
        "temperature": 0.0, "reasoning": {"effort": "low"}, "stream": True
    }
    started = time.monotonic()
    parts, completed, usage, errors = [], None, None, []
    try:
        with httpx.Client(timeout=httpx.Timeout(timeout, connect=20.0), trust_env=False) as client:
            with client.stream("POST", url, json=request, headers={
                "Authorization": "Bearer local-eval-placeholder",
                "Accept": "text/event-stream",
                "x-opencode-session": session,
                "Content-Type": "application/json",
            }) as response:
                response.raise_for_status()
                for line in response.iter_lines():
                    if not line.startswith("data:"):
                        continue
                    data = line[5:].strip()
                    if not data or data == "[DONE]":
                        continue
                    try:
                        event = json.loads(data)
                    except json.JSONDecodeError:
                        errors.append("malformed_sse_event")
                        continue
                    kind = event.get("type", "")
                    if kind == "response.output_text.delta":
                        parts.append(event.get("delta", ""))
                    elif kind == "response.completed":
                        completed = event.get("response", {})
                        usage = completed.get("usage")
                    elif kind in ("response.failed", "error"):
                        errors.append(json.dumps(event.get("error", event), ensure_ascii=False)[:500])
        answer = parse_completed_output(completed) if completed else ""
        if not answer:
            answer = "".join(parts)
        if completed is None:
            errors.append("missing_response.completed")
        extracted = extract_answer(answer) if answer else None
        correct = bool(extracted is not None and math_equal(str(extracted), str(row["target"])))
        return {
            "index": idx, "target": row["target"],
            "base_correct": False, "correct": correct,
            "extracted_prediction": extracted, "prediction": answer,
            "prompt_sha256": row["prompt_sha256"],
            "usage": usage, "elapsed_sec": round(time.monotonic()-started, 3),
            "errors": errors,
            "session": session,
        }
    except Exception as exc:
        return {"index": idx, "target": row["target"], "base_correct": False,
                "correct": False, "extracted_prediction": None, "prediction": "",
                "prompt_sha256": row["prompt_sha256"], "usage": usage,
                "elapsed_sec": round(time.monotonic()-started, 3),
                "errors": [f"{type(exc).__name__}: {exc}"], "session": session}


def atomic_json(path: Path, value):
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temp.replace(path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-reviews", type=Path, default=BASE_REVIEWS)
    parser.add_argument("--endpoint", default="http://127.0.0.1:3930/responses")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--timeout", type=int, default=1800)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    rows = collect_samples(args.base_reviews)
    atomic_json(args.output / "manifest.json", {
        "model": MODEL, "endpoint": args.endpoint, "subset_type": "historical_base_errors_only",
        "baseline_review_file": str(args.base_reviews), "indices": [r["index"] for r in rows],
        "count": len(rows), "settings": {"temperature": 0, "reasoning_effort": "low", "stream": True},
        "workers": args.workers, "started": utcnow(), "official_benchmark": False,
    })
    done = []
    lock = threading.Lock()
    def persist():
        clean = sum(1 for r in done if not r["errors"])
        fixed = sum(1 for r in done if r["correct"] and not r["errors"])
        lines = [f"updated: {utcnow()}", f"completed: {len(done)}/{len(rows)}",
                 f"transport_success: {clean}/{len(done)}", f"fixed: {fixed}/{len(done)}",
                 "finished_indices: " + ",".join(map(str, sorted(r["index"] for r in done)))]
        (args.output / "progress.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
        atomic_json(args.output / "progress.json", {"updated": utcnow(), "completed": len(done),
                   "total": len(rows), "transport_success": clean, "correct": fixed})
        with (args.output / "results.jsonl").open("w", encoding="utf-8") as f:
            for result in sorted(done, key=lambda r: r["index"]):
                f.write(json.dumps(result, ensure_ascii=False) + "\n")
    with lock:
        persist()
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        tasks = {pool.submit(run_sample, r, args.endpoint, args.timeout): r["index"] for r in rows}
        for future in as_completed(tasks):
            res = future.result()
            with lock:
                done.append(res)
                persist()
    clean = [r for r in done if not r["errors"]]
    fixed = [r for r in clean if r["correct"]]
    summary = {"completed": len(done), "valid": len(clean), "errors": len(done)-len(clean),
               "fixed_count": len(fixed), "fixed_indices": sorted(r["index"] for r in fixed),
               "unfixed_indices": sorted(r["index"] for r in clean if not r["correct"]),
               "ended": utcnow(), "not_formal_accuracy": True}
    atomic_json(args.output / "summary.json", summary)
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
