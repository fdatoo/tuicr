#!/usr/bin/env python3
# Review-guide rebuild: draft comments follow the exact source block across reload and reopen.
from e2e.review_map import guide_fixture

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s)
    path = repo / "api.txt"
    original = path.read_text()
    tui = s.start("-w", "--guide", guide, cwd=repo, keep=True)
    tui.wait_for("Review ready").cursor_on("api change one").comment("Please preserve this anchor")
    [comment] = s.session()["files"]["api.txt"]["line_comments"]["20"]
    assert comment["original_text"] == ["api change one"], comment

    repos.write(path, "inserted before the anchor\n" + original)
    tui.cmd("e").wait_for("inserted before the anchor")
    tui.cmd("summary").wait_for(r"api\.txt:21").wait_for("Please preserve this anchor")
    tui.keys("Escape").quit()

    repos.write(path, "another inserted line\n" + path.read_text())
    reopened = s.start("-w", "--guide", guide, cwd=repo, keep=True)
    reopened.wait_for("Guide prepared for an earlier version")
    reopened.cmd("summary").wait_for(r"api\.txt:22").wait_for("Please preserve this anchor")
    reopened.keys("Escape").quit()
