#!/usr/bin/env python3
# Fork review guide: quitting while Ask is waiting kills that provider (preparation is detached).
import os

from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario, wait_file


@scenario
def main(s: Scenario) -> None:
    repo = repos.git_repo(s.dir / "repo")
    release = s.dir / "release"
    s.on_exit(release.touch)
    fake_claude(
        s,
        f"""
assert 'shutdown chat question' in prompt
activity('Read')
marker = Path({str(s.dir / "chat")!r})
pending = marker.with_suffix('.tmp')
pending.write_text(f'{{os.getpid()}}\\n')
pending.replace(marker)
while not Path({str(release)!r}).exists():
    time.sleep(0.05)
""",
    )
    tui = s.start(cwd=repo)
    tui.wait_for("Unstaged changes").keys("Enter").wait_for("unstaged line")
    tui.cursor_on("unstaged line").cmd("ask").wait_for("Private companion")
    tui.type("shutdown chat question").keys("Enter").wait_for("Reading source")
    pid = int(wait_file(s.dir / "chat").read_text())
    tui.keys("Escape").wait_gone("Private companion")
    os.kill(pid, 0)  # Hiding Ask does not cancel its request.
    tui.keys(":q!", "Enter").wait_exit()

    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        pass
    else:
        raise AssertionError("the Ask provider was still running after quitting")
