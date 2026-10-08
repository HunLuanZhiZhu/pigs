#!/usr/bin/env python3
"""Initialize OpenAI Responses stream models before EvalScope starts worker threads.

OpenAI SDK/Pydantic can produce raw dicts for the first concurrent SSE
response.completed events when its response models are built concurrently.
This synthetic event initializes the same model without an API request.
"""
from __future__ import annotations

from openai._models import construct_type
from openai.types.responses import Response, ResponseStreamEvent


def prewarm_responses_models() -> None:
    event = {
        'type': 'response.completed',
        'sequence_number': 1,
        'response': {
            'id': 'local-sdk-prewarm',
            'object': 'response',
            'created_at': 0,
            'model': 'local-sdk-prewarm',
            'output': [],
            'status': 'completed',
        },
    }
    result = construct_type(type_=ResponseStreamEvent, value=event)
    if not isinstance(result.response, Response):
        raise RuntimeError(f'Responses SDK prewarm failed: {type(result.response).__name__}')
    print('OpenAI Responses SDK prewarm OK (no API request)', flush=True)


def main() -> int:
    import sys

    prewarm_responses_models()
    from evalscope.cli.cli import run_cmd
    sys.argv[0] = 'evalscope'
    return run_cmd()


if __name__ == '__main__':
    raise SystemExit(main())
