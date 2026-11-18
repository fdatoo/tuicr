"""Fake Claude used by private Ask scenarios; emits Claude's stream-json events."""

from . import Scenario, repos


def fake_claude(s: Scenario, body: str) -> None:
    script = repos.write(
        s.dir / "bin/claude",
        """#!/usr/bin/env python3
import json
import os
import sys
import time
from pathlib import Path


def emit(event):
    print(json.dumps(event), flush=True)


def activity(name):
    emit({"type": "stream_event", "event": {
        "type": "content_block_start", "content_block": {"type": "tool_use", "name": name},
    }})


def delta(text):
    emit({"type": "stream_event", "event": {
        "type": "content_block_delta", "delta": {"type": "text_delta", "text": text},
    }})


def finish(text):
    emit({"type": "result", "subtype": "success", "is_error": False, "result": text})


def answer(text):
    delta(text)
    finish(text)


prompt = sys.stdin.read()
emit({"type": "system", "subtype": "init"})
"""
        + body,
    )
    script.chmod(0o755)
    s.pre_path.insert(0, str(script.parent))
    s.env["TUICR_COMPANION"] = "claude"
