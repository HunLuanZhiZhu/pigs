"""兼容 PIGS 历史日志与当前代码使用的阶段名称。"""
from __future__ import annotations


def normalize_phase(value: str) -> str:
    """旧 Executor/executor 和新 Exec/exec 均标准化为 Exec。"""
    return {
        "pre": "Pre",
        "executor": "Exec",
        "exec": "Exec",
        "post": "Post",
        "unknown": "Unknown",
    }.get(value.strip().lower(), "Unknown")
