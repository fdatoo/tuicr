#!/usr/bin/env python3
# Fork review guide: keep the block and risk visible across wrap, resize, and dock changes.
import re

from e2e.review_map import guide_fixture

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s, risky=True, long_context=True)
    tui = s.start("-w", "--guide", guide, cwd=repo, cols=80, rows=24)
    tui.wait_for(r"\. start").keys(".").wait_for(r"▶.*bridge context 17")
    tui.keys(".").wait_for(r"▶.*api change one")
    tui.cmd("set wrap").resize(60, 15).wait_for(r"▶.*api change")
    tui.keys("C-f").wait_for("Off stop").keys("'").wait_for(r"▶.*api change").reject("Off stop")
    tui.resize(80, 24).wait_for(r"▶.*api change one")
    tui.cmd("diff").wait_for(r"▶.*api change one")
    tui.resize(60, 15).wait_for(r"▶.*api change").wait_for(r"\bcontract\.")
    # The start and tail of the first logical row may paint on different screen rows.
    tui.keys("a").wait_for("Question:").wait_for(r"\bcontract\.")
    tui.wait_for(r"▶.*\bapi\b").reject("Prepared explanation")
    screen = tui.screen(join=False)
    assert re.search(r"▶[^\n]*\bapi\b[^\n]*(?:\bone\b|\n[^\n]*\bone\b)", screen), screen
    tui.keys("Escape").cmd("guide detail").wait_for("Guide stop").wait_for(r"\bcontract\.")
    tui.keys("G").wait_for(r"\bamount\.")
    tui.keys("Escape").wait_for(r"▶.*api change")
    tui.resize(125, 55).cmd("diff").wait_for(r"▶.*api change one").wait_for(r"checkout total.*…")
    tui.reject("a for more")
    tui.wait_for("3 stops · ~12 min · 3 of 4 hunks").wait_for("stop 2 of 2 · story 1 of 2 · 2 left")
