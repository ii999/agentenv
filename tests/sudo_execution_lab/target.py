#!/usr/bin/env python3
"""Synthetic privileged target: literal argv, independent streams and status."""
import json
import sys

if sys.argv[1:] == ["--bulk"]:
    sys.stdout.buffer.write(bytes(range(256)) * 8193)
    sys.stderr.buffer.write(bytes(reversed(range(256))) * 8193)
    raise SystemExit(0)

sys.stdout.buffer.write(json.dumps(sys.argv[2:], ensure_ascii=False).encode() + b"\x00\xff")
sys.stderr.buffer.write(b"target-stderr\x00\xfe")
raise SystemExit(int(sys.argv[1]))
