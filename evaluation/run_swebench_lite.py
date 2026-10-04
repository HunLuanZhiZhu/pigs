#!/usr/bin/env python3
from __future__ import annotations
import argparse, subprocess
from pathlib import Path

from eval_common import EVAL_ROOT, add_common_args, count_swe, finalize, git_commit
from eval_common import maybe_background, prepare, run_job, run_pipelines, slug, tcp_preflight, validate

DATASET='swebench_lite'; EXPECTED=300
SWE_ROOT=EVAL_ROOT/'src/SWE-bench'; MINI_ROOT=EVAL_ROOT/'src/mini-swe-agent'
VENV=EVAL_ROOT/'swebench-venv/bin'; ADAPTER=Path(__file__).with_name('swebench_lite_adapter.py')

def main()->int:
    p=argparse.ArgumentParser(description='Run full official SWE-bench Lite test (300 tasks).')
    add_common_args(p,DATASET)
    p.add_argument('--python',type=Path,default=VENV/'python')
    p.add_argument('--mini-extra',type=Path,default=VENV/'mini-extra')
    p.add_argument('--swebench-cli',type=Path,default=VENV/'swebench')
    p.add_argument('--mini-root',type=Path,default=MINI_ROOT); p.add_argument('--swebench-root',type=Path,default=SWE_ROOT)
    args=p.parse_args(); models=validate(args)
    if maybe_background(args): return 0
    for path in (args.python,args.mini_extra,args.swebench_cli):
        if not path.exists(): raise SystemExit(f'missing SWE-bench executable: {path}')
    if not args.skip_preflight and not args.dry_run:
        tcp_preflight(args.base_url)
        if subprocess.call(['docker','info'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)!=0:
            raise SystemExit('Docker is not available; SWE-bench requires the official Docker environment.')
    official_config=args.mini_root/'src/minisweagent/config/benchmarks/swebench.yaml'
    run_id,root,jobs,store=prepare(args,DATASET,EXPECTED,models,{
        'harness':'mini-SWE-agent + official SWE-bench Docker evaluator','subset':'lite','split':'test',
        'swebench_root':str(args.swebench_root),'swebench_commit':git_commit(args.swebench_root),
        'mini_swe_agent_root':str(args.mini_root),'mini_swe_agent_commit':git_commit(args.mini_root),
        'mini_swe_agent_config':str(official_config),
    })
    print(f'run_dir={root}'); print(f'progress_file={root/"progress.txt"}')
    if args.dry_run: return 0

    def one(job):
        score_id=slug(f'{run_id}-{job.model_label}-{job.arm}')
        cmd=[str(args.python),str(ADAPTER),'--wire-model',job.model,'--base-url',args.base_url,
             '--api-key',args.api_key,'--session-id',job.session_id,'--output-dir',job.output_dir,
             '--workers',str(args.sample_workers),'--mini-extra',str(args.mini_extra),
             '--swebench',str(args.swebench_cli),'--official-config',str(official_config),
             '--score-run-id',score_id,'--thinking-effort',args.thinking_effort]
        return run_job(job,cmd,store,count_swe,args.progress_interval,secrets=[args.api_key],pigs_log_source=args.pigs_log_dir)

    failed=run_pipelines(models,args.arms,jobs,args.model_workers,one); finalize(root,jobs)
    try:
        from summarize_evaluation import summarize_run
        summarize_run(root,args.pigs_log_dir)
    except Exception as exc:
        (root/'summary_error.txt').write_text(repr(exc)+'\n',encoding='utf-8')
    return 1 if failed else 0

if __name__=='__main__': raise SystemExit(main())
