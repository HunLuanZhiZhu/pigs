#!/usr/bin/env python3
from __future__ import annotations

import argparse, json, os, shutil, socket, subprocess, sys, threading, time, uuid
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Iterable
from urllib.parse import urlparse

EVAL_ROOT = Path('/root/pigs-eval')
REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT_ROOT = EVAL_ROOT / 'outputs/formal'
DEFAULT_PIGS_LOG_DIR = REPO_ROOT / 'logs/http'
DEFAULT_BASE_URL = 'http://127.0.0.1:3927'
DEFAULT_API_KEY = 'local-eval-placeholder'
DEFAULT_MODELS = ['mimo-v2.6-flash', 'deepseek-v4.1-flash']
DEFAULT_THINKING_EFFORT = 'low'
ARMS = ['base', 'pigs']

@dataclass(frozen=True)
class ModelSpec:
    label: str
    base_model: str
    def wire(self, arm: str) -> str:
        return self.base_model if arm == 'base' else f'{self.base_model}-pigs'

@dataclass
class Job:
    job_id: str; dataset: str; model_label: str; arm: str; model: str
    expected_samples: int; output_dir: str; session_id: str
    status: str = 'queued'; completed_samples: int = 0
    exit_code: int | None = None; started_at: str | None = None; finished_at: str | None = None
    command: list[str] | None = None
    thinking_effort: str | None = None

def now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec='seconds')

def atomic_write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + '.tmp'); tmp.write_text(text, encoding='utf-8'); tmp.replace(path)

def write_json(path: Path, obj: object) -> None:
    atomic_write(path, json.dumps(obj, ensure_ascii=False, indent=2) + '\n')

def git_commit(path: Path) -> str | None:
    try:
        return subprocess.check_output(['git','rev-parse','HEAD'], cwd=path, text=True, stderr=subprocess.DEVNULL).strip()
    except Exception: return None

def slug(s: str) -> str:
    out=''.join(c if c.isalnum() or c in '-_' else '-' for c in s.lower()).strip('-')
    while '--' in out: out=out.replace('--','-')
    return out or 'model'

def parse_models(values: Iterable[str]) -> list[ModelSpec]:
    out=[]; labels=set()
    for raw in values:
        label, model = (raw.split('=',1) if '=' in raw else (raw,raw))
        label, model = slug(label.strip()), model.strip()
        if not model or model.endswith('-pigs'): raise SystemExit(f'invalid base model: {raw}')
        if label in labels: raise SystemExit(f'duplicate model label: {label}')
        labels.add(label); out.append(ModelSpec(label,model))
    if not out: raise SystemExit('at least one model is required')
    return out

def tcp_preflight(url: str, timeout: float=3.0) -> None:
    u=urlparse(url); port=u.port or (443 if u.scheme=='https' else 80)
    if not u.hostname: raise RuntimeError(f'invalid URL: {url}')
    with socket.create_connection((u.hostname,port), timeout=timeout): pass

def bar(done:int,total:int,width:int=24)->str:
    ratio=min(max(done/total if total else 0,0),1); n=int(ratio*width)
    return '['+'#'*n+'-'*(width-n)+']'

class ProgressStore:
    def __init__(self, root:Path, run_id:str, dataset:str, jobs:list[Job]):
        self.root=root; self.run_id=run_id; self.dataset=dataset; self.jobs=jobs; self.lock=threading.Lock()
    def write(self)->None:
        with self.lock:
            updated=now(); total=sum(j.expected_samples for j in self.jobs); done=sum(min(j.completed_samples,j.expected_samples) for j in self.jobs)
            write_json(self.root/'progress.json', {'schema_version':2,'run_id':self.run_id,'dataset':self.dataset,'updated_at':updated,'total_expected_samples':total,'total_completed_samples':done,'jobs':[asdict(j) for j in self.jobs]})
            lines=[f'PIGS evaluation: {self.dataset}',f'run: {self.run_id}',f'updated: {updated}','',f'Overall {bar(done,total)} {done}/{total} {(100*done/total if total else 0):6.2f}%','']
            for j in self.jobs:
                n=min(j.completed_samples,j.expected_samples); p=100*n/j.expected_samples if j.expected_samples else 0
                lines.append(f'{j.model_label:24s} {j.arm:4s} {bar(n,j.expected_samples)} {n:5d}/{j.expected_samples:<5d} {p:6.2f}% {j.status.upper()}')
            atomic_write(self.root/'progress.txt','\n'.join(lines)+'\n')

