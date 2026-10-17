#!/usr/bin/env python3
from __future__ import annotations
import argparse
import json
from collections import defaultdict
from pathlib import Path

try:
    from bfcl_official_adapter import request_fingerprint
except ModuleNotFoundError:
    from evaluation.bfcl_official_adapter import request_fingerprint

from eval_common import EVAL_ROOT, add_common_args, count_bfcl, finalize, git_commit
from eval_common import maybe_background, prepare, run_job, run_pipelines, slug, tcp_preflight, validate

DATASET='bfcl_multiturn'; EXPECTED=800
BFCL_ROOT=EVAL_ROOT/'src/gorilla/berkeley-function-call-leaderboard'
BFCL_PYTHON=EVAL_ROOT/'venv/bin/python'
ADAPTER=Path(__file__).with_name('bfcl_official_adapter.py')


def _client_request_body(path:Path)->dict|None:
    try:
        text=path.read_text(encoding='utf-8',errors='replace')
    except OSError:
        return None
    marker='\nbody:\n'
    if marker not in text:
        return None
    try:
        body=json.loads(text.split(marker,1)[1].strip())
    except json.JSONDecodeError:
        return None
    return body if isinstance(body,dict) else None


def attach_bfcl_metadata(request_map:Path,pigs_http:Path)->dict:
    """Join local BFCL identities to archived PIGS exchanges without changing HTTP."""
    records=[]
    if request_map.exists():
        for line in request_map.open(encoding='utf-8',errors='replace'):
            if not line.strip(): continue
            try: row=json.loads(line)
            except json.JSONDecodeError: continue
            if isinstance(row,dict) and row.get('fingerprint'): records.append(row)

    captured=[]
    for path in pigs_http.glob('*.client-request.txt') if pigs_http.exists() else []:
        body=_client_request_body(path)
        if body is None: continue
        exchange=path.name.split('.',1)[0]
        try: received_at_ms=int(exchange.split('-',1)[0])
        except ValueError: continue
        captured.append({
            'exchange':exchange,
            'received_at_ms':received_at_ms,
            'fingerprint':request_fingerprint(body),
        })

    record_groups=defaultdict(list); capture_groups=defaultdict(list)
    for row in records: record_groups[row['fingerprint']].append(row)
    for row in captured: capture_groups[row['fingerprint']].append(row)

    matched=[]; unmatched_records=0; unmatched_exchanges=0
    for fingerprint in set(record_groups)|set(capture_groups):
        rs=sorted(record_groups.get(fingerprint,[]),key=lambda x:int(x.get('sent_at_ns',0)))
        cs=sorted(capture_groups.get(fingerprint,[]),key=lambda x:int(x['received_at_ms']))
        pair_count=min(len(rs),len(cs))
        for i in range(pair_count):
            r,c=rs[i],cs[i]
            matched.append({
                'exchange':c['exchange'],
                'sample_id':r['sample_id'],
                'turn_id':int(r['turn_id']),
                'request_index':int(r['request_index']),
                'fingerprint':fingerprint,
            })
        unmatched_records+=len(rs)-pair_count
        unmatched_exchanges+=len(cs)-pair_count

    matched.sort(key=lambda x:x['exchange'])
    if pigs_http.exists():
        with (pigs_http/'bfcl-metadata.jsonl').open('w',encoding='utf-8') as f:
            for row in matched:
                f.write(json.dumps(row,ensure_ascii=False,separators=(',',':'))+'\n')
        for row in matched:
            (pigs_http/f"{row['exchange']}.bfcl-metadata.txt").write_text(
                f"exchange: {row['exchange']}\n"
                f"sample_id: {row['sample_id']}\n"
                f"turn_id: {row['turn_id']}\n"
                f"request_index: {row['request_index']}\n",
                encoding='utf-8',
            )
        summary={
            'adapter_requests':len(records),'captured_client_requests':len(captured),
            'matched':len(matched),'unmatched_adapter_requests':unmatched_records,
            'unmatched_captured_requests':unmatched_exchanges,
        }
        (pigs_http/'bfcl-metadata-summary.json').write_text(
            json.dumps(summary,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
        return summary
    return {'adapter_requests':len(records),'captured_client_requests':0,'matched':0,
            'unmatched_adapter_requests':len(records),'unmatched_captured_requests':0}


def main()->int:
    p=argparse.ArgumentParser(description='Run official BFCL V4 Multi-Turn (800 tasks).')
    add_common_args(p,DATASET)
    p.add_argument('--bfcl-root',type=Path,default=BFCL_ROOT)
    p.add_argument('--bfcl-python',type=Path,default=BFCL_PYTHON)
    p.add_argument('--temperature',type=float,default=0.001)
    p.add_argument('--protocol',choices=['chat','responses'],default='chat',
                   help='BFCL official OpenAI Chat or Responses handler (Muse uses responses).')
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
        'protocol':args.protocol,
    })
    print(f'run_dir={root}'); print(f'progress_file={root/"progress.txt"}')
    if args.dry_run: return 0

    def one(job):
        registry=slug(f'pigs-eval-{job.model_label}-{job.arm}')
        out=Path(job.output_dir)
        request_map=out/'bfcl-request-map.raw.jsonl'
        cmd=[str(args.bfcl_python),str(ADAPTER),'--registry-name',registry,'--wire-model',job.model,
             '--base-url',args.base_url,'--api-key',args.api_key,'--session-id',job.session_id,
             '--result-dir',str(out/'results'),'--score-dir',str(out/'scores'),
             '--request-map',str(request_map),
             '--threads',str(args.sample_workers),'--temperature',str(args.temperature),
             '--thinking-effort',args.thinking_effort,'--request-timeout',str(args.request_timeout),
             '--protocol',args.protocol]
        code=run_job(job,cmd,store,count_bfcl,args.progress_interval,cwd=args.bfcl_root,secrets=[args.api_key],pigs_log_source=args.pigs_log_dir)
        attach_bfcl_metadata(request_map,out/'pigs-http')
        return code

    failed=run_pipelines(models,args.arms,jobs,args.model_workers,one); finalize(root,jobs)
    try:
        from summarize_evaluation import summarize_run
        summarize_run(root,args.pigs_log_dir)
    except Exception as exc:
        (root/'summary_error.txt').write_text(repr(exc)+'\n',encoding='utf-8')
    return 1 if failed else 0

if __name__=='__main__': raise SystemExit(main())
