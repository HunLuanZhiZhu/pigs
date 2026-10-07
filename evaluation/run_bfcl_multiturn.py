#!/usr/bin/env python3
from __future__ import annotations
import argparse
from pathlib import Path

from eval_common import EVAL_ROOT, add_common_args, count_bfcl, finalize, git_commit
from eval_common import maybe_background, prepare, run_job, run_pipelines, slug, tcp_preflight, validate

DATASET='bfcl_multiturn'; EXPECTED=800
BFCL_ROOT=EVAL_ROOT/'src/gorilla/berkeley-function-call-leaderboard'
BFCL_PYTHON=EVAL_ROOT/'venv/bin/python'
ADAPTER=Path(__file__).with_name('bfcl_official_adapter.py')

def main()->int:
    p=argparse.ArgumentParser(description='Run official BFCL V4 Multi-Turn (800 tasks).')
    add_common_args(p,DATASET)
    p.add_argument('--bfcl-root',type=Path,default=BFCL_ROOT)
    p.add_argument('--bfcl-python',type=Path,default=BFCL_PYTHON)
    p.add_argument('--temperature',type=float,default=0.001)
    p.add_argument('--request-timeout',type=float,default=1800.0,help='Per OpenAI-compatible request timeout in seconds (default: 1800).')
    args=p.parse_args(); models=validate(args)
    if args.request_timeout<=0: raise SystemExit('--request-timeout must be positive')
    if maybe_background(args): return 0
    if not args.bfcl_python.exists(): raise SystemExit(f'BFCL Python not found: {args.bfcl_python}')
    if not args.skip_preflight and not args.dry_run: tcp_preflight(args.base_url)
    run_id,root,jobs,store=prepare(args,DATASET,EXPECTED,models,{
        'harness':'BFCL official V4 multi_turn','bfcl_root':str(args.bfcl_root),
        'bfcl_commit':git_commit(args.bfcl_root),'bfcl_category':'multi_turn',
        'bfcl_subsets':['multi_turn_base','multi_turn_miss_func','multi_turn_miss_param','multi_turn_long_context'],
        'temperature':args.temperature,'request_timeout_seconds':args.request_timeout,
    })
    print(f'run_dir={root}'); print(f'progress_file={root/"progress.txt"}')
    if args.dry_run: return 0

    def one(job):
        registry=slug(f'pigs-eval-{job.model_label}-{job.arm}')
        out=Path(job.output_dir)
        cmd=[str(args.bfcl_python),str(ADAPTER),'--registry-name',registry,'--wire-model',job.model,
             '--base-url',args.base_url,'--api-key',args.api_key,'--session-id',job.session_id,
             '--result-dir',str(out/'results'),'--score-dir',str(out/'scores'),
             '--threads',str(args.sample_workers),'--temperature',str(args.temperature),
             '--thinking-effort',args.thinking_effort,'--request-timeout',str(args.request_timeout)]
        return run_job(job,cmd,store,count_bfcl,args.progress_interval,cwd=args.bfcl_root,secrets=[args.api_key],pigs_log_source=args.pigs_log_dir)

    failed=run_pipelines(models,args.arms,jobs,args.model_workers,one); finalize(root,jobs)
    try:
        from summarize_evaluation import summarize_run
        summarize_run(root,args.pigs_log_dir)
    except Exception as exc:
        (root/'summary_error.txt').write_text(repr(exc)+'\n',encoding='utf-8')
    return 1 if failed else 0

if __name__=='__main__': raise SystemExit(main())