def add_common_args(p:argparse.ArgumentParser, dataset:str)->None:
    p.add_argument('--models',nargs='+',default=list(DEFAULT_MODELS),metavar='[LABEL=]MODEL')
    p.add_argument('--arms',nargs='+',choices=ARMS,default=list(ARMS))
    p.add_argument('--model-workers',type=int,default=1,help='Concurrent model pipelines; 0 = all models. Base then PIGS stay sequential per model.')
    p.add_argument('--sample-workers',type=int,default=1)
    p.add_argument('--base-url',default=DEFAULT_BASE_URL); p.add_argument('--api-key',default=os.getenv('PIGS_EVAL_API_KEY',DEFAULT_API_KEY))
    p.add_argument('--thinking-effort',default=DEFAULT_THINKING_EFFORT,help='Explicit reasoning_effort sent by the benchmark harness (default: low).')
    p.add_argument('--output-root',type=Path,default=DEFAULT_OUTPUT_ROOT/dataset); p.add_argument('--pigs-log-dir',type=Path,default=DEFAULT_PIGS_LOG_DIR)
    p.add_argument('--run-id'); p.add_argument('--progress-interval',type=float,default=2.0); p.add_argument('--skip-preflight',action='store_true'); p.add_argument('--dry-run',action='store_true'); p.add_argument('--background',action='store_true')

def validate(args:argparse.Namespace)->list[ModelSpec]:
    if args.model_workers<0 or args.sample_workers<=0 or args.progress_interval<=0: raise SystemExit('workers/interval must be positive (model-workers may be 0)')
    if not str(args.thinking_effort).strip(): raise SystemExit('--thinking-effort must be non-empty')
    return parse_models(args.models)

def maybe_background(args:argparse.Namespace)->bool:
    if not args.background: return False
    run_id=args.run_id or datetime.now().strftime('%Y%m%d_%H%M%S'); launch=Path(args.output_root)/'_launch'; launch.mkdir(parents=True,exist_ok=True)
    argv=[x for x in sys.argv[1:] if x!='--background']
    if not any(x == '--run-id' or x.startswith('--run-id=') for x in argv): argv += ['--run-id',run_id]
    log=launch/f'{run_id}.log'; pid=launch/f'{run_id}.pid'
    with log.open('ab',buffering=0) as fh:
        proc=subprocess.Popen([sys.executable,str(Path(sys.argv[0]).resolve()),*argv],stdout=fh,stderr=subprocess.STDOUT,stdin=subprocess.DEVNULL,start_new_session=True,close_fds=True)
    pid.write_text(str(proc.pid)+'\n'); root=Path(args.output_root)/run_id
    print(f'background_pid={proc.pid}'); print(f'run_dir={root}'); print(f'progress_file={root/"progress.txt"}'); print(f'launcher_log={log}')
    return True

def prepare(args:argparse.Namespace,dataset:str,expected:int,models:list[ModelSpec],extra:dict|None=None):
    run_id=args.run_id or datetime.now().strftime('%Y%m%d_%H%M%S'); root=Path(args.output_root)/run_id
    if root.exists() and any(root.iterdir()): raise SystemExit(f'run directory not empty: {root}')
    root.mkdir(parents=True,exist_ok=True); jobs=[]
    effort=str(args.thinking_effort).strip()
    for m in models:
        for arm in args.arms:
            jobs.append(Job(
                f'{m.label}-{arm}', dataset, m.label, arm, m.wire(arm), expected,
                str(root/m.label/arm),
                f'pigs-eval-{run_id}-{dataset}-{m.label}-{arm}-{uuid.uuid4().hex[:8]}',
                thinking_effort=effort,
            ))
    manifest={
        'schema_version':2,'run_id':run_id,'created_at':now(),'dataset':dataset,
        'expected_samples_per_arm':expected,'repo_commit':git_commit(REPO_ROOT),
        'run_root':str(root),'base_url':args.base_url,
        'models':[asdict(m)|{'thinking_effort':effort} for m in models],
        'thinking_effort':effort,'arms':args.arms,'model_workers':args.model_workers,
        'sample_workers':args.sample_workers,
        'output_token_limit_policy':'not set by evaluation runner; provider/model native limit applies',
        'pigs_log_dir':str(args.pigs_log_dir),'jobs':[asdict(j) for j in jobs],
    }
    if extra: manifest.update(extra)
    write_json(root/'run_manifest.json',manifest)
    for model in models:
        model_jobs=[j for j in jobs if j.model_label==model.label]
        write_json(root/model.label/'model_manifest.json', {
            'schema_version':1,'run_id':run_id,'dataset':dataset,
            'model_label':model.label,'base_model':model.base_model,
            'thinking_effort':effort,'jobs':[asdict(j) for j in model_jobs],
        })
    store=ProgressStore(root,run_id,dataset,jobs); store.write(); return run_id,root,jobs,store

