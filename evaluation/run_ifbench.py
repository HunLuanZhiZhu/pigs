#!/usr/bin/env python3
"""IFBench official 300-sample evaluation via the existing EvalScope runner."""
from __future__ import annotations

import os

from eval_common import EVAL_ROOT
from evalscope_dataset import run_dataset

IFBENCH_DATA = EVAL_ROOT / 'datasets/ifbench/allenai_IFBench_test'
# 早期免费模型评分器依赖独立于共享环境。 / Earlier free-model scorer dependencies are isolated from the
# 所有 EvalScope 子进程须继承 PYTHONPATH。 / shared WSL venv; inherit PYTHONPATH for EvalScope subprocesses.
IFBENCH_DEPS = EVAL_ROOT / 'isolated-deps/ifbench'


if __name__ == '__main__':
    if IFBENCH_DEPS.is_dir():
        os.environ['PYTHONPATH'] = os.pathsep.join(
            filter(None, [str(IFBENCH_DEPS), os.environ.get('PYTHONPATH', '')])
        )
    raise SystemExit(run_dataset('ifbench', 'ifbench', 300, 'IFBench',
                                 local_dataset=IFBENCH_DATA))
