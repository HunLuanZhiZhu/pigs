#!/usr/bin/env python3
"""Prepare a safe BFCL V4 Multi-Turn resume from saved results.

Keep successfully returned *structured responses*, regardless of whether the
official scorer considers the model answer correct. Remove only explicit
inference error placeholders, so the official generator will retry those IDs.

This program NEVER sends model requests, scores tests, or modifies source files.
By default it only inspects; --prepare copies clean records to an *empty* target
after verifying that the original stage controller/worker is stopped.

Once prepared, run the existing bfcl_official_adapter.py with the SAME
--registry-name as in the source, with --result-dir TARGET and without
--allow-overwrite / --run-ids. BFCL skips existing IDs automatically.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

CATEGORIES = (
    "multi_turn_base",
    "multi_turn_miss_func",
    "multi_turn_miss_param",
    "multi_turn_long_context",
)
EXPECTED_PER_CATEGORY = 200


def pid_running(pid: int | None) -> bool:
    if not isinstance(pid, int) or pid < 1:
        return False
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def stage_state(source: Path) -> dict | None:
    # Typical source: <run-root>/<stage>/attempt-N/results
    for parent in source.parents:
        manifest_path = parent / "manifest.json"
        if not manifest_path.is_file():
            continue
        try:
            run = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        for stage in run.get("stages", []):
            directory = stage.get("dir")
            if directory and (Path(directory).resolve() / "results") == source:
                return {
                    "stage_number": stage.get("number"),
                    "stage_status": stage.get("status"),
                    "worker_pid": stage.get("pid"),
                    "controller_pid": run.get("controller_pid"),
                    "run_id": run.get("run_id"),
                }
    return None


def select(source: Path) -> tuple[list[tuple[str, Path, list[dict]]], dict]:
    if not source.is_dir():
        raise ValueError(f"Source results directory not found: {source}")
    model_dirs = [p for p in source.iterdir() if p.is_dir()]
    if len(model_dirs) != 1:
        raise ValueError("Source must contain exactly one BFCL model directory")
    registry_name = model_dirs[0].name
    selected: list[tuple[str, Path, list[dict]]] = []
    summary: dict = {
        "source": str(source),
        "registry_name": registry_name,
        "categories": {},
        "kept": 0,
        "retry": 0,
        "missing": 0,
    }
    for category in CATEGORIES:
        path = model_dirs[0] / "multi_turn" / f"BFCL_v4_{category}_result.json"
        rows: list[dict] = []
        if path.is_file():
            for line_num, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                if not line.strip():
                    continue
                try:
                    item = json.loads(line)
                except json.JSONDecodeError as exc:
                    raise ValueError(f"Malformed JSON at {path}:{line_num}: {exc}") from exc
                if not isinstance(item, dict) or "id" not in item:
                    raise ValueError(f"Malformed result at {path}:{line_num}")
                rows.append(item)
        seen: set[str] = set()
        kept: list[dict] = []
        retry: list[dict] = []
        for item in rows:
            identifier = item["id"]
            prefix = f"{category}_"
            if not isinstance(identifier, str) or not identifier.startswith(prefix):
                raise ValueError(f"Unexpected category or sample ID: {identifier}")
            try:
                serial = int(identifier[len(prefix):])
            except ValueError as exc:
                raise ValueError(f"Invalid sample ID: {identifier}") from exc
            if not 0 <= serial < EXPECTED_PER_CATEGORY:
                raise ValueError(f"Out-of-range sample ID: {identifier}")
            if identifier in seen:
                raise ValueError(f"Duplicate result sample ID: {identifier}")
            seen.add(identifier)
            if isinstance(item.get("result"), list):
                kept.append(item)
            else:
                retry.append({
                    "id": identifier,
                    "reason": str(item.get("result", "<missing result>"))[:300],
                })
        expected_ids = {f"{category}_{i}" for i in range(EXPECTED_PER_CATEGORY)}
        missing = sorted(expected_ids - seen, key=lambda x: int(x.rsplit("_", 1)[1]))
        cat_status = {
            "recorded": len(rows), "kept": len(kept),
            "retry_count": len(retry), "retry": retry,
            "missing_count": len(missing), "missing_ids": missing,
            "pending_after_clean": len(retry) + len(missing),
        }
        summary["categories"][category] = cat_status
        summary["kept"] += len(kept)
        summary["retry"] += len(retry)
        summary["missing"] += len(missing)
        selected.append((category, path, kept))
    summary["total"] = 800
    summary["pending_after_clean"] = summary["retry"] + summary["missing"]
    summary["stage"] = stage_state(source)
    return selected, summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", type=Path, required=True,
                        help="Existing BFCL stage results directory (not a file)")
    parser.add_argument("--prepare", action="store_true",
                        help="Create clean copy of structured outputs (no API calls)")
    parser.add_argument("--target", type=Path,
                        help="New, NONEXISTENT result directory for --prepare")
    args = parser.parse_args()
    source = args.source.resolve()
    try:
        selected, summary = select(source)
        print(json.dumps(summary, ensure_ascii=False, indent=2))
        if not args.prepare:
            print("\nInspection only: no file changes, no API calls.")
            return 0
        if args.target is None:
            parser.error("--target is required with --prepare")
        target = args.target.resolve()
        if target.exists():
            raise ValueError(f"Target already exists; refusing overwrite: {target}")
        if target == source or source in target.parents or target in source.parents:
            raise ValueError("Source and target directories must not be nested")
        stage = summary["stage"]
        if stage and (pid_running(stage["worker_pid"]) or pid_running(stage["controller_pid"])):
            raise RuntimeError(
                "Original BFCL controller/worker is still alive. Stop it and "
                "verify it has exited before preparing a recovery copy."
            )
        target.mkdir(parents=True, exist_ok=False)
        for category, _original_path, rows in selected:
            if not rows:
                continue
            destination = target / summary["registry_name"] / "multi_turn" / (
                f"BFCL_v4_{category}_result.json"
            )
            destination.parent.mkdir(parents=True, exist_ok=True)
            with destination.open("w", encoding="utf-8") as stream:
                for item in rows:
                    stream.write(json.dumps(item, ensure_ascii=False) + "\n")
        (target / "recovery_manifest.json").write_text(
            json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        print(f"\nRecovery copy prepared: {target}")
        print(f"Retained {summary['kept']} completed responses; "
              f"BFCL will retry {summary['pending_after_clean']} IDs.")
        print("No original result files were changed. No API calls were made.")
        return 0
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
