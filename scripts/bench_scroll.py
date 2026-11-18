#!/usr/bin/env python3
"""Measure wheel input through a real Unix PTY, without a forge, AI service or display.

python3 scripts/bench_scroll.py /path/to/release/tuicr [another/release/tuicr ...]
Times include terminal output up to synchronized-update end, not physical screen scanout.
"""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import pty
import select
import signal
import statistics
import struct
import subprocess
import tempfile
import termios
import time
from pathlib import Path

FRAME_END = b"\x1b[?2026l"
WHEEL_DOWN = b"\x1b[<65;90;20M"
WHEEL_UP = b"\x1b[<64;90;20M"


class Frames:
    def __init__(self, fd: int) -> None:
        self.fd = fd
        self.pending = bytearray()

    def next(self) -> bytes:
        while FRAME_END not in self.pending:
            if not select.select([self.fd], [], [], 30)[0]:
                raise TimeoutError("no complete tuicr repaint within 30 seconds")
            chunk = os.read(self.fd, 65536)
            if not chunk:
                raise EOFError("tuicr exited before repaint")
            self.pending.extend(chunk)
        end = self.pending.index(FRAME_END)
        frame = bytes(self.pending[:end])
        del self.pending[: end + len(FRAME_END)]
        return frame


def make_repo(root: Path, files: int, rows: int) -> Path:
    repo = root / f"repo-{files}-{rows}"
    subprocess.run(["git", "init", "-q", repo], check=True)
    text = "".join(f"row {i:06} short text\n" for i in range(rows))
    for n in range(files):
        (repo / f"part{n:03}.txt").write_text(text, encoding="utf-8")
    subprocess.run(["git", "-C", repo, "add", "."], check=True)
    return repo


def measure(binary: Path, repo: Path, home: Path, layout: str) -> None:
    conf = home / ".config/tuicr"
    conf.mkdir(parents=True)
    (conf / "config.toml").write_text(f'diff_view = "{layout}"\nmouse = true\n', encoding="utf-8")
    env = {
        "PATH": os.environ["PATH"],
        "LANG": os.environ.get("LANG", "en_US.UTF-8"),
        "TERM": "xterm-256color",
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(conf.parent),
        "XDG_DATA_HOME": str(home / "data"),
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
    }
    pid, fd = pty.fork()
    if pid == 0:
        try:
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
            os.chdir(repo)
            os.execve(binary, [str(binary), "--no-update-check", "--stdout", "--theme", "dark", "-w"], env)
        finally:
            os._exit(1)
    try:
        frames = Frames(fd)
        if b"part000.txt" not in frames.next():
            raise RuntimeError("fixture diff was not shown")
        single: list[float] = []
        for event in [WHEEL_DOWN] * 18 + [WHEEL_UP] * 8:
            start = time.monotonic_ns()
            os.write(fd, event)
            frame = frames.next()
            single.append((time.monotonic_ns() - start) / 1e6)
            if len(frame) < 64:
                raise RuntimeError("wheel event did not move visible content")
        burst: list[float] = []
        for _ in range(5):
            # Ctrl-L after the reversal is an ordered barrier. Its full redraw is included for both binaries.
            start = time.monotonic_ns()
            os.write(fd, WHEEL_DOWN * 48 + WHEEL_UP * 40 + b"\x0c")
            while b"\x1b[2J" not in frames.next():
                pass
            burst.append((time.monotonic_ns() - start) / 1e6)
        print(
            json.dumps(
                {
                    "binary": str(binary),
                    "fixture": repo.name,
                    "layout": layout,
                    "p95_wheel_ms": round(sorted(single)[24], 2),
                    "median_burst_to_barrier_ms": round(statistics.median(burst), 2),
                }
            ),
            flush=True,
        )
    finally:
        os.kill(pid, signal.SIGTERM)
        os.waitpid(pid, 0)
        os.close(fd)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="+", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="tuicr-wheel-") as tmp:
        root = Path(tmp)
        for files, rows in [(1, 300), (1, 30000), (100, 300)]:
            repo = make_repo(root, files, rows)
            for layout in ["unified", "side-by-side"]:
                for i, binary in enumerate(args.binary):
                    measure(binary.resolve(strict=True), repo, root / f"home-{files}-{rows}-{layout}-{i}", layout)


if __name__ == "__main__":
    main()
