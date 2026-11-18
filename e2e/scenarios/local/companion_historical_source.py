#!/usr/bin/env python3
# Review guide rebuild: Ask can read unchanged files at the exact reviewed revision.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    callers = repos.write(repo / "tests/callers.txt", "call_api reviewed caller\n")
    feature = repos.write(repo / "src/feature.txt", "API version 1\n")
    repos.git(repo, "add", ".")
    repos.git(repo, "commit", "-qm", "base")
    repos.write(feature, "API version 2\n")
    repos.git(repo, "commit", "-qam", "change API")
    repos.write(callers, "call_api CHECKOUT_ONLY_MARKER\n")

    fake_claude(
        s,
        f"""
assert "CHECKOUT_ONLY_MARKER" not in prompt
assert Path.cwd().resolve() != Path({str(repo)!r}).resolve()
assert not Path(".git").exists()
assert Path("tests/callers.txt").read_text() == "call_api reviewed caller\\n"
activity("Read")
answer("Found unchanged caller at the reviewed revision")
""",
    )
    tui = s.start("-r", "HEAD", cwd=repo, env={"TUICR_COMPANION": "claude"})
    tui.wait_for("API version 2").cursor_on(r"▌.*API version 2")
    tui.cmd("ask").wait_for("Private companion")
    tui.type("Where is the unchanged caller?").keys("Enter")
    tui.wait_for("Found unchanged caller at the reviewed revision")
    tui.keys("Escape").quit()
