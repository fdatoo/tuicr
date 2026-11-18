#!/usr/bin/env python3
# Fork review guide: missing providers and sign-in errors give actionable help and preserve the question.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.git_repo(s.dir / "repo")
    path = s.minimal_path(s.dir / "no-provider", "git")
    tui = s.start(cwd=repo, env={"PATH": path, "TUICR_COMPANION": "claude"})
    tui.wait_for("Unstaged changes").keys("Enter").cursor_on("unstaged line")
    tui.keys("a").type("private question").keys("Enter")
    tui.wait_for("Install .*claude.*PATH").wait_for("Question: private question")
    tui.keys("Escape").quit()

    fake_claude(
        s,
        """
emit({"type": "result", "subtype": "error", "is_error": True, "result": "Not logged in"})
""",
    )
    tui = s.start(cwd=repo, keep=True)
    tui.wait_for("Unstaged changes").keys("Enter").cursor_on("unstaged line")
    tui.keys("a").type("private question").keys("Enter")
    tui.wait_for(r"run `/login`").wait_for("Question: private question")
    tui.keys("Escape").quit()
