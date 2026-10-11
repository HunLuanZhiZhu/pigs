"""BFCL 多轮退化样本的中文错误类型汇总。

每道题恰好计入一类。优先使用已经核对过的具体责任类型；
证据仅达到多项状态变更整体替换级别的样本单列“多种错误交织”，
不可把联合修复的每个操作都当作唯一错因。
"""
from __future__ import annotations
from collections import Counter

CATEGORIES = [
 ("extra","擅自多做了不必要的事","例如发动汽车后自行松刹车、无必要重新登录、提前设置导航"),
 ("premature","用户没说清楚就提前办事","例如股数、收件人、航班付款卡、保险方案还没明确就先执行"),
 ("missing","漏做了必要的动作","例如该查询发票却只看订单、该锁车却只查车况、该设置导航却停在找地址"),
 ("duplicate","同一件事做了两遍","例如前一个处理环节已经发送消息或下单，后一个环节又执行一次"),
 ("parameter","内容、金额或填写的信息错误","例如把欧元当美元、重复写标签、擅自改写消息正文"),
 ("sequence","操作的先后顺序或对象搞错","例如进入文件夹的时机错、查订单放错用户轮次"),
 ("combined","好几个问题叠在一起，不能只怪一个动作","尤其是需要同时修改多轮登录、订票和文件操作才能使整题通过的样本"),
 ("unresolved","资料不够或评分有冲突，暂不能下结论","外层步数限制强制停下，或正式评分与离线评分无法一致复现"),
]
MAP = {
 "base": {
   2:"sequence",5:"combined",9:"missing",11:"parameter",31:"missing",53:"extra",54:"extra",
   58:"extra",62:"extra",72:"extra",80:"extra",82:"extra",96:"extra",114:"duplicate",
   121:"missing",129:"premature",147:"parameter",172:"duplicate",177:"missing",180:"duplicate",
 },
 "long_context": {
   2:"sequence",36:"sequence",38:"combined",80:"extra",88:"extra",93:"missing",96:"extra",
   151:"combined",154:"missing",174:"combined",177:"missing",180:"duplicate",
 },
 "miss_func": {
   1:"unresolved",2:"combined",5:"combined",19:"unresolved",29:"missing",37:"missing",
   51:"extra",53:"duplicate",60:"missing",67:"extra",71:"premature",79:"extra",80:"extra",
   81:"extra",83:"extra",87:"missing",151:"combined",157:"extra",161:"extra",
   165:"combined",169:"combined",172:"parameter",174:"parameter",182:"combined",
 },
 "miss_param": {
   11:"parameter",32:"missing",36:"combined",46:"unresolved",61:"extra",62:"missing",63:"extra",
   68:"premature",69:"premature",76:"premature",85:"combined",86:"combined",90:"premature",
   91:"premature",93:"missing",96:"extra",107:"premature",111:"duplicate",116:"duplicate",
   119:"sequence",126:"premature",132:"premature",134:"unresolved",137:"extra",150:"extra",
   155:"premature",159:"combined",162:"combined",166:"combined",167:"combined",
   174:"combined",179:"combined",190:"combined",191:"premature",193:"missing",195:"premature",
 },
}

def primary_cause_map() -> dict[str,str]:
    all_cases={}
    for subset,items in MAP.items():
        for idx,category in items.items():
            all_cases[f"multi_turn_{subset}_{idx}"]=category
    return all_cases

def report_counts(case_ids: set[str]) -> list[dict]:
    labels=dict((k,(title,desc)) for k,title,desc in CATEGORIES)
    mapping=primary_cause_map()
    assert set(mapping)==set(case_ids),(
        f"分类对应不上：缺少 {sorted(set(case_ids)-set(mapping))}；多出 {sorted(set(mapping)-set(case_ids))}")
    counts=Counter(mapping.values())
    n=len(case_ids)
    assert n==92 and sum(counts.values())==n
    return [{
      "key":key,"title":title,"description":description,"count":counts[key],
      "percent":round(counts[key]*100/n,1),
      "ids":sorted([id for id,category in mapping.items() if category==key])
    } for key,title,description in CATEGORIES]

def validate_against_evidence(case_ids:set[str], proof_type):
    mapping=primary_cause_map()
    assert set(mapping)==case_ids
    unknown={id for id in case_ids if proof_type(id)[0] in ("stopped","nonparity")}
    assert {id for id in case_ids if mapping[id]=="unresolved"} == unknown
    batch={id for id in case_ids if proof_type(id)[0]=="batch"}
    assert batch <= {id for id in case_ids if mapping[id]=="combined"}
