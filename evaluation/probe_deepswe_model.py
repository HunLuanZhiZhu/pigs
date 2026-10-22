#!/usr/bin/env python3
"""One inexpensive OpenAI-compatible DeepSeek call through local PIGS.

Usage from Ubuntu-24.04 WSL:
  python3 /mnt/d/AIWorkspace/pigs/evaluation/probe_deepswe_model.py
"""
import json
import os
from pathlib import Path
import sys
import tomllib
import uuid
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

endpoint = os.environ.get("DEEPSWE_API_ENDPOINT", "http://127.0.0.1:3927/chat/completions")
model = os.environ.get("DEEPSWE_MODEL", "deepseek-v4.1-flash")
api_key = os.environ.get("OPENAI_API_KEY", "local-evaluation")
if os.environ.get("DEEPSWE_USE_CONFIG_KEY") == "1":
    api_key = tomllib.loads(
        (Path(__file__).resolve().parents[1] / "config.local.toml").read_text(encoding="utf-8")
    )["key"]
payload = {
    "model": model,
    "messages": [{"role": "user", "content": "Reply with the single word pong."}],
    "max_tokens": 48,
    "stream": False,
}
headers = {
    "Content-Type": "application/json",
    "Authorization": "Bearer " + api_key,
    "User-Agent": "mini-swe-agent/2.4.6",
    "x-opencode-session": "ses_" + uuid.uuid4().hex,
}
if os.environ.get("DEEPSWE_SEND_X_API_KEY") == "1":
    headers["x-api-key"] = api_key
req = Request(
    endpoint,
    data=json.dumps(payload).encode("utf-8"),
    headers=headers,
    method="POST",
)
print(f"Probe endpoint={endpoint} model={model}", flush=True)
try:
    with urlopen(req, timeout=90) as response:
        result = json.load(response)
    print(json.dumps({
        "response_model": result.get("model"),
        "content": result.get("choices", [{}])[0].get("message", {}).get("content"),
        "usage": result.get("usage"),
    }, ensure_ascii=False), flush=True)
except HTTPError as exc:
    print(f"HTTP_ERROR={exc.code} {exc.read(500).decode('utf-8', 'replace')}", file=sys.stderr)
    sys.exit(1)
except URLError as exc:
    print(f"NETWORK_ERROR={exc.reason}", file=sys.stderr)
    sys.exit(2)
