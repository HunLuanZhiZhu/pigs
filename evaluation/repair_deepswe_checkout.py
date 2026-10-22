#!/usr/bin/env python3
"""Restore Unix newlines for executable shell scripts in the DeepSWE checkout.

The DeepSWE repository was cloned by Windows Git with autocrlf=true. That
turned #!/bin/bash into #!/bin/bash\r, which cannot be executed in Linux.
Only line endings of *.sh scripts are changed; task content is not rewritten.
"""
from pathlib import Path

ROOT = Path("/mnt/d/AIWorkspace/DeepSWE/deep-swe")
RUNNER = Path("/mnt/d/AIWorkspace/pigs/evaluation/run_deepswe_pig.sh")


def main() -> None:
    if not (ROOT / "tasks").is_dir():
        raise SystemExit(f"DeepSWE checkout not found: {ROOT}")
    scripts = sorted(ROOT.rglob("*.sh"))
    if RUNNER.is_file():
        scripts.append(RUNNER)
    edited = []
    for path in scripts:
        original = path.read_bytes()
        if b"\r\n" in original:
            path.write_bytes(original.replace(b"\r\n", b"\n"))
            edited.append(path)
    verifiers = list(ROOT.glob("tasks/*/tests/test.sh"))
    assert len(verifiers) == 113, f"expected 113 test.sh, found {len(verifiers)}"
    assert all(p.read_bytes().startswith(b"#!/bin/bash\n") for p in verifiers)
    assert not any(b"\r\n" in p.read_bytes() for p in verifiers)
    print(f"Shell files found: {len(scripts)}; normalized: {len(edited)}")
    print("All 113 verifier entrypoints now have valid Linux shebangs (LF).")
    print("No task instructions, patches, or grader source logic was changed.")


if __name__ == "__main__":
    main()
