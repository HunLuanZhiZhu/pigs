#!/usr/bin/env python3
"""Run mini-SWE-agent on full SWE-bench Lite test, then official Docker scoring."""
from __future__ import annotations
import argparse, os, subprocess
from pathlib import Path


def main()->int:
    p=argparse.ArgumentParser()
    p.add_argument('--wire-model',required=True); p.add_argument('--base-url',required=True)
    p.add_argument('--api-key',required=True); p.add_argument('--session-id',required=True)
    p.add_argument('--output-dir',type=Path,required=True); p.add_argument('--workers',type=int,default=1)
    p.add_argument('--mini-extra',type=Path,required=True); p.add_argument('--swebench',type=Path,required=True)
    p.add_argument('--official-config',type=Path,required=True); p.add_argument('--score-run-id',required=True)
    p.add_argument('--thinking-effort',default='low')
    args=p.parse_args(); args.output_dir.mkdir(parents=True,exist_ok=True)
    inference=args.output_dir/'inference'; inference.mkdir(parents=True,exist_ok=True)
    override=args.output_dir/'model_override.yaml'
    override.write_text(
        'model:\n'
        f'  model_name: "openai/{args.wire_model}"\n'
        '  model_kwargs:\n'
        f'    api_base: "{args.base_url}"\n'
        '    drop_params: true\n'
        '    parallel_tool_calls: true\n'
        '    extra_headers:\n'
        f'      x-opencode-session: "{args.session_id}"\n', encoding='utf-8')
    env=os.environ.copy(); env['OPENAI_API_KEY']=args.api_key; env['PIGS_SESSION_ID']=args.session_id
    infer_cmd=[str(args.mini_extra),'swebench','--subset','lite','--split','test','--output',str(inference),
               '--workers',str(args.workers),'--model',f'openai/{args.wire_model}',
               '--config',str(args.official_config),'--config',str(override)]
    code=subprocess.call(infer_cmd,env=env)
    if code!=0: return code
    preds=inference/'preds.json'
    if not preds.exists(): raise SystemExit(f'missing predictions: {preds}')
    score_dir=args.output_dir/'scoring'; score_dir.mkdir(parents=True,exist_ok=True)
    score_cmd=[str(args.swebench),'eval','lite','--predictions',str(preds),'--run-id',args.score_run_id,
               '--workers',str(args.workers)]
    # Current CLI also supports -p/-j; use long options for manifest readability.
    return subprocess.call(score_cmd,env=env,cwd=score_dir)

if __name__=='__main__': raise SystemExit(main())
