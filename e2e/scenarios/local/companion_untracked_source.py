#!/usr/bin/env python3
# Fork review guide: for the working tree Ask runs in the repository itself, so untracked source is visible.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.git_repo(s.dir / "repo")
    repos.write(repo / "untracked-notes.txt", "From the new file: cobalt.\n")
    fake_claude(
        s,
        f"""
activity('Read')
assert Path.cwd().resolve() == Path({str(repo)!r}).resolve()
assert 'untracked-notes.txt' not in prompt
text = Path('untracked-notes.txt').read_text().strip()
assert text == 'From the new file: cobalt.'
answer(text)
""",
    )
    tui = s.start("-w", cwd=repo).wait_for("unstaged line").cursor_on("unstaged line")
    tui.keys("a").type("What did the repository's draft notes say?").keys("Enter")
    tui.wait_for("From the new file: cobalt.").keys("Escape").quit()
