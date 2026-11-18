#!/usr/bin/env python3
# Fork review guide: :o<N> selects the old side even when both sides occupy the same diff row.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    source = repos.write(repo / "sides.txt", "old side value\n")
    repos.git(repo, "add", ".")
    repos.git(repo, "commit", "-qm", "base")
    repos.write(source, "new side value\n")
    fake_claude(
        s,
        """
assert 'Review anchor: sides.txt:1 (old)' in prompt
assert 'Selected code (data):\\nold side value' in prompt
answer('The anchor points to the old side.')
""",
    )
    tui = s.start(cwd=repo)
    tui.wait_for("Unstaged changes").keys("Enter").cmd("diff").wait_for("old side value +│.*new side value")
    tui.cmd("o1").keys("a").wait_for(r"Private companion .*sides.txt:1 \(old\)")
    tui.type("Which side is this?").keys("Enter").wait_for("The anchor points to the old side")
    tui.keys("Escape").quit()
