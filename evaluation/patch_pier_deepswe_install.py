#!/usr/bin/env python3
"""Make Pier's mini-swe-agent *installation* use cached uv and a China PyPI mirror.

No benchmark prompts, agent loop, verifier, network isolation, or model behavior
is changed. Idempotent. Run in Ubuntu-24.04, with the Pier Python venv:

  /root/pigs-eval/deepswe-venv/bin/python \
    /mnt/d/AIWorkspace/pigs/evaluation/patch_pier_deepswe_install.py

Patch applies only to an installed copy of datacurve-pier 0.3.1.
"""
from __future__ import annotations

import importlib.metadata
from pathlib import Path
import sys

if importlib.metadata.version("datacurve-pier") != "0.3.1":
    raise SystemExit("Refusing to patch unexpected Pier version")

path = Path(sys.prefix) / "lib/python3.12/site-packages/pier/agents/installed/mini_swe_agent.py"
source = path.read_text(encoding="utf-8")
old_installer = "curl -LsSf https://astral.sh/uv/0.7.13/install.sh | sh"
new_installer = (
    'if ! command -v uv >/dev/null 2>&1; then\n'
    f'  {old_installer}\n'
    'fi'
)
old_env = 'source "$HOME/.local/bin/env"'
new_env = 'if [ -f "$HOME/.local/bin/env" ]; then source "$HOME/.local/bin/env"; fi'
old_install = "uv tool install mini-swe-agent{version_spec}"
new_install = (
    "UV_DEFAULT_INDEX=https://pypi.tuna.tsinghua.edu.cn/simple "
    "uv tool install mini-swe-agent{version_spec}"
)
replacements = (
    (old_installer, new_installer),
    (old_env, new_env),
    (old_install, new_install),
    (
        '"if command -v apt-get &>/dev/null; then"',
        '"if command -v curl &>/dev/null && command -v git &>/dev/null && command -v gcc &>/dev/null && command -v make &>/dev/null; then echo \'Build tools already installed\'; elif command -v apt-get &>/dev/null; then"',
    ),
    (
        "uv tool install mini-swe-agent{version_spec}",
        "uv tool install --with mcp mini-swe-agent{version_spec}",
    ),
)

if all(new in source for _, new in replacements):
    print("Pier mini-swe-agent install patch already present.")
else:
    applied = 0
    for old, new in replacements:
        if new in source:
            continue
        count = source.count(old)
        if count != 1:
            raise SystemExit(f"Expected one instance of {old[:34]!r}; found {count}")
        source = source.replace(old, new, 1)
        applied += 1
    compile(source, str(path), "exec")
    backup = path.with_suffix(".py.pre-deepswe-install.bak")
    if not backup.exists():
        backup.write_text(path.read_text(encoding="utf-8"), encoding="utf-8")
    path.write_text(source, encoding="utf-8")
    print(f"Patched Pier mini-swe-agent installation only ({applied} substitutions): {path}")
    print(f"Original Pier 0.3.1 file backed up: {backup}")
