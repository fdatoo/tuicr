#!/usr/bin/env python3
# Review-guide rebuild: read-only CLI JSON and Markdown identify orphaned comment code explicitly.
import json

from e2e.review_map import guide_fixture

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s)
    path = repo / "api.txt"
    clipboard = s.fake_clipboard()
    tui = s.start("-w", "--guide", guide, cwd=repo, keep=True)
    tui.wait_for("Review ready").cursor_on("api change one").comment("Please revise this return value")
    [session] = json.loads(s.cli("review", "list", "--repo", repo, cwd=repo))
    tui.quit()

    repos.write(path, path.read_text().replace("api change one", "api now returns none"))
    [stale] = json.loads(s.cli("review", "comments", "--session", session["slug"], "--repo", repo, cwd=repo))
    assert stale["stale"] and stale["location"] == "api.txt:20", stale
    assert stale["original_text"] == ["api change one"], stale

    reopened = s.start("-w", "--guide", guide, cwd=repo, keep=True)
    reopened.wait_for("Guide prepared for an earlier version").cursor_on("api now returns none")
    reopened.wait_for("Code changed since this comment")
    reopened.cmd("clip")
    exported = clipboard.text()
    assert "stale: true" in exported and "api change one" in exported, exported
    reopened.quit()
