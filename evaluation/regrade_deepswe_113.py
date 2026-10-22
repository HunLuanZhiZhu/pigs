#!/usr/bin/env python3
"""复用已有 model.patch 独立重评 DeepSWE。 / Replay DeepSWE verifiers without agent calls."""
from concurrent.futures import ThreadPoolExecutor, as_completed
from collections import Counter
from datetime import datetime
import csv
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import tomllib
import traceback

TASKS = Path("/mnt/d/AIWorkspace/DeepSWE/deep-swe/tasks")
BASE = Path("/root/pigs-eval/outputs/deepswe-pig/ds-pig-v11-full-fixed-20261022")
DEST = Path("/root/pigs-eval/outputs/deepswe-pig-regrade-lf-20261022")
WORKERS = 2
TIMEOUT_SECONDS = 1900


def write_json(path, value):
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
    os.replace(temp, path)


def replay(trial):
    task_name, src, patch, image, old_reward = trial
    output = DEST / "trials" / task_name
    verifier = output / "verifier"
    verifier.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    container_name = "pigs-regrade-" + task_name.replace("_", "-").lower()
    command = [
        "docker", "run", "--pull=never", "--rm", "--network=none",
        "--name", container_name, "--cpus=2", "--memory=8g",
        "--mount", f"type=bind,src={src / 'tests'},dst=/tests,readonly",
        "--mount", f"type=bind,src={patch.parent},dst=/logs/artifacts,readonly",
        "--mount", f"type=bind,src={verifier},dst=/logs/verifier",
        "--entrypoint", "/bin/bash", image, "/tests/test.sh",
    ]
    rc = None
    error = None
    with (output / "docker-run.log").open("wb") as logfile:
        try:
            rc = subprocess.run(
                command, stdout=logfile, stderr=subprocess.STDOUT,
                timeout=TIMEOUT_SECONDS, check=False
            ).returncode
        except Exception as exc:
            error = repr(exc)
            # 超时确保清理容器。 / Ensure timed-out containers are removed.
            try:
                subprocess.run(
                    ["docker", "rm", "-f", container_name],
                    capture_output=True, timeout=30, check=False
                )
            except Exception:
                pass

    reward = None
    metrics = {}
    if (verifier / "reward.json").exists():
        try:
            metrics = json.loads((verifier / "reward.json").read_text())
            reward = metrics.get("reward")
        except Exception as exc:
            error = (error or "") + "; bad reward.json: " + repr(exc)
    elif (verifier / "reward.txt").exists():
        try:
            reward = float((verifier / "reward.txt").read_text().strip())
        except ValueError:
            pass

    status = "scored" if reward is not None else "unscored"
    if error:
        status = "error"
    elif rc not in (0, None):
        status = "scored_nonzero_exit" if reward is not None else "runtime_error"
    result = {
        "name": task_name,
        "old_reward": old_reward,
        "new_reward": reward,
        "status": status,
        "docker_rc": rc,
        "seconds": round(time.monotonic() - started, 2),
        "f2p_passed": metrics.get("f2p_passed"),
        "f2p_total": metrics.get("f2p_total"),
        "p2p_passed": metrics.get("p2p_passed"),
        "p2p_total": metrics.get("p2p_total"),
        "partial": metrics.get("partial"),
        "error": error,
    }
    write_json(output / "result.json", result)
    return result


def main():
    if DEST.exists():
        raise SystemExit(f"Refusing to overwrite existing results: {DEST}")
    trials = []
    for folder in sorted(BASE.iterdir()):
        result_path = folder / "result.json"
        if not folder.is_dir() or not result_path.is_file():
            continue
        result = json.loads(result_path.read_text())
        src = TASKS / Path(result["task_id"]["path"]).name
        patch = folder / "artifacts" / "model.patch"
        image = tomllib.loads((src / "task.toml").read_text())["environment"]["docker_image"]
        assert patch.is_file(), patch
        assert b"\r\n" not in (src / "tests" / "test.patch").read_bytes(), src
        old_reward = result.get("verifier_result", {}).get("rewards", {}).get("reward")
        trials.append((folder.name, src, patch, image, old_reward))
    assert len(trials) == 113, f"Expected 113 trials; found {len(trials)}"

    DEST.mkdir(parents=True)
    rows = []
    progress = {
        "started_at": datetime.now().isoformat(), "finished_at": None,
        "total": 113, "completed": 0, "running": WORKERS,
        "pending": 113 - WORKERS, "workers": WORKERS,
        "counts": {}, "by_original_reward": {}, "recent": [],
        "model_api_calls": 0, "original_results": str(BASE),
        "results_dir": str(DEST),
    }
    write_json(DEST / "progress.json", progress)
    fields = (
        "name old_reward new_reward status docker_rc seconds "
        "f2p_passed f2p_total p2p_passed p2p_total partial error"
    ).split()
    print(f"START {datetime.now().isoformat()} verifiers=113 workers={WORKERS}", flush=True)
    with (DEST / "results.csv").open("w", encoding="utf-8", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=fields)
        writer.writeheader()
        output.flush()
        with ThreadPoolExecutor(max_workers=WORKERS) as pool:
            futures = {pool.submit(replay, trial): trial for trial in trials}
            for future in as_completed(futures):
                try:
                    row = future.result()
                except Exception:
                    task = futures[future]
                    row = {
                        "name": task[0], "old_reward": task[4], "new_reward": None,
                        "status": "runner_exception", "docker_rc": None,
                        "seconds": None, "f2p_passed": None, "f2p_total": None,
                        "p2p_passed": None, "p2p_total": None, "partial": None,
                        "error": traceback.format_exc()[-1200:],
                    }
                rows.append(row)
                writer.writerow(row)
                output.flush()
                remaining = 113 - len(rows)
                progress["completed"] = len(rows)
                progress["running"] = min(WORKERS, remaining)
                progress["pending"] = max(0, remaining - WORKERS)
                progress["counts"] = dict(Counter(str(r["new_reward"]) for r in rows))
                by_old = {}
                for r in rows:
                    k = str(r["old_reward"])
                    by_old.setdefault(k, {})
                    n = str(r["new_reward"])
                    by_old[k][n] = by_old[k].get(n, 0) + 1
                progress["by_original_reward"] = by_old
                progress["recent"] = [
                    {"name": r["name"], "reward": r["new_reward"], "status": r["status"]}
                    for r in rows[-8:]
                ]
                if not remaining:
                    progress["finished_at"] = datetime.now().isoformat()
                write_json(DEST / "progress.json", progress)
                print(
                    f"{len(rows)}/113 {row['name']} old={row['old_reward']} "
                    f"new={row['new_reward']} status={row['status']} "
                    f"seconds={row['seconds']}", flush=True
                )
    print("DONE", progress["counts"], flush=True)


if __name__ == "__main__":
    main()
