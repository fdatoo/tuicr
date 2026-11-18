#!/usr/bin/env python3
# Fork review guide: an expanded context row asks at its real source path and line number.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    rows = [f"context-row-{n:02d}" for n in range(1, 46)]
    source = repos.write(repo / "long.txt", "\n".join(rows) + "\n")
    repos.git(repo, "add", ".")
    repos.git(repo, "commit", "-qm", "base")
    rows[3] = "changed upper boundary"
    rows[41] = "changed lower boundary"
    repos.write(source, "\n".join(rows) + "\n")
    fake_claude(
        s,
        """
assert 'Review anchor: long.txt:22 (new)' in prompt
assert 'Selected code (data):\\ncontext-row-22' in prompt
answer('The selected line is unchanged context.')
""",
    )
    tui = s.start(cwd=repo)
    tui.wait_for("Unstaged changes").keys("Enter").wait_for("changed upper boundary")
    tui.keys(":22", "Enter").wait_for(r"▶\s+22\s+context-row-22")
    tui.keys("a").wait_for(r"Private companion .*long.txt:22 \(new\)")
    tui.type("What does the context show?").keys("Enter").wait_for("The selected line is unchanged context")
    tui.wait_gone("Streaming").keys("Escape").wait_for(r"◆context-row-22").quit()
