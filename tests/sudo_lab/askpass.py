#!/usr/bin/env python3
"""Synthetic askpass program for the disposable compatibility lab."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path


def secret_for(profile: str) -> bytes:
    if profile == "base":
        return b"SudoLab-Base-Only-42!"
    if profile == "length-255":
        return b"A" * 254 + b"5"
    if profile == "length-256":
        return b"B" * 255 + b"6"
    if profile == "prefix-with-suffix":
        return b"SudoLab-Prefix" + b"\nignored-suffix"
    if profile == "prefix-with-carriage-return":
        return b"SudoLab-Prefix" + b"\rignored-suffix"
    raise ValueError("unknown synthetic profile")


def main() -> int:
    profile = os.environ.get("LAB_SECRET_PROFILE", "")
    mode = os.environ.get("LAB_ASKPASS_MODE", "newline")
    record_path = os.environ.get("LAB_ASKPASS_RECORD")
    prompt = sys.argv[1] if len(sys.argv) > 1 else ""

    if record_path:
        path = Path(record_path)
        prior = []
        if path.exists():
            prior = json.loads(path.read_text(encoding="utf-8"))
        prior.append(
            {
                "fd_9_open": os.path.exists("/proc/self/fd/9"),
                "profile": profile,
                "prompt": prompt,
                "prompt_type": os.environ.get("SSH_ASKPASS_PROMPT"),
            }
        )
        path.write_text(json.dumps(prior), encoding="utf-8")

    try:
        reply = secret_for(profile)
    except ValueError:
        return 70
    if mode == "newline":
        reply += b"\n"
    elif mode != "unterminated":
        return 71
    os.write(sys.stdout.fileno(), reply)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
