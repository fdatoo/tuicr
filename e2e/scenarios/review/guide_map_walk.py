#!/usr/bin/env python3
# Review-guide rebuild: walk ordered stories, detour and return, then resume after a restart.
import re

from e2e.review_map import guide_fixture

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s)
    tui = s.start("-w", "--theme", "dark", "--guide", guide, cwd=repo, keep=True)
    tui.wait_for(r"Review ready: 3 stops.*\. start").reject("Bridge forwards the request")
    tui.cmd("version").wait_for(r"tuicr v\d").wait_for(r"Review ready: 3 stops.*\. start")
    tui.keys(".").wait_for(r"▶.*bridge context 17").wait_for("stop 1 of 2 · story 1 of 2")
    tui.wait_gone(r"\. start").wait_for("3 left")
    tui.wait_for("Not audited").wait_for("Supporting changes ▸")
    tui.wait_for("3 of 4 hunks")
    screen = tui.screen().splitlines()
    cursor = next(i for i, line in enumerate(screen) if "▶" in line and "bridge context 17" in line)
    for n in (14, 15, 16):
        assert any(f"bridge context {n}" in line for line in screen[:cursor]), tui.screen()

    def context_ink(text: str, rgb: str) -> None:
        colored = tui.screen(colors=True)
        row = next(row for row in colored.splitlines() if text in row)
        assert re.search(rf"\x1b\[[0-9;]*38;2;{rgb}(?:;[0-9;]*)?m[^\x1b]*{text}", row), repr(row)

    context_ink("bridge context 18", "211;208;200")
    context_ink("bridge context 22", "131;129;126")
    tui.cmd("theme light").wait_for("Theme: light")
    context_ink("bridge context 18", "79;91;102")
    context_ink("bridge context 22", "102;112;120")
    tui.cmd("theme dark").wait_for("Theme: dark")
    tui.keys(".").wait_for(r"▶.*api change one").wait_for("stop 2 of 2 · story 1 of 2")
    tui.cmd("diff").wait_for("Diff view mode: side-by-side").wait_for(r"▶.*api change one")
    tui.cmd("set wrap").wait_for("Diff wrapping: on").wait_for(r"▶.*api change one")
    tui.resize(100, 24).wait_for("stop 2 of 2").wait_for(r"▶.*api change one")
    screen = tui.screen().splitlines()
    cursor = next(i for i, line in enumerate(screen) if "▶" in line and "api change one" in line)
    assert any("api context 17" in line for line in screen[:cursor]), tui.screen()
    tui.cmd("diff").wait_for("Diff view mode: unified").wait_for(r"▶.*api change one")
    tui.resize(160, 45).wait_for("stop 2 of 2 · story 1 of 2")
    tui.keys(",").wait_for(r"▶.*bridge context 17")
    tui.keys("kkkk").wait_for("Off stop")
    context_ink("bridge context 22", "211;208;200")
    tui.keys("'").wait_for(r"▶.*bridge context 17").wait_gone("Off stop")
    context_ink("bridge context 22", "131;129;126")
    tui.keys(".").wait_for(r"▶.*api change one")
    tui.keys("p").wait_for(r"▶.*finish change one").wait_for("stop 1 of 1 · story 2 of 2")
    tui.wait_for(r"Request routing ✓ 2/2").reject("Bridge forwards the request")
    tui.cmd("guide off").wait_for(r"Review paused: 3 stops.*\. resume")
    tui.quit()

    resumed = s.start("-w", "--guide", guide, cwd=repo, keep=True)
    resumed.wait_for(r"\. resume").keys(".").wait_for(r"▶.*finish change one")
    resumed.keys(".").wait_for("Stories finished").wait_for("1 parked · 1 supporting hunk left")
    resumed.wait_for(r":spot +:coverage").reject(r":submit")
    resumed.wait_for(r"D docs \(1\)").keys(";h", "G", "D").wait_for("✓ docs")
    resumed.wait_for("0 supporting hunks left")
    resumed.cmd("coverage").wait_for("Review guide covers 3 of 4 hunks").keys("Escape")
    resumed.keys("t").wait_for(r"Files ·").cmd("map").wait_for(r"\bMap\b")
    resumed.keys("g", "Enter").wait_for(r"▶.*bridge context 17")
    resumed.quit()
