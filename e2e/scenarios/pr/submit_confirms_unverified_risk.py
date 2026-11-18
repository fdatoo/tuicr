#!/usr/bin/env python3
# Review-guide rebuild: both kinds of approval name the unverified risk and require an explicit yes.
from e2e.coverage import guided_pr

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    tui = guided_pr(s, risky=True)
    warning = r"1 risk item is unverified\. Approve anyway\?"
    tui.cmd("submit").wait_for("> Approve what I verified").keys("Enter").wait_for(warning)
    tui.keys("Enter").settle().check(warning, "Enter must not confirm the risk warning")
    assert tui.posts() == [], "scope approval did not require an explicit yes"
    tui.keys("Escape").wait_gone(warning)
    tui.cmd("submit").wait_for("> Approve what I verified").keys("j", "Enter").wait_for(warning)
    tui.keys("Enter").settle().check(warning, "plain approval must also require an explicit yes")
    assert tui.posts() == [], "plain approval bypassed risk confirmation"
    tui.keys("y").wait_for("Submitted GitHub review #1001")
    [post] = tui.posts()
    assert post["payload"]["event"] == "APPROVE", post
    assert not post["payload"].get("body"), post
    tui.quit()
