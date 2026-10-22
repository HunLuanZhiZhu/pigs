#!/usr/bin/env python3
"""Confirm PIGS and mini-proxy accept large bodies before JSON validation.

Intentionally invalid JSON means no real model call can be triggered.
Expected response is 400 from application JSON parsing, not 413 (Axum limit).
"""
from urllib.error import HTTPError
from urllib.request import Request, urlopen


def check(url: str, size_mib: int) -> None:
    invalid_json = b"x" * (size_mib * 1024 * 1024 + 1)
    req = Request(
        url + "/chat/completions",
        data=invalid_json,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urlopen(req, timeout=20) as resp:
            code, body = resp.status, resp.read(1000)
    except HTTPError as exc:
        code, body = exc.code, exc.read(1000)
    # 400 proves the Bytes extractor accepted the payload and the handler
    # reached JSON parsing. A limit error would be 413 and different content.
    print(f"{url}: {size_mib} MiB invalid JSON -> HTTP {code}", flush=True)
    if code != 400 or b"length limit exceeded" in body.lower():
        raise AssertionError(f"Failed large body forwarding check for {url}")


if __name__ == "__main__":
    for endpoint in ("http://127.0.0.1:7946", "http://127.0.0.1:3927", "http://172.17.0.1"):
        check(endpoint, 4)
        check(endpoint, 12)
