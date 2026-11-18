#!/usr/bin/env python3
# Review-guide rebuild: a file-tree filter skips hidden stops without abandoning the walk.
from e2e.review_map import guide_fixture

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s)
    tui = s.start("-w", "--guide", guide, cwd=repo)
    tui.wait_for("Review ready").keys(";h", "e").type(r"^api\.txt$").keys("Enter")
    tui.wait_gone(r"\bapi\.txt\b").cmd("guide").wait_for(r"▶.*bridge context 17")
    tui.keys(".").wait_for(r"▶.*finish change one").wait_for(r"Skipped hidden or changed stop: api\.txt")
    tui.wait_for("stop 1 of 1 · story 2 of 2").quit()
