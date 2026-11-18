#!/usr/bin/env python3
# Review-guide rebuild: the ordinary advance key cannot pass a risk item.
import json

from e2e.review_map import guide_fixture

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s, risky=True)
    document = json.loads(guide.read_text())
    stop = document["stories"][0]["stops"][1]
    stop["claim"] = "API returns the saved value to the original caller without recomputing a result from stale input."
    stop["risk"]["reason"] += " so the caller might no longer be able to recover from missing data."
    guide.write_text(json.dumps(document))
    tui = s.start("-w", "--guide", guide, cwd=repo, cols=80, rows=24)
    tui.wait_for(r"\. start").cmd("guide").wait_for(r"▶.*bridge context 17")
    tui.keys(".").wait_for(r"▶.*api change one").wait_for("Risk item: This path used to return an error")
    tui.wait_for("stale input").wait_for(r"\bdata\.").wait_for("K checks out")
    tui.keys(".").wait_for("Explicit verdict needed")
    assert "api change one" in tui.cursor_row(), tui.screen()
    tui.keys("K").wait_for(r"▶.*finish change one").reject("Explicit verdict needed")
    tui.quit()