def redact(cmd:list[str], secrets:Iterable[str])->list[str]:
    s={x for x in secrets if x}; return ['[REDACTED]' if x in s else x for x in cmd]

def archive_pigs_session_logs(source:Path, session_id:str, dest:Path)->int:
    dest.mkdir(parents=True,exist_ok=True)
    if not source.exists():
        write_json(dest/'archive.json',{'source':str(source),'session_id':session_id,'matched_exchanges':0,'files':0})
        return 0
    paths=[p for p in source.glob('*.txt') if p.is_file()]
    exchange_ids=set()
    for path in paths:
        try:
            if session_id in path.read_text(encoding='utf-8',errors='replace'):
                exchange_ids.add(path.name.split('.',1)[0])
        except OSError:
            pass
    copied=0
    for path in paths:
        if path.name.split('.',1)[0] not in exchange_ids:
            continue
        try:
            shutil.copy2(path,dest/path.name); copied+=1
        except OSError:
            pass
    write_json(dest/'archive.json',{'source':str(source),'session_id':session_id,'matched_exchanges':len(exchange_ids),'files':copied,'archived_at':now()})
    return copied

def run_job(job:Job, cmd:list[str], store:ProgressStore, counter:Callable[[Path],int], interval:float, env:dict[str,str]|None=None, cwd:Path|None=None, secrets:Iterable[str]=(), pigs_log_source:Path|None=None)->int:
    out=Path(job.output_dir); out.mkdir(parents=True,exist_ok=True)
    job.command=redact(cmd,secrets); job.status='running'; job.started_at=now(); store.write()
    e=os.environ.copy(); e.update(env or {}); e['PIGS_SESSION_ID']=job.session_id
    with (out/'runner.log').open('a',encoding='utf-8') as log:
        log.write(
            f'START {job.started_at}\n'
            f'MODEL {job.model}\n'
            f'THINKING_EFFORT {job.thinking_effort}\n'
            f'COMMAND {json.dumps(job.command,ensure_ascii=False)}\n'
        ); log.flush()
        proc=subprocess.Popen(cmd,stdout=log,stderr=subprocess.STDOUT,env=e,cwd=str(cwd) if cwd else None,text=True)
        while proc.poll() is None:
            job.completed_samples=min(counter(out),job.expected_samples); store.write(); time.sleep(interval)
        code=int(proc.returncode or 0); seen=min(counter(out),job.expected_samples)
        job.completed_samples=job.expected_samples if code==0 else seen
        job.exit_code=code; job.finished_at=now(); job.status='done' if code==0 else 'failed'
        log.write(f'END {job.finished_at} exit_code={code}\n')
    if pigs_log_source is not None:
        archive_pigs_session_logs(Path(pigs_log_source),job.session_id,out/'pigs-http')
    store.write(); return code

def run_pipelines(models:list[ModelSpec], arms:list[str], jobs:list[Job], workers:int, run_one:Callable[[Job],int])->bool:
    idx={(j.model_label,j.arm):j for j in jobs}
    n=len(models) if workers==0 else max(1,min(workers,len(models))); failed=threading.Event()
    def pipeline(m:ModelSpec):
        for arm in arms:
            j=idx[(m.label,arm)]
            try:
                if run_one(j)!=0: failed.set()
            except Exception:
                j.status='failed'; j.finished_at=now(); failed.set(); raise
    with ThreadPoolExecutor(max_workers=n) as pool:
        futures={pool.submit(pipeline,m):m.label for m in models}
        for f in as_completed(futures):
            try: f.result()
            except Exception as exc: print(f'model pipeline failed: {futures[f]}: {exc}',file=sys.stderr)
    return failed.is_set()

def finalize(root:Path,jobs:list[Job])->None:
    p=root/'run_manifest.json'; m=json.loads(p.read_text())
    m['jobs']=[asdict(j) for j in jobs]; m['finished_at']=now(); write_json(p,m)

def count_evalscope(root:Path)->int:
    n=0
    for p in root.rglob('*.jsonl') if root.exists() else []:
        if 'predictions' not in p.parts: continue
        try: n+=sum(1 for line in p.open(encoding='utf-8',errors='replace') if line.strip())
        except OSError: pass
    return n

def count_bfcl(root:Path)->int:
    n=0; rr=root/'results'
    for p in rr.rglob('*.json') if rr.exists() else []:
        try: n+=sum(1 for line in p.open(encoding='utf-8',errors='replace') if line.lstrip().startswith('{') and '"id"' in line)
        except OSError: pass
    return n

def count_swe(root:Path)->int:
    p=root/'inference'/'preds.json'
    try:
        x=json.loads(p.read_text()); return len(x) if isinstance(x,dict) else 0
    except (OSError,json.JSONDecodeError): return 0
