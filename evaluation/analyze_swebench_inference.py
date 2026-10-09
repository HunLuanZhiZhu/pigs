#!/usr/bin/env python3
"""SWE-bench inference-only statistics from mini-SWE-agent trajectories.

No correctness judgment: only official SWE-bench Docker scoring establishes resolved rate.
"""
from __future__ import annotations
import argparse, collections, json, statistics
from datetime import datetime, timezone
from pathlib import Path


def dist(values):
    x = sorted(values)
    if not x:
        return {"n":0, "mean":None,"median":None,"p90":None,"p95":None,"max":None}
    def pct(q):
        pos=(len(x)-1)*q
        low=int(pos)
        return round(x[low]+(x[min(low+1,len(x)-1)]-x[low])*(pos-low),2)
    return {"n":len(x),"mean":round(statistics.fmean(x),2),
            "median":round(statistics.median(x),2),"p90":pct(.90),"p95":pct(.95),"max":round(max(x),2)}


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("inference_dir",type=Path)
    parser.add_argument("--output",type=Path)
    args=parser.parse_args()
    d=args.inference_dir
    predictions=json.loads((d/"preds.json").read_text(encoding="utf-8"))
    traces=list(d.glob("*/*.traj.json"))
    totals=collections.Counter()
    statuses=collections.Counter()
    repo=collections.Counter()
    calls=[]
    tool_counts=[]
    lengths=[]
    times=[]
    cache_known=0
    errors=[]
    by_id=[]
    for path in traces:
        item=json.loads(path.read_text(encoding="utf-8"))
        iid=item.get("instance_id",path.parent.name)
        repo[iid.split("__",1)[0]]+=1
        info=item.get("info",{})
        status=info.get("exit_status") or "Unknown"
        statuses[status]+=1
        n_calls=info.get("model_stats",{}).get("api_calls",0)
        calls.append(n_calls)
        msgs=item.get("messages",[])
        ts=[]
        current=collections.Counter()
        tools=0
        cmd_fails=0
        for msg in msgs:
            extra=msg.get("extra",{})
            if not isinstance(extra,dict):
                continue
            t=extra.get("timestamp")
            if isinstance(t,(float,int)): ts.append(t)
            if msg.get("role") in ("tool","observation"):
                tools+=1
                if extra.get("returncode") not in (None,0):
                    cmd_fails+=1
                if extra.get("exception_info"):
                    current["tool_exceptions"]+=1
                    if "timed out" in str(extra["exception_info"]).lower():
                        current["tool_timeouts"]+=1
            if msg.get("role")!="assistant":
                continue
            resp=extra.get("response")
            if not isinstance(resp,dict): continue
            usage=resp.get("usage",{})
            if not isinstance(usage,dict):continue
            current["api_responses_with_usage"]+=1
            for key,upstream in [("input_tokens","prompt_tokens"),("output_tokens","completion_tokens"),("total_tokens","total_tokens")]:
                n=usage.get(upstream)
                if isinstance(n,int):current[key]+=n
            for key,detail,field in [("reasoning_tokens","completion_tokens_details","reasoning_tokens"),("cached_tokens","prompt_tokens_details","cached_tokens")]:
                obj=usage.get(detail)
                n=obj.get(field) if isinstance(obj,dict) else None
                if isinstance(n,int):current[key]+=n
            if isinstance(usage.get("prompt_tokens_details"),dict) and isinstance(usage["prompt_tokens_details"].get("cached_tokens"),int):
                current["cache_reports"]+=1
        totals.update(current)
        tool_counts.append(tools)
        sample_duration=round(max(ts)-min(ts),1) if ts else None
        if sample_duration is not None:times.append(sample_duration)
        patch=predictions.get(iid,{}).get("model_patch","")
        patchlen=len(patch)
        lengths.append(patchlen)
        by_id.append({"instance_id":iid,"status":status,"api_calls":n_calls,"tool_observations":tools,"tool_failures":cmd_fails,"duration_sec":sample_duration,"patch_chars":patchlen,
                      "input_tokens":current["input_tokens"],"output_tokens":current["output_tokens"],"total_tokens":current["total_tokens"],
                      "tool_timeouts":current["tool_timeouts"],"tool_exceptions":current["tool_exceptions"]})
        if len(ts)==0:errors.append(iid)
    submitted=set(x["instance_id"] for x in by_id)
    assert submitted==set(predictions),("missing_trajectories",sorted(set(predictions)-submitted),"other",sorted(submitted-set(predictions)))
    out={
        "generated_at":datetime.now(timezone.utc).isoformat(),
        "note":"Inference and usage statistics only. This is not a scored SWE-bench result. 299 original samples + 1 separately rerun.",
        "inference_dir":str(d),
        "prediction_count":len(predictions),"trajectory_count":len(traces),
        "nonempty_patches":sum(bool(x["model_patch"].strip()) for x in predictions.values()),
        "empty_patch_ids":sorted([k for k,x in predictions.items() if not x["model_patch"].strip()]),
        "exit_statuses":dict(statuses),"repos":dict(sorted(repo.items())),
        "api_calls":dist(calls),"tool_observations":dist(tool_counts),
        "sample_observed_duration_sec":dist(times),
        "patch_chars":dist(lengths),
        "sum_usage":{k:totals[k] for k in ("input_tokens","output_tokens","total_tokens","reasoning_tokens","cached_tokens","api_responses_with_usage","cache_reports","tool_timeouts","tool_exceptions")},
        "token_usage_explanation":"Provider report totals summed over stored assistant response objects, including cached tokens where supplied; model cost field is not usable.",
        "max_calls":sorted(by_id,key=lambda x:x["api_calls"],reverse=True)[:10],
        "max_tokens":sorted(by_id,key=lambda x:x["total_tokens"],reverse=True)[:10],
        "max_duration":sorted([x for x in by_id if x["duration_sec"] is not None],key=lambda x:x["duration_sec"],reverse=True)[:10],
        "samples_without_message_timestamps":errors,
        "sample_details":by_id,
    }
    output=args.output or d.parent/"scoring"/"inference_statistics.json"
    output.parent.mkdir(exist_ok=True,parents=True)
    output.write_text(json.dumps(out,ensure_ascii=False,indent=2)+"\n",encoding="utf-8")
    summary={k:out[k] for k in ["prediction_count","trajectory_count","nonempty_patches","empty_patch_ids","exit_statuses","repos","api_calls","tool_observations","sample_observed_duration_sec","patch_chars","sum_usage"]}
    print(json.dumps(summary,ensure_ascii=False,indent=2))
    print("JSON_FULL",output)


if __name__=="__main__":
    main()
