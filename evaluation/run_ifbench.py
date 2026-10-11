#!/usr/bin/env python3
"""IFBench official 300-sample evaluation via the existing EvalScope runner."""
from __future__ import annotations

import os

from eval_common import EVAL_ROOT
from evalscope_dataset import run_dataset

IFBENCH_DATA = EVAL_ROOT / 'datasets/ifbench/allenai_IFBench_test'
# Dependencies from the earlier free-model scorer check are isolated from the
# shared WSL venv. Inherit PYTHONPATH for all EvalScope subprocesses.
IFBENCH_DEPS = EVAL_ROOT / 'isolated-deps/ifbench'


if __name__ == '__main__':
    if IFBENCH_DEPS.is_dir():
        os.environ['PYTHONPATH'] = os.pathsep.join(
            filter(None, [str(IFBENCH_DEPS), os.environ.get('PYTHONPATH', '')])
        )
    raise SystemExit(run_dataset('ifbench', 'ifbench', 300, 'IFBench',
                                 local_dataset=IFBENCH_DATA))
