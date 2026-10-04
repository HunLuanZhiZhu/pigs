#!/usr/bin/env python3
"""Compatibility front door for the redesigned PIGS paper evaluation.

Formal runs are intentionally dataset-specific so each benchmark can use its
own official harness and can be launched/monitored independently.
"""
from pathlib import Path

ENTRIES = {
    'gsm8k': 'run_gsm8k.py',
    'ifeval': 'run_ifeval.py',
    'bfcl_multiturn': 'run_bfcl_multiturn.py',
    'swebench_lite': 'run_swebench_lite.py',
}

if __name__ == '__main__':
    here=Path(__file__).resolve().parent
    print('PIGS formal evaluation now uses one entry script per benchmark:')
    for name,script in ENTRIES.items():
        print(f'  {name:16s} python {here/script}')
    print('\nEach script supports --models, --model-workers, --sample-workers, --background and file-based progress.')
    print('Use --help on the dataset script for its exact options.')
