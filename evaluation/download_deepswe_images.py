#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Parallel DeepSWE v1.1 Docker image downloader for Windows + Ubuntu WSL.

No third-party Python packages required. Examples:
    python evaluation/download_deepswe_images.py --dry-run
    python evaluation/download_deepswe_images.py --probe-only
    python evaluation/download_deepswe_images.py --workers 4 --limit 3
    python evaluation/download_deepswe_images.py --workers 4

The official task.toml is the source of truth. All images are retained under
their OFFICIAL public.ecr.aws names so Pier needs no task configuration edits.
Mirror support uses full repository-prefix replacement, NOT Docker Hub's
registry-mirrors setting (which does not accelerate ECR Public).
"""

from __future__ import annotations

import argparse
from collections import deque
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from datetime import datetime
import json
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import sys
import threading
import time
import tomllib
from urllib import error, parse, request


DEFAULT_TASKS = Path(r"D:\AIWorkspace\DeepSWE\deep-swe\tasks")
DEFAULT_LOG_ROOT = Path(__file__).resolve().parent.parent / "logs" / "deepswe-images"
DEFAULT_MIRRORS = ("m.daocloud.io",)
ORIGINAL_PREFIX = "public.ecr.aws/"
ACCEPT_MANIFEST = ", ".join((
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
    "application/vnd.docker.distribution.manifest.v2+json",
))


@dataclass(frozen=True)
class TaskImage:
    task: str
    image: str


def stamp(message: str) -> None:
    print(f"[{datetime.now():%H:%M:%S}] {message}", flush=True)


def bar(done: int, total: int, width: int = 26) -> str:
    percent = min(1.0, done / max(1, total))
    fill = round(width * percent)
    return f"[{'#' * fill}{'.' * (width - fill)}] {percent * 100:5.1f}%"


def format_time(seconds: float) -> str:
    sec = max(0, int(seconds))
    return f"{sec // 3600:02d}:{sec // 60 % 60:02d}:{sec % 60:02d}"


def gib(value: int) -> str:
    return f"{value / (1024**3):.1f} GiB"


def wsl_docker(*args: str) -> list[str]:
    # The Docker *daemon* is in Ubuntu WSL, not Windows Docker Desktop.
    # Do not accidentally pass the malformed WSL separator '---'.
    return ["wsl.exe", "-d", "Ubuntu-24.04", "-u", "root", "docker", *args]


def quick_command(args: list[str], timeout: float = 45) -> str:
    proc = subprocess.run(args, capture_output=True, timeout=timeout, check=False)
    output = (proc.stdout + proc.stderr).decode("utf-8", "replace").strip()
    if proc.returncode:
        raise RuntimeError(f"command exited {proc.returncode}: {output[-450:]}")
    return output


def discover_tasks(root: Path) -> list[TaskImage]:
    if not root.is_dir():
        raise FileNotFoundError(f"Cannot locate official DeepSWE tasks: {root}")
    tasks: list[TaskImage] = []
    for path in sorted(root.glob("*/task.toml")):
        doc = tomllib.loads(path.read_text(encoding="utf-8"))
        image = doc.get("environment", {}).get("docker_image")
        if not isinstance(image, str) or not image.startswith(ORIGINAL_PREFIX):
            raise RuntimeError(f"Unexpected/missing ECR image in {path}: {image!r}")
        tasks.append(TaskImage(task=path.parent.name, image=image))
    if not tasks:
        raise RuntimeError("No task.toml files found")
    if len({task.image for task in tasks}) != len(tasks):
        raise RuntimeError("Duplicate Docker image references; inspect the benchmark version")
    return tasks


def normalize_mirror(raw: str) -> str:
    source = raw.strip().removeprefix("https://").removeprefix("http://").strip("/")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]*(?:/[A-Za-z0-9._/-]+)?", source):
        raise ValueError(f"Invalid registry mirror: {raw!r}")
    return source


def mirror_ref(mirror: str, image: str) -> str:
    # Example: m.daocloud.io/public.ecr.aws/d3j8x8q7/repo:tag
    # This is NOT equivalent to m.daocloud.io/d3j8x8q7/repo:tag.
    return f"{mirror}/{image}"


def registry_probe(mirror: str, image: str, timeout: float = 7.0) -> tuple[bool, str]:
    """HEAD an exact image manifest, including the Registry v2 bearer handshake.

    No large image layers are downloaded. A 403/404 is an unusable mirror.
    """
    rest = image.split("/", 1)[1]
    name, tag = rest.rsplit(":", 1)
    repo = f"public.ecr.aws/{name}"
    url = f"https://{mirror}/v2/{repo}/manifests/{tag}"
    headers = {"Accept": ACCEPT_MANIFEST, "User-Agent": "PIGS-DeepSWE-prefetch/1.0"}

    def head(with_token: str | None = None):
        h = dict(headers)
        if with_token:
            h["Authorization"] = f"Bearer {with_token}"
        return request.urlopen(request.Request(url, headers=h, method="HEAD"), timeout=timeout)

    try:
        with head() as resp:
            return resp.status == 200, f"HTTP {resp.status}"
    except error.HTTPError as exc:
        if exc.code != 401:
            return False, f"HTTP {exc.code}"
        challenge = exc.headers.get("WWW-Authenticate", "")
        fields = dict(re.findall(r'(\w+)="([^"]*)"', challenge))
        realm = fields.get("realm", "")
        if not challenge.lower().startswith("bearer ") or not realm.startswith("https://"):
            return False, "Unsupported registry authentication challenge"
        params = {k: fields[k] for k in ("service", "scope") if k in fields}
        token_url = realm + ("&" if "?" in realm else "?") + parse.urlencode(params)
        try:
            with request.urlopen(token_url, timeout=timeout) as auth:
                auth_json = json.load(auth)
                token = auth_json.get("token") or auth_json.get("access_token")
            if not token:
                return False, "Registry issued no bearer token"
            with head(token) as resp:
                return resp.status == 200, f"HTTP {resp.status}"
        except error.HTTPError as auth_error:
            return False, f"HTTP {auth_error.code} (authenticated)"
        except (OSError, ValueError) as auth_error:
            return False, f"Auth error: {str(auth_error)[:85]}"
    except (OSError, ValueError) as exc:
        return False, f"Network unavailable: {str(exc)[:85]}"


class DownloadState:
    def __init__(self, total: int, log_root: Path, d_drive: str):
        self.total = total
        self.started = time.monotonic()
        self.lock = threading.Lock()
        self.active: dict[str, str] = {}
        self.results: dict[str, dict] = {}
        self.log_root = log_root
        self.d_drive = d_drive
        self.stopping = False

    def update(self, task: str, line: str) -> None:
        with self.lock:
            self.active[task] = line[-95:]

    def finished(self, task: str, outcome: dict) -> None:
        with self.lock:
            self.active.pop(task, None)
            self.results[task] = outcome
            # Atomic status file write, useful after an interrupted run.
            state_file = self.log_root / "status.json"
            tmp = state_file.with_suffix(".tmp")
            tmp.write_text(json.dumps(self.results, ensure_ascii=False, indent=2), encoding="utf-8")
            tmp.replace(state_file)

    def snapshot(self) -> tuple[dict[str, str], dict[str, dict]]:
        with self.lock:
            return dict(self.active), dict(self.results)

    def show(self, force: bool = False) -> None:
        active, results = self.snapshot()
        done = sum(r["result"] in ("downloaded", "cached") for r in results.values())
        failed = sum(r["result"] == "failed" for r in results.values())
        remaining = self.total - len(results)
        try:
            available = gib(shutil.disk_usage(self.d_drive).free)
        except OSError:
            available = "unknown"
        stamp(f"TOTAL {bar(len(results), self.total)} | OK {done} | FAIL {failed} | "
              f"PENDING {remaining} | ACTIVE {len(active)} | D:free {available} | "
              f"ELAPSED {format_time(time.monotonic() - self.started)}")
        for name, line in list(active.items())[:8]:
            print(f"    {name[:34]:34}  {line}", flush=True)


def docker_pull_stream(ref: str, state: DownloadState, task: str,
                       log, timeout: int) -> tuple[bool, str]:
    args = wsl_docker("pull", ref)
    lines: queue.Queue[str | None] = queue.Queue()
    recent: deque[str] = deque(maxlen=16)
    try:
        proc = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    except OSError as exc:
        return False, str(exc)

    def reader():
        assert proc.stdout
        # Bytes may contain Docker's \r-overwritten progress; split both ways.
        pending = b""
        try:
            while True:
                chunk = proc.stdout.read1(4096)
                if not chunk:
                    break
                pending += chunk
                parts = re.split(rb"[\r\n]", pending)
                pending = parts.pop()
                for part in parts:
                    if part.strip():
                        lines.put(part.decode("utf-8", "replace"))
            if pending.strip():
                lines.put(pending.decode("utf-8", "replace"))
        finally:
            lines.put(None)

    thread = threading.Thread(target=reader, daemon=True)
    thread.start()
    deadline = time.monotonic() + timeout
    finished = False
    while not finished:
        if time.monotonic() > deadline or state.stopping:
            proc.kill()
            proc.wait(timeout=15)
            return False, "Cancelled or timed out"
        try:
            line = lines.get(timeout=1)
        except queue.Empty:
            continue
        if line is None:
            finished = True
        else:
            log.write(line + "\n")
            log.flush()
            recent.append(line)
            state.update(task, line.strip())
    rc = proc.wait(timeout=15)
    return rc == 0, " | ".join(recent)[-550:]


def download_one(item: TaskImage, mirrors: list[str], options,
                 state: DownloadState, cached: set[str]) -> dict:
    if item.image in cached:
        outcome = {"image": item.image, "result": "cached", "source": "local"}
        state.finished(item.task, outcome)
        return outcome
    if shutil.disk_usage(options.disk).free < options.min_free_gib * 1024**3:
        outcome = {"image": item.image, "result": "failed", "error": "Windows D: free space below limit"}
        state.finished(item.task, outcome)
        return outcome

    logpath = state.log_root / f"{item.task}.log"
    candidates = [mirror_ref(m, item.image) for m in mirrors]
    if not options.no_direct_fallback:
        candidates.append(item.image)
    last_error = "No usable source"
    with logpath.open("a", encoding="utf-8") as log:
        for ref in candidates:
            for attempt in range(1, options.retries + 1):
                label = "domestic" if ref != item.image else "official"
                state.update(item.task, f"{label} attempt {attempt}/{options.retries} ...")
                log.write(f"\n=== {datetime.now().isoformat()} | {ref} | try {attempt} ===\n")
                try:
                    ok, detail = docker_pull_stream(ref, state, item.task, log, options.timeout)
                except Exception as exc:
                    ok, detail = False, f"{type(exc).__name__}: {exc}"
                if ok:
                    try:
                        if ref != item.image:
                            quick_command(wsl_docker("tag", ref, item.image))
                            # Deleting extra mirror TAG does not remove shared layers.
                            quick_command(wsl_docker("image", "rm", ref))
                        # Docker validates layer digests while pulling.
                        quick_command(wsl_docker("image", "inspect", item.image), timeout=45)
                    except Exception as exc:
                        ok, detail = False, f"Tag/inspect failed: {exc}"
                if ok:
                    outcome = {"image": item.image, "result": "downloaded", "source": ref}
                    state.finished(item.task, outcome)
                    return outcome
                last_error = detail
                log.write(f"ERROR: {detail}\n")
                state.update(item.task, f"ERROR {ref[:20]}: {detail[-60:]}")
                time.sleep(min(2 * attempt, 8))
    outcome = {"image": item.image, "result": "failed", "error": last_error}
    state.finished(item.task, outcome)
    return outcome


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--tasks", type=Path, default=DEFAULT_TASKS, help="official DeepSWE tasks directory")
    ap.add_argument("--workers", type=int, default=4, help="parallel Docker pulls (default 4)")
    ap.add_argument("--retries", type=int, default=2, help="attempts per source (default 2)")
    ap.add_argument("--timeout", type=int, default=2700, help="seconds per image pull")
    ap.add_argument("--limit", type=int, default=0, help="first N tasks for smoke test (0 = all)")
    ap.add_argument("--task", action="append", default=[], help="select task name; repeatable")
    ap.add_argument("--mirror", action="append", default=[], help="replace default CN mirrors; repeatable")
    ap.add_argument("--direct-only", action="store_true", help="only use original ECR registry")
    ap.add_argument("--no-direct-fallback", action="store_true", help="never fall back to original ECR")
    ap.add_argument("--skip-probe", action="store_true", help="try mirror even when network probe would fail")
    ap.add_argument("--probe-only", action="store_true", help="only check mirror metadata accessibility")
    ap.add_argument("--dry-run", action="store_true", help="list tasks/images; no network or Docker actions")
    ap.add_argument("--min-free-gib", type=float, default=40.0, help="stop starting pulls if Windows D: is too full")
    ap.add_argument("--disk", default="D:\\", help="disk whose Windows free space must be protected")
    ap.add_argument("--logs", type=Path, default=DEFAULT_LOG_ROOT, help="progress/error log folder")
    opts = ap.parse_args()
    if os.name != "nt":
        ap.error("Run with native Windows Python (WSL Docker is accessed via wsl.exe)")
    if not 1 <= opts.workers <= 16 or not 1 <= opts.retries <= 6:
        ap.error("--workers must be 1..16 and --retries 1..6")
    if opts.limit < 0 or opts.timeout < 30 or opts.min_free_gib < 0:
        ap.error("invalid limit/timeout/min-free-gib")

    tasks = discover_tasks(opts.tasks)
    if opts.task:
        selected = set(opts.task)
        available = {item.task for item in tasks}
        unknown = selected - available
        if unknown:
            ap.error(f"unknown tasks: {', '.join(sorted(unknown))}")
        tasks = [item for item in tasks if item.task in selected]
    if opts.limit:
        tasks = tasks[:opts.limit]
    mirrors = [] if opts.direct_only else [normalize_mirror(x) for x in (opts.mirror or list(DEFAULT_MIRRORS))]
    stamp(f"DeepSWE official tasks: {len(tasks)} | parallel workers: {opts.workers}")
    stamp(f"Docker: Ubuntu-24.04 WSL | Images: official public.ecr.aws tags | Storage: D: WSL VHDX")
    if opts.dry_run:
        for index, item in enumerate(tasks, 1):
            print(f"{index:3d} {item.task:45} {item.image}")
        stamp(f"DRY RUN completed: {len(tasks)} images; nothing was downloaded")
        return 0

    if mirrors and not opts.skip_probe:
        confirmed = []
        for m in mirrors:
            stamp(f"Probe domestic registry: {m} (manifest HEAD, no layer download)")
            success, reason = registry_probe(m, tasks[0].image)
            stamp(f"  {m}: {'USABLE' if success else 'UNAVAILABLE'} ({reason})")
            if success:
                confirmed.append(m)
        mirrors = confirmed
    if not mirrors and not opts.direct_only:
        stamp("WARNING: no usable CN ECR mirror detected. Official ECR is the fallback.")
    if opts.probe_only:
        return 0 if mirrors else 2
    if not mirrors and opts.no_direct_fallback:
        stamp("No usable registry and direct fallback disabled.")
        return 2

    try:
        quick_command(wsl_docker("info", "--format", "{{.DockerRootDir}}"), timeout=60)
        found = quick_command(wsl_docker("image", "ls", "--format", "{{.Repository}}:{{.Tag}}"), timeout=60)
    except Exception as exc:
        stamp(f"Cannot access Ubuntu Docker daemon: {exc}")
        return 3
    cached = set(found.splitlines())
    stamp(f"Already cached official refs: {sum(item.image in cached for item in tasks)}/{len(tasks)}")
    stamp(f"Windows D: free: {gib(shutil.disk_usage(opts.disk).free)} | threshold: {opts.min_free_gib:.1f} GiB")
    opts.logs.mkdir(parents=True, exist_ok=True)
    logfile = opts.logs / datetime.now().strftime("%Y%m%d-%H%M%S")
    logfile.mkdir(parents=True, exist_ok=True)
    state = DownloadState(len(tasks), logfile, opts.disk)
    stamp(f"Per-image logs and status.json: {logfile}")
    stamp("Progress percentage measures COMPLETED TASK IMAGES; per-task lines show real Docker layer progress.")

    with ThreadPoolExecutor(max_workers=opts.workers) as pool:
        futures = [pool.submit(download_one, item, mirrors, opts, state, cached) for item in tasks]
        try:
            while any(not f.done() for f in futures):
                state.show()
                time.sleep(5)
        except KeyboardInterrupt:
            stamp("CTRL+C received. Cancelling in-flight Docker pull subprocesses.")
            state.stopping = True
        for f in as_completed(futures):
            try:
                f.result()
            except Exception as exc:
                stamp(f"Unexpected worker error: {exc}")
    state.show(force=True)
    _, results = state.snapshot()
    unsuccessful = [key for key, value in results.items() if value["result"] == "failed"]
    if unsuccessful:
        stamp(f"FAILED {len(unsuccessful)} image(s); rerun the SAME command to skip cached images")
        return 1
    stamp(f"SUCCESS: all {len(tasks)} task images available under official ECR names")
    stamp("Next: install Pier and run a single official task; downloading alone is not a graded run.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as exc:
        stamp(f"ERROR: {type(exc).__name__}: {exc}")
        sys.exit(1)
