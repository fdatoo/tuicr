#!/usr/bin/env python3
# Review-guide rebuild: wrong opens an ordinary range comment, and only saving moves on.
from e2e.review_map import guide_fixture

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s)
    tui = s.start("-w", "--guide", guide, cwd=repo)
    tui.wait_for("Review ready").cmd("set novim").keys(".").wait_for(r"▶.*bridge context 17")
    draft = "This does not check out: Bridge forwards the request"
    tui.keys("x").wait_for(draft).keys("Escape").wait_gone(draft)
    tui.wait_for("stop 1 of 2 · story 1 of 2").keys("x").wait_for(draft)
    tui.keys("C-s").wait_for(r"▶.*api change one").wait_for("stop 2 of 2 · story 1 of 2")
    comments = s.session()["files"]["bridge.txt"]["line_comments"]["21"]
    [comment] = comments
    assert comment["content"] == draft, comment
    assert comment["line_range"] == {"start": 17, "end": 21} and comment["side"] == "new", comment
    tui.quit()
