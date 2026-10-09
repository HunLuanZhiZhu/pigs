#!/usr/bin/env python3
"""Merge exactly one completed SWE-bench retry into an incomplete base run.

Preserves first-pass artifacts and writes explicit heterogeneous-timeout provenance.
Run only once; refuses to overwrite backups or reuse an existing sample.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from datetime import datetime, timezone
from pathlib import Path

import yaml


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--original-inference", type=Path, required=True)
    parser.add_argument("--retry-inference", type=Path, required=True)
    parser.add_argument("--sample-id", required=True)
    args = parser.parse_args()
    original = args.original_inference
    retry = args.retry_inference
    sample = args.sample_id
    original_file = original / "preds.json"
    retry_file = retry / "preds.json"
    backup = original / "preds_original_299_20261008.json"
    provenance = original / "supplemental_integration_20261008.json"

    primary = json.loads(original_file.read_text(encoding="utf-8"))
    supplement = json.loads(retry_file.read_text(encoding="utf-8"))
    assert len(primary) == 299 and sample not in primary
    assert list(supplement) == [sample]
    submission = supplement[sample]
    assert submission["instance_id"] == sample
    assert submission["model_patch"].startswith("diff --git ")
    trajectory = retry / sample / f"{sample}.traj.json"
    trace = json.loads(trajectory.read_text(encoding="utf-8"))
    assert trace["instance_id"] == sample and trace["info"]["exit_status"] == "Submitted"
    assert not backup.exists() and not provenance.exists()
    assert not (original / sample).exists()
    merged = primary | supplement
    assert len(merged) == 300

    # Save originals first, then atomically replace predictions.
    shutil.copy2(original_file, backup)
    orig_sha = hashlib.sha256(backup.read_bytes()).hexdigest()
    tmp = original / "preds.supplement.tmp"
    tmp.write_text(json.dumps(merged, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    tmp.replace(original_file)
    target_dir = original / sample
    target_dir.mkdir()
    shutil.copy2(trajectory, target_dir / trajectory.name)

    status_files = list(original.glob("exit_statuses_*.yaml"))
    assert len(status_files) == 1
    status_file = status_files[0]
    shutil.copy2(status_file, original / "exit_statuses_original_299_20261008.yaml")
    statuses = yaml.safe_load(status_file.read_text(encoding="utf-8"))
    groups = statuses["instances_by_exit_status"]
    assert not any(sample in ids for ids in groups.values())
    groups.setdefault("Submitted", []).append(sample)
    status_file.write_text(yaml.safe_dump(statuses, sort_keys=False), encoding="utf-8")

    meta = {
        "integrated_at": datetime.now(timezone.utc).isoformat(),
        "sample_id": sample,
        "original_count": len(primary),
        "supplement_count": len(supplement),
        "merged_count": len(merged),
        "original_backup": str(backup),
        "original_sha256": orig_sha,
        "retry_source": str(retry),
        "retry_exit_status": "Submitted",
        "retry_command_timeout_seconds": 60,
        "original_command_timeout_seconds": 1000000,
        "model": "deepseek-v4.1-flash",
        "reasoning_effort": "low",
        "official_scorer_status": "pending",
        "note": "Supplementary rerun, not a homogeneous first-pass 300-sample inference."
    }
    provenance.write_text(json.dumps(meta, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    assert len(json.loads(original_file.read_text(encoding="utf-8"))) == 300
    print(json.dumps({"merged": 300, "supplemental": sample, "backup": str(backup)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
