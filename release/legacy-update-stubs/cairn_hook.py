#!/usr/bin/env python3
"""Stub published so a 0.1.5 `cairn update` can finish. The hook is now `cairn hook`."""
import sys

sys.stderr.write("cairn: the Python hook is retired; run `cairn setup <server-url> --agent claude|codex|opencode`\n")
sys.exit(0)
