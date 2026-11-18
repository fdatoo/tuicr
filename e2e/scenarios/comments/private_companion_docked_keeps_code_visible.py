#!/usr/bin/env python3
# Fork review guide: the dock leaves code visible and scrollable, with input and errors on short terminals.
import re

from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


def dock_row(screen: str) -> int:
    return next(row for row, line in enumerate(screen.splitlines()) if "Private companion" in line)


def first_diff_line(screen: str) -> int:
    for row in screen.splitlines()[: dock_row(screen)]:
        match = re.search(r"dock-pin-(\d+)", row)
        if match:
            return int(match[1])
    raise AssertionError(f"no diff line above the dock:\n{screen}")


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    source = repos.write(repo / "source.txt", "initial row\n")
    repos.git(repo, "add", ".")
    repos.git(repo, "commit", "-qm", "base")
    repos.write(source, "".join(f"dock-pin-{row:02d}\n" for row in range(1, 61)))
    fake_claude(s, "answer('Those three lines illustrate the change.')\n")
    tui = s.start(cwd=repo, cols=110, rows=34)
    tui.wait_for("Unstaged changes").keys("Enter").wait_for("dock-pin-01")
    tui.keys(":28", "Enter").wait_for(r"▶\s+28 ▌ dock-pin-28")
    tui.keys("a").wait_for(r"Private companion .*source.txt:28")
    screen = tui.screen(join=False)
    original_dock = dock_row(screen)
    assert "dock-pin-28" in "\n".join(screen.splitlines()[:original_dock]), screen
    tui.keys("C-Up").settle()
    grown = tui.screen(join=False)
    assert dock_row(grown) < original_dock and "dock-pin-28" in "\n".join(grown.splitlines()[: dock_row(grown)])
    tui.type("keep this question").wait_for("Question: keep this question")
    first = first_diff_line(grown)
    tui.type("\x1b[<65;60;6M" * 2).settle()
    scrolled = first_diff_line(tui.screen(join=False))
    assert scrolled > first, "wheel over the diff must scroll the code"
    tui.wait_for(r"Private companion .*source.txt:28").wait_for("Question: keep this question")
    tui.keys("S-Up").settle()
    assert first_diff_line(tui.screen(join=False)) < scrolled, "Shift-Up must scroll the diff"
    tui.keys("C-u")
    tui.resize(90, 9).keys("C-d").wait_for("Nothing to send").wait_for("Question:")
    tui.type("still composing").wait_for("Question: still composing")
    tui.keys("Escape").wait_for("NORMAL").keys(":q!", "Enter").wait_exit()
    tui = s.start(cwd=repo, cols=60, rows=15, keep=True)
    tui.wait_for("Unstaged changes").keys("Enter").wait_for("dock-pin-01")
    tui.cmd("28").wait_for(r"▶\s+28 ▌ dock-pin-28")
    tui.keys("v", "j", "j", "a").wait_for(r"Private companion .*source.txt:28–30")
    tui.type("What changed in this range?").keys("Enter")
    tui.wait_for("Those three lines").wait_for("Question:")
    screen = tui.screen(join=False)
    code = "\n".join(screen.splitlines()[: dock_row(screen)])
    assert all(f"dock-pin-{row}" in code for row in (28, 29, 30)), screen
    tui.keys("Escape").wait_for("VISUAL").keys("Escape").wait_for("NORMAL").quit()
