#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path

from eval_common import EVAL_ROOT, add_common_args, count_evalscope, finalize
from eval_common import maybe_background, prepare, run_job, run_pipelines, tcp_preflight, validate

DEFAULT_EVALSCOPE = EVAL_ROOT / 'venv/bin/evalscope'


def run_dataset(dataset: str, evalscope_name: str, full_size: int, display_name: str) -> int:
    parser = argparse.ArgumentParser(description=f'Run {display_name} Base/PIGS evaluation.')
    add_common_args(parser, dataset)
    parser.add_argument('--evalscope', type=Path, default=DEFAULT_EVALSCOPE)
    parser.add_argument('--seed', type=int, default=20261001)
    parser.add_argument('--temperature', type=float, default=0.0)
    parser.add_argument('--limit', type=int, default=None, help='Development only; omit for formal runs.')
    args = parser.parse_args()
    models = validate(args)
    if args.limit is not None and args.limit <= 0:
        raise SystemExit('--limit must be > 0')
    if maybe_background(args):
        return 0
    if not args.evalscope.exists():
        raise SystemExit(f'EvalScope not found: {args.evalscope}')
    if not args.skip_preflight and not args.dry_run:
        tcp_preflight(args.base_url)

    expected = min(args.limit, full_size) if args.limit else full_size
    run_id, root, jobs, store = prepare(args, dataset, expected, models, {
        'harness': 'EvalScope',
        'evalscope': str(args.evalscope),
        'evalscope_dataset': evalscope_name,
        'official_full_size': full_size,
        'limit': args.limit,
        'seed': args.seed,
        'temperature': args.temperature,
    })
    print(f'run_dir={root}')
    print(f'progress_file={root / "progress.txt"}')
    if args.dry_run:
        return 0

    def run_one(job):
        generation = {
            'temperature': args.temperature,
            'reasoning_effort': args.thinking_effort,
            'retries': 0,
            'timeout': 600,
            'stream': True,
            'extra_headers': {'x-opencode-session': job.session_id},
        }
        command = [
            str(args.evalscope), 'eval',
            '--model', job.model,
            '--model-args', json.dumps({'max_retries': 0}, separators=(',', ':')),
            '--api-url', args.base_url,
            '--api-key', args.api_key,
            '--eval-type', 'openai_api',
            '--datasets', evalscope_name,
            '--eval-batch-size', str(args.sample_workers),
            '--generation-config', json.dumps(generation, separators=(',', ':')),
            '--seed', str(args.seed),
            '--ignore-errors',
            '--work-dir', job.output_dir,
        ]
        if args.limit:
            command += ['--limit', str(args.limit)]
        return run_job(
            job, command, store, count_evalscope, args.progress_interval,
            secrets=[args.api_key], pigs_log_source=args.pigs_log_dir,
        )

    failed = run_pipelines(models, args.arms, jobs, args.model_workers, run_one)
    finalize(root, jobs)
    try:
        from summarize_evaluation import summarize_run
        summarize_run(root, args.pigs_log_dir)
    except Exception as exc:
        (root / 'summary_error.txt').write_text(repr(exc) + '\n', encoding='utf-8')
    return 1 if failed else 0
