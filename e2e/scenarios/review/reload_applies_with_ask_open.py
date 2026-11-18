#!/usr/bin/env python3
# Review-guide rebuild: delivering a watched diff leaves the docked private thread open.
from e2e.review_map import guide_fixture

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s)
    path = repo / "bridge.txt"
    repos.write(s.dir / "config/tuicr/config.toml", "diff_watch_interval_ms = 200\n")
    tui = s.start("-w", "--guide", guide, cwd=repo, keep=True)
    tui.wait_for("Review ready").cursor_on("bridge change one").keys("a")
    tui.wait_for("Private").wait_for(r"▶.*bridge change one")
    tui.type("Why is this necessary?")

    repos.write(path, path.read_text().replace("bridge change one", "bridge adjusted while asking"))
    tui.wait_for(r"▶.*bridge adjusted while asking").wait_for("thread is about earlier code")
    tui.wait_for("Why is this necessary?").wait_for("Private")
    tui.keys("Escape").wait_gone("thread is about earlier code")
    tui.quit()
