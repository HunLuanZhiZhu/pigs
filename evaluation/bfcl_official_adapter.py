#!/usr/bin/env python3
"""Register an OpenAI-compatible model for official BFCL V4 Multi-Turn evaluation."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import threading
import time
from contextvars import ContextVar
from pathlib import Path
from types import SimpleNamespace


def _jsonable(value):
    if hasattr(value, 'model_dump'):
        return _jsonable(value.model_dump(exclude_none=True))
    if isinstance(value, dict):
        return {str(k): _jsonable(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_jsonable(v) for v in value]
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    return str(value)


def request_fingerprint(payload: dict) -> str:
    """Stable fingerprint of model-visible BFCL request content.

    The fingerprint deliberately ignores transport headers and request-only tuning
    fields. It is used only to join the adapter's local request map to archived
    PIGS client-request bodies after the run.
    """
    content = payload.get('messages')
    if content is None:
        content = payload.get('input')
    core = {
        'model': payload.get('model'),
        'content': _jsonable(content),
        'tools': _jsonable(payload.get('tools', [])),
    }
    raw = json.dumps(core, ensure_ascii=False, sort_keys=True, separators=(',', ':'))
    return hashlib.sha256(raw.encode('utf-8')).hexdigest()


class BFCLLocalTraceMixin:
    """Record sample/turn identity locally without changing HTTP requests."""

    evaluation_request_map: Path | None = None
    _trace_context: ContextVar[dict | None] = ContextVar('bfcl_trace_context', default=None)
    _trace_lock = threading.Lock()

    def _pre_query_processing_FC(self, inference_data: dict, test_entry: dict) -> dict:
        inference_data = super()._pre_query_processing_FC(inference_data, test_entry)
        inference_data['_bfcl_sample_id'] = str(test_entry['id'])
        inference_data['_bfcl_turn_id'] = -1
        inference_data['_bfcl_request_index'] = 0
        return inference_data

    def add_first_turn_message_FC(
        self, inference_data: dict, first_turn_message: list[dict]
    ) -> dict:
        inference_data = super().add_first_turn_message_FC(
            inference_data, first_turn_message
        )
        inference_data['_bfcl_turn_id'] = 0
        inference_data['_bfcl_request_index'] = 0
        return inference_data

    def _add_next_turn_user_message_FC(
        self, inference_data: dict, user_message: list[dict]
    ) -> dict:
        inference_data = super()._add_next_turn_user_message_FC(
            inference_data, user_message
        )
        inference_data['_bfcl_turn_id'] = int(inference_data['_bfcl_turn_id']) + 1
        inference_data['_bfcl_request_index'] = 0
        return inference_data

    def _query_FC(self, inference_data: dict):
        meta = {
            'sample_id': str(inference_data['_bfcl_sample_id']),
            'turn_id': int(inference_data['_bfcl_turn_id']),
            'request_index': int(inference_data['_bfcl_request_index']),
        }
        inference_data['_bfcl_request_index'] = meta['request_index'] + 1
        token = self._trace_context.set(meta)
        try:
            return super()._query_FC(inference_data)
        finally:
            self._trace_context.reset(token)

    def _record_local_request(self, kwargs: dict) -> None:
        meta = self._trace_context.get()
        path = self.evaluation_request_map
        if meta is None or path is None:
            return
        row = {
            'sent_at_ns': time.time_ns(),
            **meta,
            'fingerprint': request_fingerprint(kwargs),
        }
        line = json.dumps(row, ensure_ascii=False, separators=(',', ':')) + '\n'
        with self._trace_lock:
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open('a', encoding='utf-8') as f:
                f.write(line)


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument('--registry-name', required=True)
    p.add_argument('--wire-model', required=True)
    p.add_argument('--base-url', required=True)
    p.add_argument('--api-key', required=True)
    p.add_argument('--session-id', required=True)
    p.add_argument('--result-dir', type=Path, required=True)
    p.add_argument('--score-dir', type=Path, required=True)
    p.add_argument('--request-map', type=Path, required=True,
                   help='Local JSONL request map; never sent over HTTP.')
    p.add_argument('--threads', type=int, default=1)
    p.add_argument('--temperature', type=float, default=0.001)
    p.add_argument('--thinking-effort', default='low')
    p.add_argument('--eval-type', choices=['openai_api', 'openai_responses_api'], default=None)
    p.add_argument('--protocol', choices=['chat', 'responses'], default='chat',
                   help='Alias for official BFCL Chat/Responses protocol selection.')
    p.add_argument('--request-timeout', type=float, default=1800.0)
    args = p.parse_args()
    if args.request_timeout <= 0 or args.threads <= 0:
        raise SystemExit('request timeout and threads must be positive')

    os.environ['OPENAI_BASE_URL'] = args.base_url
    os.environ['OPENAI_API_KEY'] = args.api_key
    os.environ['OPENAI_DEFAULT_HEADERS'] = '{"x-opencode-session":"' + args.session_id + '"}'

    from bfcl_eval._llm_response_generation import main as generation_main
    from bfcl_eval.constants.model_config import ModelConfig, MODEL_CONFIG_MAPPING
    from bfcl_eval.eval_checker.eval_runner import main as evaluation_main
    from bfcl_eval.model_handler.api_inference.openai_completion import OpenAICompletionsHandler
    from bfcl_eval.model_handler.api_inference.openai_response import OpenAIResponsesHandler

    class EvaluationOpenAICompletionsHandler(BFCLLocalTraceMixin, OpenAICompletionsHandler):
        evaluation_reasoning_effort = args.thinking_effort
        evaluation_request_timeout = args.request_timeout
        evaluation_request_map = args.request_map

        def _build_client_kwargs(self):
            kwargs = super()._build_client_kwargs()
            kwargs['timeout'] = self.evaluation_request_timeout
            return kwargs

        def generate_with_backoff(self, **kwargs):
            self._record_local_request(kwargs)
            kwargs['reasoning_effort'] = self.evaluation_reasoning_effort
            return super().generate_with_backoff(**kwargs)

    class EvaluationOpenAIResponsesHandler(BFCLLocalTraceMixin, OpenAIResponsesHandler):
        evaluation_reasoning_effort = args.thinking_effort
        evaluation_request_timeout = args.request_timeout
        evaluation_request_map = args.request_map

        def _build_client_kwargs(self):
            kwargs = super()._build_client_kwargs()
            kwargs['timeout'] = self.evaluation_request_timeout
            return kwargs

        def generate_with_backoff(self, **kwargs):
            # BFCL 官方 Responses 处理器负责智能体/工具循环。 / BFCL's official Responses handler owns the agent/tool loop.
            # 本地记录请求映射，不修改线上请求；同时维持统一的低推理预算。 / Record the request map locally without changing the wire request.
            self._record_local_request(kwargs)
            kwargs['reasoning'] = {'effort': self.evaluation_reasoning_effort}
            return super().generate_with_backoff(**kwargs)

    use_responses = (args.eval_type == 'openai_responses_api'
                     if args.eval_type is not None else args.protocol == 'responses')
    handler = (EvaluationOpenAIResponsesHandler
               if use_responses else EvaluationOpenAICompletionsHandler)

    MODEL_CONFIG_MAPPING[args.registry_name] = ModelConfig(
        model_name=args.wire_model,
        display_name=args.wire_model,
        url='', org='PIGS evaluation', license='N/A',
        model_handler=handler,
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
