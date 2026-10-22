#!/usr/bin/env python3
# -*- coding: utf-8 -*-
r"""Read-only live monitor for evaluation/download_deepswe_images.py.

Windows PowerShell:
    python evaluation\monitor_deepswe_images.py
    python evaluation\monitor_deepswe_images.py --once

Ctrl+C stops the monitor, NOT the Docker downloads.
"""
from __future__ import annotations

import argparse
from datetime import datetime
import json
from pathlib import Path
import shutil
import sys
import time

LOGS = Path(__file__).resolve().parent.parent / "logs" / "deepswe-images"
DEFAULT_TOTAL = 113


def latest_job(logs: Path) -> Path:
    jobs = sorted((p for p in logs.iterdir() if p.is_dir()), key=lambda p: p.name)
    if not jobs:
        raise FileNotFoundError(f"No download job logs found in {logs}")
    return jobs[-1]


def last_line(path: Path) -> str:
    try:
        with path.open("rb") as stream:
            stream.seek(0, 2)
            size = stream.tell()
            stream.seek(max(0, size - 16384))
            data = stream.read().decode("utf-8", "replace")
        items = [v.strip() for v in data.replace("\r", "\n").splitlines()
                 if v.strip() and not v.startswith("===")]
        return items[-1][-120:] if items else "(no Docker output yet)"
    except OSError:
        return "(log currently inaccessible)"


def report(job: Path, total: int) -> None:
    status = job / "status.json"
    try:
        outcomes = json.loads(status.read_text(encoding="utf-8")) if status.exists() else {}
    except (OSError, ValueError):
        outcomes = {}
    ok = sum(v.get("result") in ("cached", "downloaded") for v in outcomes.values())
    failed = sum(v.get("result") == "failed" for v in outcomes.values())
    done = len(outcomes)
    active_files = [
        p for p in job.glob("*.log")
        if p.stem not in outcomes
    ]
    active_files.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    width = 30
    filled = round(min(1.0, done / max(total, 1)) * width)
    progress = f"[{'#' * filled}{'.' * (width-filled)}] {100.0*done/max(total,1):5.1f}%"
    try:
        free_gib = shutil.disk_usage("D:\\").free / (1024 ** 3)
        free = f"{free_gib:,.1f} GiB"
    except OSError:
        free = "unknown"
    print(f"\n[{datetime.now():%Y-%m-%d %H:%M:%S}] DeepSWE | {job.name}", flush=True)
    print(f"TOTAL {progress} | OK {ok}/{total} | FAIL {failed} | "
          f"CURRENT {len(active_files)} | REMAINING {total-done} | D: free {free}", flush=True)
    for p in active_files[:8]:
        age = int(time.time() - p.stat().st_mtime)
        print(f"  {p.stem[:37]:37} {last_line(p)} | last output {age}s ago", flush=True)
    if failed:
        bad = [k for k, v in outcomes.items() if v.get("result") == "failed"]
        print(f"  Failed: {', '.join(bad[:5])}" + (" ..." if len(bad) > 5 else ""), flush=True)
    if done == total:
        print("All tasks have a recorded result. Re-run download script to retry failures.", flush=True)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--once", action="store_true", help="show one snapshot then exit")
    p.add_argument("--interval", type=float, default=5.0, help="refresh seconds (default 5)")
    p.add_argument("--job", type=Path, help="exact log job directory")
    p.add_argument("--total", type=int, default=DEFAULT_TOTAL, help="total task count (default 113)")
    args = p.parse_args()
    if args.total < 1 or args.interval < 1:
        p.error("--total and --interval must be positive")
    job = args.job or latest_job(LOGS)
    print(f"Monitoring (read-only): {job}", flush=True)
    print("Ctrl+C exits the monitor only. Downloads keep running.", flush=True)
    try:
        while True:
            report(job, args.total)
            if args.once:
                return
            time.sleep(args.interval)
    except KeyboardInterrupt:
        print("\nMonitor stopped; downloads were not interrupted.", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (FileNotFoundError, NotADirectoryError, PermissionError) as exc:
        print(f"Monitor error: {exc}", file=sys.stderr)
        sys.exit(1)
