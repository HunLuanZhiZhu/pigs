#!/usr/bin/env python3
"""Register one OpenAI-compatible model at runtime, then run official BFCL V4 Multi-Turn."""
from __future__ import annotations

import argparse
import os
from pathlib import Path
from types import SimpleNamespace


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument('--registry-name', required=True)
    p.add_argument('--wire-model', required=True)
    p.add_argument('--base-url', required=True)
    p.add_argument('--api-key', required=True)
    p.add_argument('--session-id', required=True)
    p.add_argument('--result-dir', type=Path, required=True)
    p.add_argument('--score-dir', type=Path, required=True)
    p.add_argument('--threads', type=int, default=1)
    p.add_argument('--temperature', type=float, default=0.001)
    p.add_argument('--thinking-effort', default='low')
    p.add_argument('--request-timeout', type=float, default=1800.0)
    args = p.parse_args()
    if args.request_timeout <= 0:
        raise SystemExit('--request-timeout must be positive')

    os.environ['OPENAI_BASE_URL'] = args.base_url
    os.environ['OPENAI_API_KEY'] = args.api_key
    os.environ['OPENAI_DEFAULT_HEADERS'] = '{"x-opencode-session":"' + args.session_id + '"}'

    from bfcl_eval._llm_response_generation import main as generation_main
    from bfcl_eval.constants.model_config import ModelConfig, MODEL_CONFIG_MAPPING
    from bfcl_eval.eval_checker.eval_runner import main as evaluation_main
    from bfcl_eval.model_handler.api_inference.openai_completion import OpenAICompletionsHandler

    class EvaluationOpenAICompletionsHandler(OpenAICompletionsHandler):
        evaluation_reasoning_effort = args.thinking_effort
        evaluation_request_timeout = args.request_timeout

        def _build_client_kwargs(self):
            kwargs = super()._build_client_kwargs()
            kwargs['timeout'] = self.evaluation_request_timeout
            return kwargs

        def generate_with_backoff(self, **kwargs):
            kwargs['reasoning_effort'] = self.evaluation_reasoning_effort
            return super().generate_with_backoff(**kwargs)

    MODEL_CONFIG_MAPPING[args.registry_name] = ModelConfig(
        model_name=args.wire_model,
        display_name=args.wire_model,
        url='', org='PIGS evaluation', license='N/A',
        model_handler=EvaluationOpenAICompletionsHandler,
        input_price=None, output_price=None,
        is_fc_model=True,
        underscore_to_dot=True,
    )

    args.result_dir.mkdir(parents=True, exist_ok=True)
    args.score_dir.mkdir(parents=True, exist_ok=True)
    generation_main(SimpleNamespace(
        model=[args.registry_name], test_category=['multi_turn'],
        temperature=args.temperature, include_input_log=False, exclude_state_log=False,
        num_threads=args.threads, num_gpus=0, gpu_memory_utilization=0.0,
        backend='sglang', skip_server_setup=True, local_model_path=None,
        result_dir=str(args.result_dir), run_ids=False, allow_overwrite=False,
        enable_lora=False, max_lora_rank=None, lora_modules=None,
    ))
    evaluation_main(
        [args.registry_name], ['multi_turn'], str(args.result_dir), str(args.score_dir), False
    )
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
