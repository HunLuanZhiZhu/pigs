#!/usr/bin/env python3
"""Compare OpenCode Console/Go inference endpoints with the local PIGS key.

This probe reads config.local.toml but NEVER prints the key or request headers.
It calls upstream directly, bypassing mini-proxy's long retry loop.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import json
from pathlib import Path
import time
import tomllib
import uuid
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
ROUTES = (
    ("old-go", "https://opencode.ai/zen/go/v1"),
    ("new-go", "https://opencode.ai/inference/go/v1"),
    ("new-go-explicit", "https://opencode.ai/inference/go/openai/v1"),
    ("console", "https://opencode.ai/inference/openai/v1"),
)
SESSION_ID = "ses_" + uuid.uuid4().hex


def probe(name: str, url: str, key: str, method: str, timeout: float) -> dict:
    headers = {
        "Authorization": "Bearer " + key,
        "Accept": "application/json",
        "Content-Type": "application/json",
        "User-Agent": "pigs-deepswe-evaluation/1.0",
        "x-opencode-session": SESSION_ID,
    }
    if method == "GET":
        request = Request(url + "/models", headers=headers, method="GET")
    else:
        payload = {
            "model": "deepseek-v4.1-flash",
            "messages": [{"role": "user", "content": "For a coding-agent connectivity check, write only the word pong."}],
            "max_tokens": 24,
            "stream": False,
        }
        request = Request(
            url + "/chat/completions",
            headers=headers,
            data=json.dumps(payload).encode("utf-8"),
            method="POST",
        )
    start = time.monotonic()
    outcome = {"endpoint": name, "method": method}
    try:
        with urlopen(request, timeout=timeout) as response:
            outcome["status"] = response.status
            body = response.read(250_000)
            try:
                data = json.loads(body)
            except (ValueError, UnicodeError):
                data = {}
            if method == "GET":
                if isinstance(data, dict) and isinstance(data.get("data"), list):
                    names = [item.get("id") for item in data["data"] if isinstance(item, dict)]
                    outcome["models"] = len(names)
                    outcome["deepseek-v4.1-flash_listed"] = "deepseek-v4.1-flash" in names
            elif isinstance(data, dict):
                outcome["response_model"] = data.get("model")
                outcome["has_choices"] = bool(data.get("choices"))
    except HTTPError as exc:
        outcome["status"] = exc.code
        # Do not print response bodies: third-party errors may contain private data.
    except (URLError, TimeoutError, OSError) as exc:
        outcome["status"] = "network_error"
        outcome["reason_type"] = type(exc).__name__
    outcome["seconds"] = round(time.monotonic() - start, 2)
    return outcome


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--post", action="store_true", help="send a low-token completion to the endpoints")
    parser.add_argument("--timeout", type=float, default=12.0)
    args = parser.parse_args()
    key = tomllib.loads((ROOT / "config.local.toml").read_text(encoding="utf-8")).get("key", "")
    if not key:
        parser.error("No API key in config.local.toml")
    method = "POST" if args.post else "GET"
    print(f"Testing {method} endpoints with configured credential; credentials are hidden.", flush=True)
    with ThreadPoolExecutor(max_workers=4) as pool:
        futures = {
            pool.submit(probe, name, url, key, method, args.timeout): name
            for name, url in ROUTES
        }
        for future in as_completed(futures):
            print(json.dumps(future.result(), ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
