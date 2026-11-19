#!/usr/bin/env python3
# Review-guide rebuild: prepare, switch tabs, ring once, then open the prepared guide.
from shlex import quote

from e2e.prepare import fake_claude, github_pr_response
from e2e.tui import tmux

from e2e import Scenario, repos, scenario, wait_cmd, wait_file


@scenario
def main(s: Scenario) -> None:
    repo = repos.pr_repo(s.dir / "repo")
    release = s.dir / "release-reviewer"
    s.on_exit(release.touch)
    calls = fake_claude(s, response=github_pr_response(), digest_from_prompt=True, release=release)
    tui = s.start(cwd=repo, cols=100, env={"TMUX": "e2e", "TUICR_PREPARE_ON_OPEN": "1"})
    tui.wait_for("Pull Requests").keys("Tab").wait_for(r"#1 .*not prepared")
    assert not calls.exists(), "the list must not prepare without user consent"
    tui.check(r"p prepare · P cancel prep", "prepare and cancel must be discoverable")

    raw = s.dir / "pane.raw"
    tmux("pipe-pane", "-O", "-t", tui.session, f"cat > {quote(str(raw))}")
    tui.keys("p").wait_for(r"#1 .*preparing ·")
    wait_file(calls)
    tui.keys("Enter").wait_for("PR Description").wait_for("Preparing review ·")
    tui.cmd("prs").wait_for(r"#1 .*preparing ·")
    list_fetches = tui.forge_count(r"\"pr\", \"list\"")
    tui.keys("P").wait_for(r"#1 .*cancelled")
    tui.keys("p").wait_for(r"#1 .*preparing ·")
    wait_cmd(10, lambda: len(calls.read_text().splitlines()) >= 2)
    tui.keys("Tab").wait_for(r"guide +#1 .*preparing ·")
    release.touch()
    tui.wait_for(r"guide +#1 .*ready · \d+ stops · about 12 min", 20)
    wait_cmd(5, lambda: raw.exists() and b"\x07\x1bPtmux;" in raw.read_bytes())
    assert tui.forge_count(r"\"pr\", \"list\"") == list_fetches, "updating job status must not refetch the PR list"
    tui.keys("BTab").wait_for(r"#1 .*ready · \d+ stops · about 12 min")

    tui.keys("Enter").wait_for("PR Description").wait_for(r"Review ready: \d+ stops, about 12 min")
    tui.keys(".").wait_for("The greeting prints hello from the PR")
    output = raw.read_bytes()
    assert output.count(b"\x07") == 1, output[-2000:]
    assert b"\x1b\x1b]777;notify;Add greeting and tweak README;" in output, output[-2000:]
    tui.quit()
