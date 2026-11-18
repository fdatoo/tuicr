#!/usr/bin/env python3
# Review-guide rebuild: Sessions discovers a guide without saved review progress or launch flags.
from e2e.prepare import fake_claude, review_fixture

from e2e import Scenario, scenario, wait_file


@scenario
def main(s: Scenario) -> None:
    repo, guide = review_fixture(s)
    release = s.dir / "release-reviewer"
    s.on_exit(release.touch)
    calls = fake_claude(
        s,
        response={
            "guide": guide,
            "audit": {"stops": [{"id": "greeting", "audit": {"result": "holds"}, "untested": True}]},
        },
        release=release,
    )
    tui = s.start("-w", cwd=repo, keep=True)
    tui.wait_for("reviewed.rs").cmd("prepare").wait_for("Preparing review ·", 10)
    wait_file(calls)
    tui.cmd("sessions").wait_for(r"guide +Working tree changes .*preparing ·")
    tui.keys("Enter").wait_for("Preparing review ·")
    release.touch()
    tui.wait_for("Review ready: 1 stops", 20)
    tui.quit()

    tui = s.start(cwd=repo, keep=True)
    tui.wait_for("Pull Requests").keys("BTab").wait_for(r"guide +Working tree changes .*ready · 1 stops")
    tui.keys("Enter").wait_for("Review ready: 1 stops").keys(".")
    tui.wait_for("The greeting is a static string")
    tui.quit()
