#!/usr/bin/env python3
"""Unified statistics for the redesigned PIGS paper evaluation.

Supports live and completed runs from:
- GSM8K / IFEval via EvalScope
- BFCL V4 Multi-Turn official harness
- SWE-bench Lite via mini-SWE-agent + official Docker scorer

The script never re-scores benchmark answers. Official benchmark metrics are read
from the official harness artifacts. It adds execution, efficiency, orchestration,
and paired Base-vs-PIGS statistics around those official scores.
"""
from __future__ import annotations

import argparse
import ast
import csv
import json
import math
import statistics
from collections import Counter
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable

TERMINAL = {"done", "failed"}
BFCL_CATEGORIES = [
    "multi_turn_base",
    "multi_turn_miss_func",
    "multi_turn_miss_param",
    "multi_turn_long_context",
]


def atomic_write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    tmp.replace(path)


def write_json(path: Path, data: object) -> None:
    atomic_write(path, json.dumps(data, ensure_ascii=False, indent=2) + "\n")


def load_json(path: Path, default: Any = None) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return default


def number(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    return float(value) if isinstance(value, (int, float)) else None


def percentile(values: list[float], q: float) -> float | None:
    if not values:
        return None
    xs = sorted(values)
    if len(xs) == 1:
        return xs[0]
    pos = (len(xs) - 1) * q
    lo, hi = math.floor(pos), math.ceil(pos)
    if lo == hi:
        return xs[lo]
    frac = pos - lo
    return xs[lo] * (1 - frac) + xs[hi] * frac


def numeric_summary(values: Iterable[float]) -> dict[str, Any]:
    xs = [float(x) for x in values]
    return {
        "n": len(xs),
        "mean": statistics.fmean(xs) if xs else None,
        "median": statistics.median(xs) if xs else None,
        "p90": percentile(xs, 0.90),
        "p95": percentile(xs, 0.95),
        "max": max(xs) if xs else None,
    }


def parse_iso(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def elapsed_seconds(started: str | None, finished: str | None, updated: str | None) -> float | None:
    start = parse_iso(started)
    end = parse_iso(finished) or parse_iso(updated)
    if not start or not end:
        return None
    return max((end - start).total_seconds(), 0.0)


def read_jsonl_files(paths: Iterable[Path]) -> tuple[list[dict[str, Any]], int]:
    rows: list[dict[str, Any]] = []
    malformed = 0
    for path in sorted(paths):
        try:
            with path.open("r", encoding="utf-8", errors="replace") as handle:
                for line in handle:
                    if not line.strip():
                        continue
                    try:
                        item = json.loads(line)
                        if isinstance(item, dict):
                            rows.append(item)
                        else:
                            malformed += 1
                    except json.JSONDecodeError:
                        malformed += 1
        except OSError:
            continue
    return rows, malformed


def prediction_records(job_dir: Path) -> tuple[list[dict[str, Any]], int]:
    return read_jsonl_files(
        p for p in job_dir.rglob("*.jsonl") if "predictions" in p.parts
    )


def review_records(job_dir: Path) -> tuple[list[dict[str, Any]], int]:
    return read_jsonl_files(p for p in job_dir.rglob("*.jsonl") if "reviews" in p.parts)


def usage_value(usage: dict[str, Any], *keys: str) -> float:
    for key in keys:
        val = number(usage.get(key))
        if val is not None:
            return val
    return 0.0


def parse_usage(output: dict[str, Any]) -> tuple[dict[str, float], bool]:
    usage = output.get("usage") if isinstance(output, dict) else None
    usage = usage if isinstance(usage, dict) else {}
    inp = usage_value(usage, "input_tokens", "prompt_tokens")
    out = usage_value(usage, "output_tokens", "completion_tokens")
    total = usage_value(usage, "total_tokens") or (inp + out if inp or out else 0.0)
    cache_known = False
    cache_read = usage_value(usage, "input_tokens_cache_read", "cache_read_input_tokens")
    if any(k in usage for k in ("input_tokens_cache_read", "cache_read_input_tokens")):
        cache_known = True
    for key in ("prompt_tokens_details", "input_tokens_details"):
        details = usage.get(key)
        if isinstance(details, dict) and number(details.get("cached_tokens")) is not None:
            cache_read = usage_value(details, "cached_tokens")
            cache_known = True
            break
    cache_write = usage_value(usage, "input_tokens_cache_write", "cache_creation_input_tokens")
    details = usage.get("prompt_tokens_details")
    if cache_write == 0 and isinstance(details, dict):
        cache_write = usage_value(details, "cache_write_tokens")
    reasoning = usage_value(usage, "reasoning_tokens")
    details = usage.get("completion_tokens_details")
    if reasoning == 0 and isinstance(details, dict):
        reasoning = usage_value(details, "reasoning_tokens")
    return ({
        "input_tokens": inp,
        "output_tokens": out,
        "total_tokens": total,
        "cache_read_tokens": cache_read,
        "cache_write_tokens": cache_write,
        "reasoning_tokens": reasoning,
    }, cache_known)


def prediction_stats(rows: list[dict[str, Any]], malformed: int) -> dict[str, Any]:
    totals: Counter[str] = Counter()
    latencies: list[float] = []
    ttfts: list[float] = []
    tpots: list[float] = []
    errors = 0
    rows_with_usage = 0
    cache_known_input = 0.0
    indices: set[str] = set()

    for row in rows:
        if "index" in row:
            indices.add(str(row["index"]))
        output = row.get("model_output")
        if not isinstance(output, dict):
            errors += 1
            continue
        if output.get("error") not in (None, "", False):
            errors += 1
        perf = output.get("perf_metrics") if isinstance(output.get("perf_metrics"), dict) else {}
        latency = number(output.get("time")) or number(perf.get("latency"))
        ttft = number(perf.get("ttft"))
        tpot = number(perf.get("tpot"))
        if latency is not None: latencies.append(latency)
        if ttft is not None: ttfts.append(ttft)
        if tpot is not None: tpots.append(tpot)
        parsed, cache_known = parse_usage(output)
        if any(parsed.values()):
            rows_with_usage += 1
            totals.update(parsed)
            if cache_known:
                cache_known_input += parsed["input_tokens"]
        elif perf:
            inp = number(perf.get("input_tokens")) or 0.0
            out = number(perf.get("output_tokens")) or 0.0
            if inp or out:
                rows_with_usage += 1
                totals.update({"input_tokens": inp, "output_tokens": out, "total_tokens": inp + out})

    inp = totals["input_tokens"]
    cache = totals["cache_read_tokens"]
    success = max(len(rows) - errors, 0)
    return {
        "prediction_rows": len(rows),
        "unique_sample_ids": len(indices),
        "malformed_jsonl_rows": malformed,
        "error_rows": errors,
        "success_rows": success,
        "latency_seconds": numeric_summary(latencies),
        "ttft_seconds": numeric_summary(ttfts),
        "tpot_seconds": numeric_summary(tpots),
        "reported_usage": {
            "rows_with_usage": rows_with_usage,
            "input_tokens": int(inp),
            "output_tokens": int(totals["output_tokens"]),
            "total_tokens": int(totals["total_tokens"]),
            "reasoning_tokens": int(totals["reasoning_tokens"]),
            "cache_read_tokens": int(cache),
            "cache_write_tokens": int(totals["cache_write_tokens"]),
            "uncached_input_tokens": int(max(inp - cache, 0)),
            "cache_hit_ratio": cache / cache_known_input if cache_known_input else None,
            "coverage": rows_with_usage / len(rows) if rows else None,
            "per_success": {
                "input_tokens": inp / success if success else None,
                "output_tokens": totals["output_tokens"] / success if success else None,
                "reasoning_tokens": totals["reasoning_tokens"] / success if success else None,
            },
        },
    }


def evalscope_report_metrics(job_dir: Path) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for path in sorted(job_dir.rglob("*.json")):
        if "reports" not in path.parts or path.name == "dataset_stats.json":
            continue
        report = load_json(path, {})
        if not isinstance(report, dict):
            continue
        for metric in report.get("metrics") or []:
            ident = metric.get("identity") or {}
            out.append({
                "metric": ident.get("name"),
                "aggregation": ident.get("aggregation"),
                "num": metric.get("num"),
                "score": metric.get("score"),
                "macro_score": metric.get("macro_score"),
                "source": str(path),
            })
    return out


def review_metric_maps(job_dir: Path) -> tuple[dict[str, dict[str, float]], int, int]:
    rows, malformed = review_records(job_dir)
    metrics: dict[str, dict[str, float]] = {}
    for row in rows:
        sample = row.get("sample_score")
        if not isinstance(sample, dict):
            continue
        sample_id = sample.get("sample_id", row.get("index"))
        score = sample.get("score")
        value = score.get("value") if isinstance(score, dict) else None
        if not isinstance(value, dict):
            continue
        for name, raw in value.items():
            val = number(raw)
            if val is not None:
                metrics.setdefault(str(name), {})[str(sample_id)] = val
    return metrics, len(rows), malformed


def parse_key_value_file(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    try:
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            if ": " in line:
                k, v = line.split(": ", 1)
                values[k.strip()] = v.strip()
    except OSError:
        pass
    return values


def client_response_usage(log_dir: Path, exchange: str | None) -> dict[str, Any] | None:
    if not exchange:
        return None
    path = log_dir / f"{exchange}.client-response.txt"
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    marker = "\nbody:\n"
    if marker not in text:
        return None
    body = text.split(marker, 1)[1].strip()
    if not body.startswith("{"):
        return None
    try:
        payload = json.loads(body)
    except json.JSONDecodeError:
        return None
    usage = payload.get("usage")
    return usage if isinstance(usage, dict) else None


def orchestration_index(log_dir: Path) -> list[dict[str, Any]]:
    exchange_session: dict[str, str] = {}
    outcomes: list[dict[str, Any]] = []
    if not log_dir.exists():
        return outcomes
    for path in log_dir.glob("*.orchestration-decision.txt"):
        values = parse_key_value_file(path)
        if values.get("exchange") and values.get("client_session"):
            exchange_session[values["exchange"]] = values["client_session"]
    for path in log_dir.glob("*.orchestration-outcome.txt"):
        values = parse_key_value_file(path)
        exchange = values.get("exchange")
        session = values.get("session") or (exchange_session.get(exchange) if exchange else None)
        raw_path = values.get("path", "").strip().strip("[]").strip()
        phases = [x.strip() for x in raw_path.split(",") if x.strip()] if raw_path else []
        calls: list[str] = []
        raw_calls = values.get("tool_call_ids")
        if raw_calls:
            try:
                parsed = ast.literal_eval(raw_calls)
                if isinstance(parsed, list):
                    calls = [str(x) for x in parsed]
            except (ValueError, SyntaxError):
                pass
        outcomes.append({
            "exchange": exchange,
            "session": session,
            "outcome": values.get("outcome"),
            "ended_with": values.get("ended_with"),
            "path": phases,
            "tool_call_ids": calls,
            "client_usage": client_response_usage(log_dir, exchange),
        })
    return outcomes


def orchestration_summary(session_id: str, log_dir: Path) -> dict[str, Any]:
    matched = [x for x in orchestration_index(log_dir) if x.get("session") == session_id]
    completed = [x for x in matched if x.get("outcome") == "completed"]
    paused = [x for x in matched if x.get("outcome") == "paused"]
    ended = Counter(x.get("ended_with") for x in completed if x.get("ended_with"))
    phase_counts: Counter[str] = Counter()
    pig_counts: list[int] = []
    replans = 0
    for row in completed:
        path = row.get("path") or []
        pig_counts.append(len(path))
        phase_counts.update(path)
        for prev, cur in zip(path, path[1:]):
            if cur == "Pre" and prev in {"Pre", "Post"}:
                replans += 1
    usage_totals: Counter[str] = Counter()
    cache_known_input = 0.0
    usage_rows = 0
    for row in matched:
        usage = row.get("client_usage")
        if not isinstance(usage, dict) or not usage:
            continue
        parsed, cache_known = parse_usage({"usage": usage})
        usage_totals.update(parsed)
        if cache_known:
            cache_known_input += parsed["input_tokens"]
        usage_rows += 1
    cache = usage_totals["cache_read_tokens"]
    observed = len(completed) + len(paused)
    return {
        "observed_orchestrations": observed,
        "completed_orchestrations": len(completed),
        "paused_orchestrations": len(paused),
        "simple_path": ended.get("SIMPLE_PATH", 0),
        "full_path": ended.get("PIGEND", 0),
        "simple_path_ratio": ended.get("SIMPLE_PATH", 0) / observed if observed else None,
        "completed_path_coverage": len(completed) / observed if observed else None,
        "mean_pigs": statistics.fmean(pig_counts) if pig_counts else None,
        "pig_count_distribution": dict(sorted(Counter(pig_counts).items())),
        "phase_counts": dict(phase_counts),
        "replans": replans,
        "tool_calls": sum(len(x.get("tool_call_ids") or []) for x in paused),
        "client_reported_usage": {
            "rows_with_usage": usage_rows,
            "input_tokens": int(usage_totals["input_tokens"]),
            "output_tokens": int(usage_totals["output_tokens"]),
            "total_tokens": int(usage_totals["total_tokens"]),
            "reasoning_tokens": int(usage_totals["reasoning_tokens"]),
            "cache_read_tokens": int(cache),
            "cache_write_tokens": int(usage_totals["cache_write_tokens"]),
            "uncached_input_tokens": int(max(usage_totals["input_tokens"] - cache, 0)),
            "cache_hit_ratio": cache / cache_known_input if cache_known_input else None,
        },
    }


def bfcl_category_from_path(path: Path) -> str | None:
    name = path.name
    for category in BFCL_CATEGORIES:
        if category in name:
            return category
    return None


def bfcl_stats(job_dir: Path) -> tuple[list[dict[str, Any]], dict[str, set[str]], dict[str, set[str]], dict[str, Any]]:
    metrics: list[dict[str, Any]] = []
    correct_sets: dict[str, set[str]] = {}
    result_ids: dict[str, set[str]] = {}
    incorrect_ids: dict[str, set[str]] = {}

    for path in (job_dir / "results").rglob("*_result.json") if (job_dir / "results").exists() else []:
        category = bfcl_category_from_path(path)
        if not category:
            continue
        rows, _ = read_jsonl_files([path])
        result_ids.setdefault(category, set()).update(str(x.get("id")) for x in rows if x.get("id") is not None)

    for path in (job_dir / "scores").rglob("*_score.json") if (job_dir / "scores").exists() else []:
        category = bfcl_category_from_path(path)
        if not category:
            continue
        rows, malformed = read_jsonl_files([path])
        if not rows:
            continue
        header = rows[0]
        metrics.append({
            "metric": category,
            "score": number(header.get("accuracy")),
            "num": header.get("total_count"),
            "correct": header.get("correct_count"),
            "malformed_rows": malformed,
            "source": str(path),
        })
        incorrect_ids.setdefault(category, set()).update(
            str(x.get("id")) for x in rows[1:] if x.get("id") is not None
        )

    by_name = {m["metric"]: m for m in metrics}
    if all(c in by_name for c in BFCL_CATEGORIES):
        values = [by_name[c]["score"] for c in BFCL_CATEGORIES]
        if all(v is not None for v in values):
            metrics.insert(0, {
                "metric": "multi_turn_overall",
                "score": statistics.fmean(float(v) for v in values),
                "num": sum(int(by_name[c].get("num") or 0) for c in BFCL_CATEGORIES),
                "aggregation": "unweighted_mean_of_4_official_categories",
                "source": "official BFCL category score files",
            })
    for category, ids in result_ids.items():
        correct_sets[category] = ids - incorrect_ids.get(category, set())
    if correct_sets:
        correct_sets["multi_turn_overall"] = set().union(*(correct_sets.get(c,set()) for c in BFCL_CATEGORIES))
    universes = {c:set(ids) for c,ids in result_ids.items()}
    if universes:
        universes["multi_turn_overall"] = set().union(*(universes.get(c,set()) for c in BFCL_CATEGORIES))
    details={"generated_by_category":{c:len(ids) for c,ids in result_ids.items()},"incorrect_by_category":{c:len(ids) for c,ids in incorrect_ids.items()}}
    return metrics, correct_sets, universes, details


def find_swe_results(job_dir: Path) -> tuple[Path | None, dict[str, Any] | None]:
    candidates = sorted((job_dir / "scoring").rglob("results.json")) if (job_dir / "scoring").exists() else []
    for path in reversed(candidates):
        data = load_json(path)
        if isinstance(data, dict) and "resolved_instances" in data:
            return path, data
    return None, None


def swe_stats(job_dir: Path) -> tuple[list[dict[str, Any]], set[str], set[str], dict[str, Any]]:
    metrics: list[dict[str, Any]] = []
    resolved: set[str] = set()
    universe: set[str] = set()
    details: dict[str, Any] = {}
    result_path, result = find_swe_results(job_dir)
    if result:
        total = int(result.get("total_instances") or 0)
        resolved_n = int(result.get("resolved_instances") or 0)
        resolved = {str(x) for x in result.get("resolved_ids") or []}
        universe = {str(x) for x in (result.get("submitted_ids") or result.get("completed_ids") or [])}
        metrics.append({
            "metric": "resolved_rate",
            "score": resolved_n / total if total else None,
            "num": total,
            "correct": resolved_n,
            "source": str(result_path),
        })
        details["official_scorer"] = {
            k: result.get(k)
            for k in (
                "total_instances", "submitted_instances", "completed_instances",
                "resolved_instances", "unresolved_instances", "infra_failure_instances",
                "ambiguous_failure_instances", "empty_patch_instances", "error_instances",
                "unstopped_instances",
            )
        }
        details["failure_reasons"] = result.get("failure_reasons") or {}

    preds = load_json(job_dir / "inference" / "preds.json", {})
    details["prediction_count"] = len(preds) if isinstance(preds, dict) else 0
    trajectories = list((job_dir / "inference").rglob("*.traj.json")) if (job_dir / "inference").exists() else []
    exit_status: Counter[str] = Counter()
    api_calls = 0
    costs: list[float] = []
    assistant_turns: list[int] = []
    for path in trajectories:
        data = load_json(path)
        if isinstance(data, list):
            # Some historical/test fixtures use a list wrapper; formal v2 files are dicts.
            data = data[0] if data and isinstance(data[0], dict) else {}
        if not isinstance(data, dict):
            continue
        info = data.get("info") if isinstance(data.get("info"), dict) else {}
        status = info.get("exit_status")
        if status:
            exit_status[str(status)] += 1
        stats = info.get("model_stats") if isinstance(info.get("model_stats"), dict) else {}
        api_calls += int(stats.get("api_calls") or 0)
        cost = number(stats.get("instance_cost"))
        if cost is not None:
            costs.append(cost)
        messages = data.get("messages") if isinstance(data.get("messages"), list) else []
        assistant_turns.append(sum(1 for x in messages if isinstance(x, dict) and x.get("role") == "assistant"))
    details["trajectories"] = {
        "count": len(trajectories),
        "exit_status": dict(exit_status),
        "api_calls": api_calls,
        "instance_cost_sum": sum(costs) if costs else None,
        "assistant_turns": numeric_summary(assistant_turns),
    }
    if not universe and isinstance(preds, dict):
        universe = {str(x) for x in preds}
    return metrics, resolved, universe, details


def exact_mcnemar_p(fixes: int, breaks: int) -> float | None:
    n = fixes + breaks
    if n == 0:
        return 1.0
    k = min(fixes, breaks)
    # Exact two-sided binomial test under p=0.5.
    tail = sum(math.comb(n, i) for i in range(0, k + 1)) / (2 ** n)
    return min(1.0, 2.0 * tail)


def paired_binary(base: dict[str, float] | set[str], pigs: dict[str, float] | set[str], universe: set[str] | None = None) -> dict[str, Any]:
    if isinstance(base, set) and isinstance(pigs, set):
        ids = universe if universe is not None else (base | pigs)
        b = {x: 1.0 if x in base else 0.0 for x in ids}
        p = {x: 1.0 if x in pigs else 0.0 for x in ids}
    else:
        assert isinstance(base, dict) and isinstance(pigs, dict)
        ids = set(base) & set(pigs)
        b, p = base, pigs
    both_correct = both_wrong = fixes = breaks = 0
    for sid in ids:
        bv = 1 if float(b[sid]) >= 0.5 else 0
        pv = 1 if float(p[sid]) >= 0.5 else 0
        if bv and pv: both_correct += 1
        elif not bv and not pv: both_wrong += 1
        elif not bv and pv: fixes += 1
        else: breaks += 1
    n = len(ids)
    return {
        "paired_n": n,
        "base_correct": both_correct + breaks,
        "pigs_correct": both_correct + fixes,
        "base_rate": (both_correct + breaks) / n if n else None,
        "pigs_rate": (both_correct + fixes) / n if n else None,
        "delta": (fixes - breaks) / n if n else None,
        "both_correct": both_correct,
        "both_wrong": both_wrong,
        "fixes": fixes,
        "breaks": breaks,
        "mcnemar_exact_p": exact_mcnemar_p(fixes, breaks),
    }


def paired_numeric(base: dict[str, float], pigs: dict[str, float]) -> dict[str, Any]:
    ids = sorted(set(base) & set(pigs))
    diffs = [pigs[x] - base[x] for x in ids]
    return {
        "paired_n": len(ids),
        "base_mean": statistics.fmean(base[x] for x in ids) if ids else None,
        "pigs_mean": statistics.fmean(pigs[x] for x in ids) if ids else None,
        "delta": statistics.fmean(diffs) if diffs else None,
        "improved": sum(1 for x in diffs if x > 0),
        "regressed": sum(1 for x in diffs if x < 0),
        "tied": sum(1 for x in diffs if x == 0),
    }


def merge_runtime_jobs(manifest: dict[str, Any], progress: dict[str, Any] | None) -> tuple[list[dict[str, Any]], str | None]:
    by_id = {x.get("job_id"): x for x in (progress or {}).get("jobs", []) if isinstance(x, dict)}
    jobs = []
    for raw in manifest.get("jobs") or []:
        job = dict(raw)
        live = by_id.get(job.get("job_id"))
        if live:
            for key in ("status", "completed_samples", "exit_code", "started_at", "finished_at", "command", "thinking_effort"):
                if key in live:
                    job[key] = live[key]
        jobs.append(job)
    return jobs, (progress or {}).get("updated_at")


def headline_metric(dataset: str, metrics: list[dict[str, Any]]) -> dict[str, Any] | None:
    preferred = {
        "gsm8k": ["accuracy"],
        "ifeval": ["prompt_level_strict"],
        "bfcl_multiturn": ["multi_turn_overall"],
        "swebench_lite": ["resolved_rate"],
    }.get(dataset, [])
    by_name = {str(m.get("metric")): m for m in metrics}
    for name in preferred:
        if name in by_name:
            return by_name[name]
    return metrics[0] if metrics else None


def job_common(job: dict[str, Any], updated_at: str | None) -> dict[str, Any]:
    status = str(job.get("status") or "queued")
    expected = int(job.get("expected_samples") or 0)
    completed = int(job.get("completed_samples") or 0)
    elapsed = elapsed_seconds(job.get("started_at"), job.get("finished_at"), updated_at)
    return {
        "job_id": job.get("job_id"),
        "dataset": job.get("dataset"),
        "model_label": job.get("model_label") or str(job.get("model", "")).removesuffix("-pigs"),
        "arm": job.get("arm"),
        "model": job.get("model"),
        "thinking_effort": job.get("thinking_effort"),
        "status": status,
        "terminal": status in TERMINAL,
        "exit_code": job.get("exit_code"),
        "expected_samples": expected,
        "completed_samples": completed,
        "completion_ratio": completed / expected if expected else None,
        "started_at": job.get("started_at"),
        "finished_at": job.get("finished_at"),
        "elapsed_seconds": elapsed,
        "observed_throughput_samples_per_second": completed / elapsed if elapsed and completed else None,
        "output_dir": job.get("output_dir"),
        "session_id": job.get("session_id"),
    }


def summarize_run(run_root: Path, pigs_log_dir: Path | None = None, write_files: bool = True) -> dict[str, Any]:
    run_root = Path(run_root)
    manifest = load_json(run_root / "run_manifest.json")
    if not isinstance(manifest, dict):
        raise FileNotFoundError(f"invalid or missing run manifest: {run_root / 'run_manifest.json'}")
    progress = load_json(run_root / "progress.json", {})
    jobs, updated_at = merge_runtime_jobs(manifest, progress if isinstance(progress, dict) else None)
    dataset = str(manifest.get("dataset") or "")
    global_log_dir = Path(pigs_log_dir or manifest.get("pigs_log_dir") or "logs/http")

    jobs_out: list[dict[str, Any]] = []
    paired_material: dict[tuple[str, str], dict[str, Any]] = {}

    for job in jobs:
        row = job_common(job, updated_at)
        job_dir = Path(str(job.get("output_dir")))
        metrics: list[dict[str, Any]] = []
        material: dict[str, Any] = {}

        if dataset in {"gsm8k", "ifeval"} or manifest.get("harness") == "EvalScope":
            preds, malformed = prediction_records(job_dir)
            pred = prediction_stats(preds, malformed)
            review_maps, review_rows, review_malformed = review_metric_maps(job_dir)
            metrics = evalscope_report_metrics(job_dir)
            terminal_missing = max(row["expected_samples"] - pred["prediction_rows"], 0) if row["terminal"] else None
            pred["final_missing_prediction_rows"] = terminal_missing
            pred["final_execution_failures"] = (
                pred["error_rows"] + pred["malformed_jsonl_rows"] + terminal_missing
                if terminal_missing is not None else None
            )
            row["artifacts"] = {
                "prediction_rows": pred["prediction_rows"],
                "review_rows": review_rows,
                "review_malformed_rows": review_malformed,
            }
            row["predictions"] = pred
            material["review_maps"] = review_maps
            # Fixed benchmark denominator is meaningful for prompt-level task metrics.
            fixed: dict[str, float] = {}
            for name, mapping in review_maps.items():
                if dataset == "gsm8k" or name.startswith("prompt_level_"):
                    fixed[name] = sum(mapping.values()) / row["expected_samples"] if row["expected_samples"] else 0.0
            row["fixed_denominator_scores"] = fixed

        elif dataset == "bfcl_multiturn":
            metrics, correct, universes, details = bfcl_stats(job_dir)
            material["correct_sets"] = correct
            material["universes"] = universes
            generated = len(universes.get("multi_turn_overall", set()))
            row["artifacts"] = {"generated_tasks": generated}
            row["bfcl"] = details
            row["final_missing_tasks"] = max(row["expected_samples"] - generated, 0) if row["terminal"] else None

        elif dataset == "swebench_lite":
            metrics, resolved, universe, details = swe_stats(job_dir)
            material["resolved"] = resolved
            material["universe"] = universe
            row["swebench"] = details
            row["artifacts"] = {"prediction_count": details.get("prediction_count", 0), "scored_tasks": len(universe)}
            row["final_missing_tasks"] = max(row["expected_samples"] - len(universe), 0) if row["terminal"] else None
        else:
            preds, malformed = prediction_records(job_dir)
            row["predictions"] = prediction_stats(preds, malformed)
            metrics = evalscope_report_metrics(job_dir)

        row["official_metrics"] = metrics
        row["headline_metric"] = headline_metric(dataset, metrics)

        if row["arm"] == "pigs":
            local = job_dir / "pigs-http"
            selected = local if any(local.glob("*.txt")) else global_log_dir
            row["orchestration_log_dir"] = str(selected)
            row["orchestration"] = orchestration_summary(str(job.get("session_id") or ""), selected)

        paired_material[(str(row["model_label"]), str(row["arm"]))] = material
        jobs_out.append(row)

    comparisons: list[dict[str, Any]] = []
    labels = sorted({str(x["model_label"]) for x in jobs_out})
    for label in labels:
        base = next((x for x in jobs_out if x["model_label"] == label and x["arm"] == "base"), None)
        pigs = next((x for x in jobs_out if x["model_label"] == label and x["arm"] == "pigs"), None)
        if not base or not pigs:
            continue
        comp: dict[str, Any] = {
            "model_label": label,
            "dataset": dataset,
            "thinking_effort": manifest.get("thinking_effort") or base.get("thinking_effort"),
            "official_metric_deltas": [],
            "paired": {},
        }
        bm = {str(x.get("metric")): x for x in base.get("official_metrics", [])}
        pm = {str(x.get("metric")): x for x in pigs.get("official_metrics", [])}
        for name in sorted(set(bm) & set(pm)):
            bv, pv = number(bm[name].get("score")), number(pm[name].get("score"))
            if bv is not None and pv is not None:
                comp["official_metric_deltas"].append({"metric": name, "base": bv, "pigs": pv, "delta": pv - bv})

        bmat = paired_material.get((label, "base"), {})
        pmat = paired_material.get((label, "pigs"), {})
        if dataset in {"gsm8k", "ifeval"}:
            bmaps = bmat.get("review_maps", {})
            pmaps = pmat.get("review_maps", {})
            for name in sorted(set(bmaps) & set(pmaps)):
                vals = list(bmaps[name].values()) + list(pmaps[name].values())
                binary = all(v in (0.0, 1.0) for v in vals)
                comp["paired"][name] = paired_binary(bmaps[name], pmaps[name]) if binary else paired_numeric(bmaps[name], pmaps[name])
        elif dataset == "bfcl_multiturn":
            bc, pc = bmat.get("correct_sets", {}), pmat.get("correct_sets", {})
            bu, pu = bmat.get("universes", {}), pmat.get("universes", {})
            for name in sorted(set(bc) & set(pc)):
                universe = set(bu.get(name, set())) & set(pu.get(name, set()))
                comp["paired"][name] = paired_binary(set(bc[name]), set(pc[name]), universe)
        elif dataset == "swebench_lite":
            universe = set(bmat.get("universe", set())) & set(pmat.get("universe", set()))
            if universe:
                comp["paired"]["resolved"] = paired_binary(
                    set(bmat.get("resolved", set())),
                    set(pmat.get("resolved", set())),
                    universe,
                )
        comparisons.append(comp)

    statuses = {str(x.get("status")) for x in jobs_out}
    if any(s in {"running", "queued"} for s in statuses):
        run_status = "running"
    elif "failed" in statuses:
        run_status = "failed"
    else:
        run_status = "done"

    summary = {
        "schema_version": 2,
        "generated_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "run_id": manifest.get("run_id"),
        "run_root": str(run_root),
        "status": run_status,
        "dataset": dataset,
        "harness": manifest.get("harness"),
        "thinking_effort": manifest.get("thinking_effort"),
        "model_workers": manifest.get("model_workers"),
        "sample_workers": manifest.get("sample_workers"),
        "repo_commit": manifest.get("repo_commit"),
        "output_token_limit_policy": manifest.get("output_token_limit_policy"),
        "progress_updated_at": updated_at,
        "jobs": jobs_out,
        "comparisons": comparisons,
        "usage_caveat": "Endpoint-reported token usage is not accumulated across every internal PIGS phase call.",
    }
    if write_files:
        write_json(run_root / "summary.json", summary)
        atomic_write(run_root / "summary.md", build_markdown(summary))
        write_summary_csv(run_root / "summary.csv", summary)
    return summary


def fmt(value: Any, digits: int = 4) -> str:
    return f"{value:.{digits}f}" if isinstance(value, (int, float)) else "-"


def pct(value: Any) -> str:
    return f"{100 * value:.2f}%" if isinstance(value, (int, float)) else "-"


def build_markdown(summary: dict[str, Any]) -> str:
    lines = [
        f"# Evaluation summary — {summary['run_id']}", "",
        f"- Status: **{summary['status']}**",
        f"- Dataset: {summary['dataset']}",
        f"- Harness: {summary.get('harness')}",
        f"- Thinking effort: {summary.get('thinking_effort')}",
        f"- Concurrency: model_workers={summary.get('model_workers')}, sample_workers={summary.get('sample_workers')}",
        f"- Progress updated: {summary.get('progress_updated_at')}", "",
    ]
    if summary["status"] == "running":
        lines += ["> Live snapshot: unattempted samples are not counted as failures until a job becomes terminal.", ""]
    lines += [
        "## Jobs", "",
        "| Model | Arm | Status | Persisted/Expected | Headline | Score | Latency mean | Reasoning tok | Final missing | Simple path | Mean pigs |",
        "|---|---|---|---:|---|---:|---:|---:|---:|---:|---:|",
    ]
    for job in summary["jobs"]:
        head = job.get("headline_metric") or {}
        pred = job.get("predictions") or {}
        usage = pred.get("reported_usage") or {}
        artifacts = job.get("artifacts") or {}
        persisted = artifacts.get("prediction_rows", artifacts.get("generated_tasks", artifacts.get("scored_tasks", 0)))
        missing = pred.get("final_missing_prediction_rows", job.get("final_missing_tasks"))
        latency = (pred.get("latency_seconds") or {}).get("mean")
        orch = job.get("orchestration") or {}
        lines.append(
            f"| {job['model_label']} | {job['arm']} | {job['status']} | {persisted}/{job['expected_samples']} | "
            f"{head.get('metric','-')} | {pct(head.get('score'))} | {fmt(latency,3)} | "
            f"{usage.get('reasoning_tokens','-')} | {missing if missing is not None else '-'} | "
            f"{pct(orch.get('simple_path_ratio'))} | {fmt(orch.get('mean_pigs'),2)} |"
        )
    lines += ["", "## Official benchmark metrics", "", "| Model | Arm | Metric | N | Score |", "|---|---|---|---:|---:|"]
    any_metric = False
    for job in summary["jobs"]:
        for metric in job.get("official_metrics") or []:
            any_metric = True
            lines.append(f"| {job['model_label']} | {job['arm']} | {metric.get('metric')} | {metric.get('num','-')} | {pct(metric.get('score'))} |")
    if not any_metric:
        lines.append("| - | - | official scorer has not produced final metrics yet | - | - |")
    if summary.get("comparisons"):
        lines += ["", "## Base vs PIGS", ""]
        for comp in summary["comparisons"]:
            lines.append(f"### {comp['model_label']} — {comp.get('thinking_effort')}")
            lines.append("")
            if comp.get("official_metric_deltas"):
                lines += ["| Metric | Base | PIGS | Delta |", "|---|---:|---:|---:|"]
                for row in comp["official_metric_deltas"]:
                    lines.append(f"| {row['metric']} | {pct(row['base'])} | {pct(row['pigs'])} | {pct(row['delta'])} |")
                lines.append("")
            if comp.get("paired"):
                lines += ["| Paired metric | N | Base | PIGS | Delta | Fixes | Breaks | McNemar p |", "|---|---:|---:|---:|---:|---:|---:|---:|"]
                for name, row in comp["paired"].items():
                    lines.append(
                        f"| {name} | {row.get('paired_n','-')} | {pct(row.get('base_rate',row.get('base_mean')))} | "
                        f"{pct(row.get('pigs_rate',row.get('pigs_mean')))} | {pct(row.get('delta'))} | "
                        f"{row.get('fixes',row.get('improved','-'))} | {row.get('breaks',row.get('regressed','-'))} | {fmt(row.get('mcnemar_exact_p'),4)} |"
                    )
                lines.append("")
    lines += ["## Notes", "", f"- {summary['usage_caveat']}", "- Official quality scores come from each benchmark's scorer artifacts; this script does not re-score answers.", ""]
    return "\n".join(lines)


def write_summary_csv(path: Path, summary: dict[str, Any]) -> None:
    fields = [
        "run_id", "dataset", "thinking_effort", "model_label", "arm", "model", "status",
        "expected_samples", "completed_samples", "headline_metric", "headline_score",
        "latency_mean_s", "ttft_mean_s", "tpot_mean_s", "input_tokens", "output_tokens",
        "reasoning_tokens", "cache_read_tokens", "cache_hit_ratio", "simple_path_ratio",
        "mean_pigs", "replans", "tool_calls",
    ]
    rows = []
    for job in summary["jobs"]:
        pred = job.get("predictions") or {}
        usage = pred.get("reported_usage") or {}
        orch = job.get("orchestration") or {}
        head = job.get("headline_metric") or {}
        rows.append({
            "run_id": summary["run_id"], "dataset": summary["dataset"], "thinking_effort": summary.get("thinking_effort"),
            "model_label": job["model_label"], "arm": job["arm"], "model": job["model"], "status": job["status"],
            "expected_samples": job["expected_samples"], "completed_samples": job["completed_samples"],
            "headline_metric": head.get("metric"), "headline_score": head.get("score"),
            "latency_mean_s": (pred.get("latency_seconds") or {}).get("mean"),
            "ttft_mean_s": (pred.get("ttft_seconds") or {}).get("mean"),
            "tpot_mean_s": (pred.get("tpot_seconds") or {}).get("mean"),
            "input_tokens": usage.get("input_tokens"), "output_tokens": usage.get("output_tokens"),
            "reasoning_tokens": usage.get("reasoning_tokens"), "cache_read_tokens": usage.get("cache_read_tokens"),
            "cache_hit_ratio": usage.get("cache_hit_ratio"), "simple_path_ratio": orch.get("simple_path_ratio"),
            "mean_pigs": orch.get("mean_pigs"), "replans": orch.get("replans"), "tool_calls": orch.get("tool_calls"),
        })
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Summarize one PIGS evaluation run, live or completed.")
    p.add_argument("run_dir", type=Path)
    p.add_argument("--pigs-log-dir", type=Path, default=None)
    p.add_argument("--no-write", action="store_true")
    return p.parse_args()


def main() -> int:
    args = parse_args()
    summary = summarize_run(args.run_dir, args.pigs_log_dir, write_files=not args.no_write)
    compact = {
        "run_id": summary["run_id"],
        "status": summary["status"],
        "dataset": summary["dataset"],
        "thinking_effort": summary.get("thinking_effort"),
        "jobs": [
            {"job_id": j["job_id"], "status": j["status"], "completed": j["completed_samples"], "expected": j["expected_samples"], "headline": j.get("headline_metric")}
            for j in summary["jobs"]
        ],
    }
    print(json.dumps(compact, ensure_ascii=False, indent=2))
    if not args.no_write:
        print(args.run_dir / "summary.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
