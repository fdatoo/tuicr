#!/usr/bin/env python3
# Review-guide rebuild: the claim and verdict/navigation keys survive a 60x15 terminal.
from e2e.review_map import guide_fixture

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s, risky=True)
    tui = s.start("-w", "--guide", guide, cwd=repo, cols=60, rows=15)
    tui.wait_for(r"\. start").keys(".").wait_for("Bridge forwards the request")
    tui.wait_for(r"\. next +K pass +x wrong +p park").wait_for(r", prev +' return +t map +a ask")
    tui.wait_for(r"▶.*bridge context 17")
    tui.keys(".").wait_for("API returns the saved value").wait_for(r"K checks out +x wrong +p park")
    tui.reject(r"\. next")
    tui.keys(".").wait_for("Explicit verdict needed").keys("K").wait_for("Finish reports its outcome")
    tui.quit()
