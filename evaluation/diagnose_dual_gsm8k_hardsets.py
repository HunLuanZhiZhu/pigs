#!/usr/bin/env python3
"""Focused or full hardset probe for both PIGS models, with correct controls.

Developer diagnostics ONLY. Selections were chosen after examining previous
GSM8K outcomes, so do not report them as statistically independent evidence.
Resumable, file-progress, request cap, no retries of completed requests.
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

DEFAULT_HARDSETS = Path("/root/pigs-eval/outputs/diagnostic/gsm8k-hardsets-20261008")
DEFAULT_ENDPOINT = "http://127.0.0.1:3932"


def now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def jsonl(path: Path) -> list[dict]:
    return [json.loads(ln) for ln in path.open(encoding="utf-8")]


def save_json(path: Path, value: object) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def select(root: Path, stage: str, controls: int, models: list[str]) -> list[dict]:
    control_pool = jsonl(root / "shared_correct_controls.jsonl")[:controls]
    selected = []
    for model in models:
        all_hard = jsonl(root / (model + "_hard.jsonl"))
        for item in all_hard:
            active = (stage == "all" or item["regression_vs_base"] or
                      item["regression_vs_v5"] or item["fix_vs_base"] or
                      item["fix_vs_v5"])
            if not active:
                continue
            selected.append({"model_label": model, "index": item["index"],
                             "is_control": False, "prompt": item["prompt"],
                             "target": item["gold_target"],
                             "prompt_sha256": item["prompt_sha256"],
                             "base_correct": item["base_correct"],
                             "v5_correct": item["v5_correct"],
                             "v6_correct": item["v6_correct"],
                             "flags": {"regression_vs_base": item["regression_vs_base"],
                                       "regression_vs_v5": item["regression_vs_v5"],
                                       "fix_vs_base": item["fix_vs_base"],
                                       "fix_vs_v5": item["fix_vs_v5"]}})
        for item in control_pool:
            selected.append({"model_label": model, "index": item["index"],
                             "is_control": True, "prompt": item["prompt"],
                             "target": item["gold_target"],
                             "prompt_sha256": item["prompt_sha256"],
                             "base_correct": True, "v5_correct": True, "v6_correct": True,
                             "flags": {}})
    assert len({(v["model_label"], v["index"]) for v in selected}) == len(selected)
    return selected


def output_text_response(response: dict) -> str:
    return "".join(part.get("text", "") for item in response.get("output", [])
                   if item.get("type") == "message"
                   for part in item.get("content", [])
                   if part.get("type") == "output_text")


def run(row: dict, endpoint: str, timeout: int) -> dict:
    model = row["model_label"]
    session = f"pigs-dual-probe-{model}-{row['index']}-{uuid.uuid4().hex[:10]}"
    is_muse = model == "muse"
    url = endpoint + ("/responses" if is_muse else "/chat/completions")
    name = "muse-spark-1.3-contributor-pig" if is_muse else "deepseek-v4.1-flash-pig"
    payload = {"model": name,
               "temperature": 0.0,
               "stream": True,
               **({"reasoning": {"effort": "low"},
                   "input": [{"role": "user", "content": row["prompt"]}]}
                  if is_muse else {"reasoning_effort": "low",
                                   "messages": [{"role": "user", "content": row["prompt"]}]})}
    started = time.monotonic()
    result = {"index": row["index"], "model_label": model, "target": row["target"],
              "is_control": row["is_control"], "flags": row["flags"],
              "base_correct": row["base_correct"], "v5_correct": row["v5_correct"],
              "v6_correct": row["v6_correct"],
              "prompt_sha256": row["prompt_sha256"], "session": session,
              "errors": [], "usage": None, "elapsed_sec": None}
    try:
        texts: list[str] = []
        completed = None
        with httpx.Client(timeout=httpx.Timeout(timeout, connect=20.0), trust_env=False) as client:
            with client.stream("POST", url, json=payload, headers={
                "Authorization": "Bearer local-eval-placeholder",
                "Accept": "text/event-stream",
                "x-opencode-session": session,
                "Content-Type": "application/json",
            }) as resp:
                resp.raise_for_status()
                for ln in resp.iter_lines():
                    if not ln.startswith("data:"):
                        continue
                    raw = ln[5:].strip()
                    if not raw or raw == "[DONE]":
                        continue
                    try:
                        event = json.loads(raw)
                    except ValueError:
                        result["errors"].append("malformed_sse")
                        continue
                    if is_muse:
                        typ = event.get("type", "")
                        if typ == "response.output_text.delta":
                            texts.append(event.get("delta", ""))
                        elif typ == "response.completed":
                            completed = event.get("response", {})
                            result["usage"] = completed.get("usage")
                        elif typ in ("response.failed", "error"):
                            result["errors"].append(str(event.get("error", typ))[:300])
                    else:
                        if event.get("usage"):
                            result["usage"] = event["usage"]
                        for choice in event.get("choices", []):
                            delta = choice.get("delta", {})
                            part = delta.get("content")
                            if isinstance(part, str):
                                texts.append(part)
                        if event.get("error"):
                            result["errors"].append(str(event["error"])[:300])
        if is_muse and completed is None:
            result["errors"].append("missing_response.completed")
        answer = output_text_response(completed) if completed else ""
        if not answer:
            answer = "".join(texts)
        pred = extract_answer(answer) if answer else None
        result.update(output=answer, answer=pred, correct=bool(
            pred is not None and math_equal(str(pred), str(row["target"]))))
        if not answer:
            result["errors"].append("empty_answer")
    except Exception as exc:
        result.update(output="", answer=None, correct=False)
        result["errors"].append(f"{type(exc).__name__}: {exc}")
    result["elapsed_sec"] = round(time.monotonic() - started, 3)
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--hardsets", type=Path, default=DEFAULT_HARDSETS)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--endpoint", default=DEFAULT_ENDPOINT)
    parser.add_argument("--stage", choices=("focused", "all"), default="focused")
    parser.add_argument("--models", nargs="+", choices=("ds_v4", "muse"),
                        default=["ds_v4", "muse"])
    parser.add_argument("--controls", type=int, default=12)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--max-requests", type=int, default=250)
    args = parser.parse_args()
    cases = select(args.hardsets, args.stage, args.controls, args.models)
    if len(cases) > args.max_requests:
        raise ValueError("Request cap exceeded")
    args.output.mkdir(parents=True, exist_ok=True)
    manifest = args.output / "manifest.json"
    new_manifest = {"stage": args.stage, "models": args.models,
                    "controls_per_model": args.controls, "endpoint": args.endpoint,
                    "total": len(cases), "created": now(),
                    "not_independent_benchmark": True,
                    "cases": [{"model": x["model_label"], "index": x["index"],
                               "prompt_sha256": x["prompt_sha256"]} for x in cases]}
    if manifest.exists():
        old = json.loads(manifest.read_text(encoding="utf-8"))
        for k in ("stage", "models", "controls_per_model", "endpoint", "total", "cases"):
            if old[k] != new_manifest[k]:
                raise ValueError(f"Existing manifest mismatch: {k}")
    else:
        save_json(manifest, new_manifest)
    output_file = args.output / "results.jsonl"
    done = jsonl(output_file) if output_file.exists() else []
    ids = {(x["model_label"], x["index"]): x for x in cases}
    if len({(r["model_label"], r["index"]) for r in done}) != len(done):
        raise ValueError("Duplicate prior results")
    if any((r["model_label"], r["index"]) not in ids or
           r["prompt_sha256"] != ids[(r["model_label"], r["index"])]["prompt_sha256"]
           for r in done):
        raise ValueError("Prior results mismatch")
    lock = threading.Lock()

    def summary() -> dict:
        ret = {}
        for model in args.models:
            model_rows = [r for r in done if r["model_label"] == model]
            valid = [r for r in model_rows if not r["errors"]]
            ret[model] = {
                "completed": len(model_rows),
                "valid": len(valid),
                "v6_correct_on_completed": sum(r["v6_correct"] for r in valid),
                "candidate_correct": sum(r["correct"] for r in valid),
                "net_vs_v6": sum(int(r["correct"])-int(r["v6_correct"]) for r in valid),
                "newly_broken_controls": sorted(r["index"] for r in valid
                                                if r["is_control"] and not r["correct"]),
                "fixed": sorted(r["index"] for r in valid
                                if not r["v6_correct"] and r["correct"]),
                "broken": sorted(r["index"] for r in valid
                                 if r["v6_correct"] and not r["correct"]),
                "errors": sorted(r["index"] for r in model_rows if r["errors"])
            }
        return ret

    def persist() -> None:
        metrics = summary()
        lines = [f"updated: {now()}", f"completed: {len(done)}/{len(cases)}",
                 f"valid: {sum(not x['errors'] for x in done)}/{len(done)}",
                 f"stage: {args.stage}"]
        for model, vals in metrics.items():
            lines.append(f"{model}: {vals['completed']} done, {vals['valid']} valid, "
                         f"correct {vals['candidate_correct']} vs v6 {vals['v6_correct_on_completed']}, "
                         f"net={vals['net_vs_v6']:+d}, control_breaks={len(vals['newly_broken_controls'])}")
        (args.output / "progress.txt").write_text("\n".join(lines)+"\n", encoding="utf-8")
        save_json(args.output / "progress.json",
                  {"updated": now(), "completed": len(done),
                   "total": len(cases), "models": metrics})
        with output_file.open("w", encoding="utf-8") as f:
            for x in sorted(done, key=lambda r: (r["model_label"], r["index"])):
                f.write(json.dumps(x, ensure_ascii=False) + "\n")

    with lock:
        persist()
    previous = {(r["model_label"], r["index"]) for r in done}
    to_run = [r for r in cases if (r["model_label"], r["index"]) not in previous]
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        tasks = {pool.submit(run, row, args.endpoint, args.timeout):
                 (row["model_label"], row["index"]) for row in to_run}
        for future in as_completed(tasks):
            response = future.result()
            with lock:
                done.append(response)
                persist()
    out = {"completed": len(done), "total": len(cases),
           "valid": sum(not r["errors"] for r in done),
           "models": summary(), "ended": now(),
           "not_independent_benchmark": True}
    save_json(args.output / "summary.json", out)
    print(json.dumps(out, ensure_ascii=False))


if __name__ == "__main__":
    main()
